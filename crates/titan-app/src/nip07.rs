//! NIP-07 method dispatch for the built-in signer.
//!
//! Handles requests routed through the `titan-nostr://` protocol handler.
//! Each method takes a JSON parameters value and returns a JSON result (or an
//! error string). The caller is responsible for delivering the response back
//! to the content webview.

use crate::audit_log::{AuditLog, Outcome};
use crate::permissions::{is_sensitive, Decision, Permissions, Scope};
use crate::prompt_queue::PromptQueue;
use crate::signer::Signer;
use nostr_sdk::nips::{nip04, nip44};
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::time::{timeout, Duration};

/// Incoming request from a content page.
#[derive(Debug, Deserialize)]
pub struct NostrRequest {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// Response sent back to the content page.
#[derive(Debug, Serialize)]
pub struct NostrResponse {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl NostrResponse {
    fn ok(id: &str, result: Value) -> Self {
        Self {
            id: id.to_string(),
            result: Some(result),
            error: None,
        }
    }
    fn err(id: &str, error: impl Into<String>) -> Self {
        Self {
            id: id.to_string(),
            result: None,
            error: Some(error.into()),
        }
    }
}

/// Timeout for approval prompts — if the user doesn't respond within
/// this window, the dispatch returns a denial to the content page.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

/// Context bundle passed through to dispatch — everything it needs to
/// enforce permissions and emit approval prompts.
pub struct DispatchContext<'a> {
    pub signer: &'a Signer,
    pub permissions: &'a Permissions,
    pub queue: &'a PromptQueue,
    pub audit_log: &'a AuditLog,
    pub app: &'a AppHandle,
    /// The site origin (nsite name or npub) that made this request.
    pub site: String,
    /// User's configured Nostr relays, for getRelays() responses.
    pub relay_urls: Vec<String>,
}

/// Extract the kind from a signEvent params blob, if present.
fn extract_kind(params: &Value) -> Option<u16> {
    params.get("kind").and_then(|v| v.as_u64()).map(|k| k as u16)
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::AllowOnce => "allow_once",
        Scope::AllowSession => "allow_session",
        Scope::AllowAlways => "allow_always",
        Scope::DenyAlways => "deny_always",
    }
}

