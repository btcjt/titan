//! NIP-07 method dispatch for the built-in signer.
//!
//! Handles requests routed through the `titan-nostr://` protocol handler.
//! Each method takes a JSON parameters value and returns a JSON result (or an
//! error string). The caller is responsible for delivering the response back
//! to the content webview.

use crate::signer::Signer;
use nostr_sdk::nips::{nip04, nip44};
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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

/// Dispatch a NIP-07 request against the signer.
///
/// Future phases will consult the permission model and approval prompts
/// before executing sensitive methods. For now, unlocked == allowed.
pub async fn dispatch(signer: &Signer, req: NostrRequest) -> NostrResponse {
    if !signer.is_unlocked() {
        if !signer.has_identity() {
            return NostrResponse::err(&req.id, "No Nostr identity configured in Titan");
        }
        return NostrResponse::err(&req.id, "Titan signer is locked");
    }

    match req.method.as_str() {
        "getPublicKey" => dispatch_get_public_key(signer, &req),
        "signEvent" => dispatch_sign_event(signer, &req),
        "getRelays" => dispatch_get_relays(signer, &req),
        "nip04.encrypt" => dispatch_nip04_encrypt(signer, &req),
        "nip04.decrypt" => dispatch_nip04_decrypt(signer, &req),
        "nip44.encrypt" => dispatch_nip44_encrypt(signer, &req),
        "nip44.decrypt" => dispatch_nip44_decrypt(signer, &req),
        other => NostrResponse::err(&req.id, format!("Unknown method: {other}")),
    }
}

fn dispatch_get_public_key(signer: &Signer, req: &NostrRequest) -> NostrResponse {
    match signer.get_pubkey() {
        Some(hex) => NostrResponse::ok(&req.id, Value::String(hex)),
        None => NostrResponse::err(&req.id, "Signer locked"),
    }
}

fn dispatch_get_relays(_signer: &Signer, req: &NostrRequest) -> NostrResponse {
    // For v1 we return an empty object. Once Settings exposes a personal
    // relay list with read/write markers we'll plumb that through here.
    NostrResponse::ok(&req.id, json!({}))
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
}