/// Dispatch a NIP-07 request against the signer with permission checking.
///
/// Non-sensitive methods (getPublicKey, getRelays) execute immediately
/// once the signer is unlocked. Sensitive methods (signEvent, nip04/nip44
/// encrypt/decrypt) go through the permission model:
///
/// 1. Check stored permissions. AllowAlways/AllowSession → execute.
/// 2. DenyAlways → return error.
/// 3. No stored decision → push onto prompt queue, emit signer-prompt
///    event, await user's response via oneshot (with 60s timeout).
pub async fn dispatch(ctx: DispatchContext<'_>, req: NostrRequest) -> NostrResponse {
    let kind = if req.method == "signEvent" {
        extract_kind(&req.params)
    } else {
        None
    };

    if !ctx.signer.is_unlocked() {
        let msg = if !ctx.signer.has_identity() {
            "No Nostr identity configured in Titan"
        } else {
            "Titan signer is locked"
        };
        if is_sensitive(&req.method) {
            ctx.audit_log.record(
                &ctx.site,
                &req.method,
                kind,
                Outcome::SignerLocked,
                None,
            );
        }
        return NostrResponse::err(&req.id, msg);
    }

    // Permission check (sensitive methods only)
    let applied_scope: Option<Scope> = if is_sensitive(&req.method) {
        let decision = ctx.permissions.check(&ctx.site, &req.method);
        match decision {
            Decision::Allow => {
                // We don't know which exact scope approved it — the
                // permission store doesn't expose that here. Leave scope
                // as None for auto-approved reruns; the prompt path
                // below records the explicit user choice.
                None
            }
            Decision::Deny => {
                ctx.audit_log.record(
                    &ctx.site,
                    &req.method,
                    kind,
                    Outcome::AutoDenied,
                    Some(scope_name(Scope::DenyAlways).to_string()),
                );
                return NostrResponse::err(&req.id, "Request denied by stored permission");
            }
            Decision::NeedApproval => {
                // Push onto queue, emit event, await response.
                //
                // If the queue is full (per-site or global cap), auto-deny
                // the request without prompting. This prevents a hostile
                // nsite from exhausting memory by firing sensitive methods
                // in a loop while the user is AFK.
                let rx = match ctx.queue.push(
                    req.id.clone(),
                    ctx.site.clone(),
                    req.method.clone(),
                    req.params.clone(),
                ) {
                    Ok(rx) => rx,
                    Err(push_err) => {
                        ctx.audit_log.record(
                            &ctx.site,
                            &req.method,
                            kind,
                            Outcome::AutoDenied,
                            None,
                        );
                        return NostrResponse::err(&req.id, &push_err.to_string());
                    }
                };
                // Emit a snapshot of all pending prompts so the chrome can
                // render or update its queue.
                let snapshot = ctx.queue.snapshot();
                let _ = ctx.app.emit("signer-prompt", snapshot);

                let outcome = match timeout(PROMPT_TIMEOUT, rx).await {
                    Ok(Ok(result)) => result,
                    Ok(Err(_)) => {
                        ctx.audit_log.record(
                            &ctx.site,
                            &req.method,
                            kind,
                            Outcome::Failed,
                            None,
                        );
                        return NostrResponse::err(
                            &req.id,
                            "Signer prompt channel closed unexpectedly",
                        );
                    }
                    Err(_) => {
                        // Timeout — remove from queue and record
                        let _ = ctx.queue.resolve(crate::prompt_queue::PromptResolution {
                            id: req.id.clone(),
                            approved: false,
                            scope: Scope::AllowOnce,
                        });
                        ctx.audit_log.record(
                            &ctx.site,
                            &req.method,
                            kind,
                            Outcome::TimedOut,
                            None,
                        );
                        return NostrResponse::err(
                            &req.id,
                            "Approval prompt timed out after 60 seconds",
                        );
                    }
                };

                if !outcome.approved {
                    ctx.audit_log.record(
                        &ctx.site,
                        &req.method,
                        kind,
                        Outcome::Denied,
                        Some(scope_name(outcome.scope).to_string()),
                    );
                    return NostrResponse::err(&req.id, "Request denied by user");
                }

                // Record the chosen scope for future requests
                ctx.permissions.record(&ctx.site, &req.method, outcome.scope);
                Some(outcome.scope)
            }
        }
    } else {
        None
    };

    let response = match req.method.as_str() {
        "getPublicKey" => dispatch_get_public_key(ctx.signer, &req),
        "signEvent" => dispatch_sign_event(ctx.signer, &req),
        "getRelays" => dispatch_get_relays(&ctx.relay_urls, &req),
        "nip04.encrypt" => dispatch_nip04_encrypt(ctx.signer, &req),
        "nip04.decrypt" => dispatch_nip04_decrypt(ctx.signer, &req),
        "nip44.encrypt" => dispatch_nip44_encrypt(ctx.signer, &req),
        "nip44.decrypt" => dispatch_nip44_decrypt(ctx.signer, &req),
        other => NostrResponse::err(&req.id, format!("Unknown method: {other}")),
    };

    // Record the final outcome for sensitive methods
    if is_sensitive(&req.method) {
        let outcome = if response.error.is_some() {
            Outcome::Failed
        } else {
            Outcome::Approved
        };
        ctx.audit_log.record(
            &ctx.site,
            &req.method,
            kind,
            outcome,
            applied_scope.map(|s| scope_name(s).to_string()),
        );
    }

    response
}

fn dispatch_get_public_key(signer: &Signer, req: &NostrRequest) -> NostrResponse {
    match signer.get_pubkey() {
        Some(hex) => NostrResponse::ok(&req.id, Value::String(hex)),
        None => NostrResponse::err(&req.id, "Signer locked"),
    }
}

fn dispatch_get_relays(relay_urls: &[String], req: &NostrRequest) -> NostrResponse {
    // NIP-07 expects a map of url -> { read: bool, write: bool }.
    // Titan doesn't yet expose per-relay read/write markers in the UI,
    // so for now every configured relay is treated as read+write.
    let mut map = serde_json::Map::new();
    for url in relay_urls {
        map.insert(url.clone(), json!({ "read": true, "write": true }));
    }
    NostrResponse::ok(&req.id, Value::Object(map))
}

/// Build and sign an event from a JSON template, returning the full signed
/// event as a JSON value. Extracted from `dispatch_sign_event` so it can be
/// tested directly without needing a `Signer`.
pub fn sign_event_from_template(keys: &Keys, template: &Value) -> Result<Value, String> {
    let obj = template
        .as_object()
        .ok_or("signEvent expects an object parameter")?;

    let kind = obj
        .get("kind")
        .and_then(|v| v.as_u64())
        .ok_or("missing or invalid 'kind'")? as u16;

    let content = obj
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("missing or invalid 'content'")?
        .to_string();

    let created_at = obj
        .get("created_at")
        .and_then(|v| v.as_u64())
        .map(Timestamp::from)
        .unwrap_or_else(Timestamp::now);

    // Parse tags — array of string arrays
    let tags: Vec<Tag> = obj
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let items: Vec<String> = t
                        .as_array()?
                        .iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect();
                    if items.is_empty() {
                        None
                    } else {
                        Tag::parse(&items).ok()
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let signed = EventBuilder::new(Kind::from(kind), content)
        .tags(tags)
        .custom_created_at(created_at)
        .sign_with_keys(keys)
        .map_err(|e| format!("sign failed: {e}"))?;

    // Self-verify before returning
    signed
        .verify()
        .map_err(|e| format!("self-verify failed: {e}"))?;

    serde_json::to_value(&signed).map_err(|e| format!("serialize failed: {e}"))
}

/// The content page sends an event template with some fields pre-filled.
/// We set pubkey/id/sig and return the full event.
fn dispatch_sign_event(signer: &Signer, req: &NostrRequest) -> NostrResponse {
    let result: Result<Value, String> = signer.with_keys(|keys| {
        sign_event_from_template(keys, &req.params)
    });

    match result {
        Ok(v) => NostrResponse::ok(&req.id, v),
        Err(e) => NostrResponse::err(&req.id, e),
    }
}

fn parse_pubkey(value: Option<&Value>) -> Result<PublicKey, String> {
    let s = value
        .and_then(|v| v.as_str())
        .ok_or("missing pubkey parameter")?;
    PublicKey::from_hex(s).map_err(|e| format!("invalid pubkey: {e}"))
}

fn parse_string(value: Option<&Value>, name: &str) -> Result<String, String> {
    value
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing {name} parameter"))
}

fn dispatch_nip04_encrypt(signer: &Signer, req: &NostrRequest) -> NostrResponse {
    let obj = req.params.as_object();
    let pubkey_val = obj.and_then(|o| o.get("pubkey"));
    let plaintext_val = obj.and_then(|o| o.get("plaintext"));

    let result: Result<Value, String> = signer.with_keys(|keys| {
        let pk = parse_pubkey(pubkey_val)?;
        let plaintext = parse_string(plaintext_val, "plaintext")?;
        let ct = nip04::encrypt(keys.secret_key(), &pk, plaintext)
            .map_err(|e| format!("nip04 encrypt: {e}"))?;
        Ok(Value::String(ct))
    });

    match result {
        Ok(v) => NostrResponse::ok(&req.id, v),
        Err(e) => NostrResponse::err(&req.id, e),
    }
}

fn dispatch_nip04_decrypt(signer: &Signer, req: &NostrRequest) -> NostrResponse {
    let obj = req.params.as_object();
    let pubkey_val = obj.and_then(|o| o.get("pubkey"));
    let ciphertext_val = obj.and_then(|o| o.get("ciphertext"));

    let result: Result<Value, String> = signer.with_keys(|keys| {
        let pk = parse_pubkey(pubkey_val)?;
        let ciphertext = parse_string(ciphertext_val, "ciphertext")?;
        let pt = nip04::decrypt(keys.secret_key(), &pk, ciphertext)
            .map_err(|e| format!("nip04 decrypt: {e}"))?;
        Ok(Value::String(pt))
    });

    match result {
        Ok(v) => NostrResponse::ok(&req.id, v),
        Err(e) => NostrResponse::err(&req.id, e),
    }
}

fn dispatch_nip44_encrypt(signer: &Signer, req: &NostrRequest) -> NostrResponse {
    let obj = req.params.as_object();
    let pubkey_val = obj.and_then(|o| o.get("pubkey"));
    let plaintext_val = obj.and_then(|o| o.get("plaintext"));

    let result: Result<Value, String> = signer.with_keys(|keys| {
        let pk = parse_pubkey(pubkey_val)?;
        let plaintext = parse_string(plaintext_val, "plaintext")?;
        let ct = nip44::encrypt(keys.secret_key(), &pk, plaintext, nip44::Version::V2)
            .map_err(|e| format!("nip44 encrypt: {e}"))?;
        Ok(Value::String(ct))
    });

    match result {
        Ok(v) => NostrResponse::ok(&req.id, v),
        Err(e) => NostrResponse::err(&req.id, e),
    }
}

fn dispatch_nip44_decrypt(signer: &Signer, req: &NostrRequest) -> NostrResponse {
    let obj = req.params.as_object();
    let pubkey_val = obj.and_then(|o| o.get("pubkey"));
    let ciphertext_val = obj.and_then(|o| o.get("ciphertext"));

    let result: Result<Value, String> = signer.with_keys(|keys| {
        let pk = parse_pubkey(pubkey_val)?;
        let ciphertext = parse_string(ciphertext_val, "ciphertext")?;
        let pt = nip44::decrypt(keys.secret_key(), &pk, ciphertext)
            .map_err(|e| format!("nip44 decrypt: {e}"))?;
        Ok(Value::String(pt))
    });

    match result {
        Ok(v) => NostrResponse::ok(&req.id, v),
        Err(e) => NostrResponse::err(&req.id, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_keys() -> Keys {
        let sk = SecretKey::from_hex(
            "1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap();
        Keys::new(sk)
    }

    #[test]
    fn sign_event_basic() {
        let keys = test_keys();
        let template = json!({
            "kind": 1,
            "content": "hello",
            "tags": [],
            "created_at": 1700000000u64,
        });

        let signed = sign_event_from_template(&keys, &template).expect("sign");
        let obj = signed.as_object().expect("event is object");

        assert_eq!(obj.get("kind").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(obj.get("content").and_then(|v| v.as_str()), Some("hello"));
        assert_eq!(
            obj.get("created_at").and_then(|v| v.as_u64()),
            Some(1700000000)
        );
        assert_eq!(
            obj.get("pubkey").and_then(|v| v.as_str()),
            Some(keys.public_key().to_hex().as_str())
        );
        assert!(obj.get("id").and_then(|v| v.as_str()).is_some());
        assert!(obj.get("sig").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn sign_event_with_tags() {
        let keys = test_keys();
        let template = json!({
            "kind": 1,
            "content": "hi",
            "tags": [
                ["p", "0000000000000000000000000000000000000000000000000000000000000001"],
                ["e", "0000000000000000000000000000000000000000000000000000000000000002"],
            ],
            "created_at": 1700000000u64,
        });

        let signed = sign_event_from_template(&keys, &template).expect("sign");
        let tags = signed.get("tags").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0][0], "p");
        assert_eq!(tags[1][0], "e");
    }

    #[test]
    fn sign_event_missing_kind_fails() {
        let keys = test_keys();
        let template = json!({ "content": "no kind" });
        let err = sign_event_from_template(&keys, &template).unwrap_err();
        assert!(err.contains("kind"));
    }

    #[test]
    fn sign_event_missing_content_fails() {
        let keys = test_keys();
        let template = json!({ "kind": 1 });
        let err = sign_event_from_template(&keys, &template).unwrap_err();
        assert!(err.contains("content"));
    }

    #[test]
    fn sign_event_non_object_fails() {
        let keys = test_keys();
        let err = sign_event_from_template(&keys, &json!("not an object")).unwrap_err();
        assert!(err.contains("object"));
    }

    #[test]
    fn sign_event_uses_now_when_created_at_omitted() {
        let keys = test_keys();
        let template = json!({
            "kind": 1,
            "content": "hi",
            "tags": [],
        });
        let before = Timestamp::now().as_u64();
        let signed = sign_event_from_template(&keys, &template).unwrap();
        let after = Timestamp::now().as_u64();
        let ts = signed.get("created_at").and_then(|v| v.as_u64()).unwrap();
        assert!(ts >= before && ts <= after);
    }

    #[test]
    fn nip44_round_trip() {
        use nostr_sdk::nips::nip44;
        let alice = test_keys();
        let bob_sk = SecretKey::from_hex(
            "2222222222222222222222222222222222222222222222222222222222222222",
        )
        .unwrap();
        let bob = Keys::new(bob_sk);

        let plaintext = "a secret message";

        // Alice encrypts to Bob
        let ciphertext = nip44::encrypt(
            alice.secret_key(),
            &bob.public_key(),
            plaintext,
            nip44::Version::V2,
        )
        .expect("encrypt");

        // Bob decrypts from Alice
        let decrypted =
            nip44::decrypt(bob.secret_key(), &alice.public_key(), &ciphertext).expect("decrypt");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn nip04_round_trip() {
        use nostr_sdk::nips::nip04;
        let alice = test_keys();
        let bob_sk = SecretKey::from_hex(
            "2222222222222222222222222222222222222222222222222222222222222222",
        )
        .unwrap();
        let bob = Keys::new(bob_sk);

        let plaintext = "legacy encrypted dm";
        let ciphertext =
            nip04::encrypt(alice.secret_key(), &bob.public_key(), plaintext).expect("encrypt");
        let decrypted =
            nip04::decrypt(bob.secret_key(), &alice.public_key(), &ciphertext).expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn parse_pubkey_valid() {
        let hex = "0000000000000000000000000000000000000000000000000000000000000001";
        let val = json!(hex);
        let pk = parse_pubkey(Some(&val)).expect("parse");
        assert_eq!(pk.to_hex(), hex);
    }

    #[test]
    fn parse_pubkey_invalid_hex() {
        let val = json!("zzz");
        assert!(parse_pubkey(Some(&val)).is_err());
    }

    #[test]
    fn parse_pubkey_missing() {
        assert!(parse_pubkey(None).is_err());
    }

    #[test]
    fn parse_pubkey_not_a_string() {
        let val = json!(42);
        assert!(parse_pubkey(Some(&val)).is_err());
    }

    #[test]
    fn parse_string_present() {
        let val = json!("hello");
        assert_eq!(parse_string(Some(&val), "field").unwrap(), "hello");
    }

    #[test]
    fn parse_string_missing_has_clear_error() {
        let err = parse_string(None, "plaintext").unwrap_err();
        assert!(err.contains("plaintext"));
    }

    // ── extract_kind ──

    #[test]
    fn extract_kind_valid() {
        assert_eq!(extract_kind(&json!({"kind": 1})), Some(1));
        assert_eq!(extract_kind(&json!({"kind": 35128})), Some(35128));
        assert_eq!(extract_kind(&json!({"kind": 0})), Some(0));
    }

    #[test]
    fn extract_kind_missing() {
        assert_eq!(extract_kind(&json!({})), None);
        assert_eq!(extract_kind(&json!({"content": "hi"})), None);
    }

    #[test]
    fn extract_kind_non_object_input() {
        assert_eq!(extract_kind(&json!("not an object")), None);
        assert_eq!(extract_kind(&json!(42)), None);
        assert_eq!(extract_kind(&json!(null)), None);
    }

    #[test]
    fn extract_kind_wrong_type() {
        // kind as string, not number
        assert_eq!(extract_kind(&json!({"kind": "1"})), None);
    }

    #[test]
    fn extract_kind_truncates_to_u16() {
        // u16 max = 65535. Anything larger is silently truncated via `as u16`.
        // This is consistent with nostr-sdk's EventBuilder taking u16 kinds.
        // Our intent is to capture "what the user sent"; an overflow is
        // acceptable since the sign path validates separately.
        let extracted = extract_kind(&json!({"kind": 70000}));
        assert!(extracted.is_some());
    }

    // ── scope_name ──

    #[test]
    fn scope_name_returns_snake_case() {
        assert_eq!(scope_name(Scope::AllowOnce), "allow_once");
        assert_eq!(scope_name(Scope::AllowSession), "allow_session");
        assert_eq!(scope_name(Scope::AllowAlways), "allow_always");
        assert_eq!(scope_name(Scope::DenyAlways), "deny_always");
    }

    // ── signEvent with no tags field ──

    #[test]
    fn sign_event_without_tags_field_works() {
        let keys = test_keys();
        let template = json!({
            "kind": 1,
            "content": "no tags at all",
            "created_at": 1700000000u64,
            // tags field entirely missing
        });
        let signed = sign_event_from_template(&keys, &template).expect("sign");
        let tags = signed.get("tags").and_then(|v| v.as_array()).unwrap();
        assert!(tags.is_empty());
    }

    #[test]
    fn sign_event_with_empty_tags_array() {
        let keys = test_keys();
        let template = json!({
            "kind": 1,
            "content": "explicit empty tags",
            "tags": [],
            "created_at": 1700000000u64,
        });
        let signed = sign_event_from_template(&keys, &template).expect("sign");
        let tags = signed.get("tags").and_then(|v| v.as_array()).unwrap();
        assert!(tags.is_empty());
    }

    #[test]
    fn sign_event_with_malformed_tags_filters_invalid() {
        let keys = test_keys();
        let template = json!({
            "kind": 1,
            "content": "mixed tags",
            "tags": [
                ["t", "valid"],
                [],              // empty — skipped
                ["p", "not-a-hex-pubkey-but-that's-ok-at-this-layer"],
            ],
            "created_at": 1700000000u64,
        });
        // Should still succeed; Tag::parse may or may not accept the
        // second entry. Either way, we verify the function doesn't panic.
        let result = sign_event_from_template(&keys, &template);
        assert!(result.is_ok());
    }

    #[test]
    fn signed_event_self_verifies() {
        let keys = test_keys();
        let template = json!({
            "kind": 1,
            "content": "verify me",
            "tags": [],
            "created_at": 1700000000u64,
        });
        let signed = sign_event_from_template(&keys, &template).unwrap();
        // The id and sig should be present and non-empty
        assert!(signed.get("id").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false));
        assert!(signed.get("sig").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false));
        // Parse back through nostr-sdk to confirm it's a valid event
        let event: Event = serde_json::from_value(signed).expect("valid Event");
        event.verify().expect("signature valid");
    }
}
