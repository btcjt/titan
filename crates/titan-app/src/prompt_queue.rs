//! Pending approval request queue for the built-in signer.
//!
//! When a content page calls a sensitive NIP-07 method, the dispatch path
//! checks the permission store. If approval is needed, it creates a
//! `PendingRequest`, pushes it onto the queue, emits a `signer-prompt`
//! event to the chrome, and awaits the user's decision via a oneshot
//! channel. The chrome shows a modal, the user clicks approve/deny, and
//! chrome calls `resolve_prompt(id, decision, scope)` which sends the
//! answer through the oneshot.
//!
//! Multiple requests can queue up if several come in before the user
//! responds — the chrome processes them one at a time.

use crate::permissions::Scope;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// Per-site cap on outstanding pending prompts.
///
/// Protects against memory-exhaustion DoS from a hostile nsite that fires
/// `signEvent` (or any sensitive method) in a loop while the user is AFK.
/// Each pending request holds a cloned `Value` of the raw params — without
/// a cap, a site calling `signEvent` with a 1 MB `content` field in a loop
/// could grow the queue unbounded before the 60s timeout reaps any entries.
///
/// 16 is a generous ceiling for any legitimate UI — real sites should
/// never have more than a handful of prompts in flight at once.
pub const MAX_PENDING_PER_SITE: usize = 16;

/// Global cap on outstanding pending prompts across all sites.
///
/// A second line of defense if somehow many sites each reach their
/// per-site cap. 128 is plenty for normal browsing.
pub const MAX_PENDING_TOTAL: usize = 128;

/// Error returned when the queue refuses a new prompt due to caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    /// The calling site already has `MAX_PENDING_PER_SITE` prompts waiting.
    PerSiteLimitExceeded,
    /// The global queue already has `MAX_PENDING_TOTAL` prompts waiting.
    GlobalLimitExceeded,
}

impl std::fmt::Display for PushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushError::PerSiteLimitExceeded => write!(
                f,
                "Too many pending approval prompts for this site (max {})",
                MAX_PENDING_PER_SITE
            ),
            PushError::GlobalLimitExceeded => write!(
                f,
                "Too many pending approval prompts across all sites (max {})",
                MAX_PENDING_TOTAL
            ),
        }
    }
}

/// Represents a single pending approval request.
pub struct PendingRequest {
    pub id: String,
    pub site: String,
    pub method: String,
    pub params: Value,
    /// Responder for delivering the decision back to the dispatcher.
    pub responder: oneshot::Sender<PromptResult>,
}

/// Snapshot of a pending request (without the responder) for serializing
/// to the chrome webview.
#[derive(Debug, Clone, Serialize)]
pub struct PendingRequestSnapshot {
    pub id: String,
    pub site: String,
    pub method: String,
    pub params: Value,
}

/// Payload the chrome sends back to resolve a pending prompt.
#[derive(Debug, Deserialize)]
pub struct PromptResolution {
    pub id: String,
    pub approved: bool,
    pub scope: Scope,
}

/// What the dispatcher receives back.
#[derive(Debug, Clone, Copy)]
pub struct PromptResult {
    pub approved: bool,
    pub scope: Scope,
}

#[derive(Default)]
pub struct PromptQueue {
    pending: Mutex<Vec<PendingRequest>>,
}

impl PromptQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a new pending request. Returns the receiver the dispatcher
    /// should await, or a `PushError` if queue caps would be exceeded.
    ///
    /// The caller is responsible for emitting the `signer-prompt` event
    /// after a successful push.
    pub fn push(
        &self,
        id: String,
        site: String,
        method: String,
        params: Value,
    ) -> Result<oneshot::Receiver<PromptResult>, PushError> {
        let mut guard = self.pending.lock().unwrap();

        // Global cap first — prevents the per-site count check from
        // masking a runaway situation where many sites each fill up.
        if guard.len() >= MAX_PENDING_TOTAL {
            return Err(PushError::GlobalLimitExceeded);
        }

        // Per-site cap — the primary DoS defense. Count how many pending
        // entries the calling site already has in flight.
        let site_count = guard.iter().filter(|r| r.site == site).count();
        if site_count >= MAX_PENDING_PER_SITE {
            return Err(PushError::PerSiteLimitExceeded);
        }

        let (tx, rx) = oneshot::channel();
        let req = PendingRequest {
            id,
            site,
            method,
            params,
            responder: tx,
        };
        guard.push(req);
        Ok(rx)
    }

    /// Snapshot of all pending requests for the UI.
    pub fn snapshot(&self) -> Vec<PendingRequestSnapshot> {
        self.pending
            .lock()
            .unwrap()
            .iter()
            .map(|p| PendingRequestSnapshot {
                id: p.id.clone(),
                site: p.site.clone(),
                method: p.method.clone(),
                params: p.params.clone(),
            })
            .collect()
    }

    /// Resolve a pending prompt by id. Returns true on success.
    pub fn resolve(&self, resolution: PromptResolution) -> bool {
        let mut guard = self.pending.lock().unwrap();
        if let Some(idx) = guard.iter().position(|r| r.id == resolution.id) {
            let req = guard.remove(idx);
            let result = PromptResult {
                approved: resolution.approved,
                scope: resolution.scope,
            };
            // The dispatcher may have given up waiting, in which case send()
            // fails. That's OK — the dispatcher's error path handles it.
            let _ = req.responder.send(result);
            true
        } else {
            false
        }
    }

    /// Deny and clear every pending request. Called when the signer is
    /// locked while prompts are outstanding.
    pub fn deny_all(&self) {
        let mut guard = self.pending.lock().unwrap();
        for req in guard.drain(..) {
            let _ = req.responder.send(PromptResult {
                approved: false,
                scope: Scope::AllowOnce,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn push_and_resolve_roundtrip() {
        let queue = PromptQueue::new();
        let rx = queue
            .push(
                "req-1".to_string(),
                "titan".to_string(),
                "signEvent".to_string(),
                json!({"kind": 1, "content": "hi"}),
            )
            .unwrap();

        // Snapshot reflects the pending request
        let snapshot = queue.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].id, "req-1");

        // Resolve it
        assert!(queue.resolve(PromptResolution {
            id: "req-1".to_string(),
            approved: true,
            scope: Scope::AllowOnce,
        }));

        // Receiver gets the decision
        let result = rx.await.unwrap();
        assert!(result.approved);
        assert_eq!(result.scope, Scope::AllowOnce);

        // Queue is empty
        assert_eq!(queue.snapshot().len(), 0);
    }

    #[tokio::test]
    async fn resolve_unknown_id_returns_false() {
        let queue = PromptQueue::new();
        assert!(!queue.resolve(PromptResolution {
            id: "nonexistent".to_string(),
            approved: true,
            scope: Scope::AllowOnce,
        }));
    }

    #[tokio::test]
    async fn deny_all_rejects_outstanding_requests() {
        let queue = PromptQueue::new();
        let rx1 = queue
            .push(
                "a".to_string(),
                "site1".to_string(),
                "signEvent".to_string(),
                json!({}),
            )
            .unwrap();
        let rx2 = queue
            .push(
                "b".to_string(),
                "site2".to_string(),
                "nip44.encrypt".to_string(),
                json!({}),
            )
            .unwrap();

        queue.deny_all();

        let r1 = rx1.await.unwrap();
        let r2 = rx2.await.unwrap();
        assert!(!r1.approved);
        assert!(!r2.approved);
        assert_eq!(queue.snapshot().len(), 0);
    }

    #[tokio::test]
    async fn multiple_pending_preserves_order() {
        let queue = PromptQueue::new();
        let _rx1 = queue
            .push(
                "a".to_string(),
                "x".to_string(),
                "signEvent".to_string(),
                json!({}),
            )
            .unwrap();
        let _rx2 = queue
            .push(
                "b".to_string(),
                "x".to_string(),
                "signEvent".to_string(),
                json!({}),
            )
            .unwrap();

        let snap = queue.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].id, "a");
        assert_eq!(snap[1].id, "b");
    }

    #[tokio::test]
    async fn per_site_cap_blocks_excess_prompts() {
        let queue = PromptQueue::new();
        // Fill the per-site quota for "attacker"
        let mut receivers = Vec::new();
        for i in 0..MAX_PENDING_PER_SITE {
            let rx = queue
                .push(
                    format!("req-{i}"),
                    "attacker".to_string(),
                    "signEvent".to_string(),
                    json!({"kind": 1, "content": "x"}),
                )
                .unwrap();
            receivers.push(rx);
        }

        // The next push for the same site must fail with PerSiteLimitExceeded
        let err = queue
            .push(
                "overflow".to_string(),
                "attacker".to_string(),
                "signEvent".to_string(),
                json!({}),
            )
            .unwrap_err();
        assert_eq!(err, PushError::PerSiteLimitExceeded);

        // A different site is still allowed
        assert!(queue
            .push(
                "other-1".to_string(),
                "honest-site".to_string(),
                "signEvent".to_string(),
                json!({}),
            )
            .is_ok());
    }

    #[tokio::test]
    async fn per_site_cap_recovers_after_resolve() {
        let queue = PromptQueue::new();
        // Fill to exactly the cap
        for i in 0..MAX_PENDING_PER_SITE {
            queue
                .push(
                    format!("req-{i}"),
                    "site".to_string(),
                    "signEvent".to_string(),
                    json!({}),
                )
                .unwrap();
        }
        // Overflow is blocked
        assert!(queue
            .push(
                "overflow".to_string(),
                "site".to_string(),
                "signEvent".to_string(),
                json!({}),
            )
            .is_err());

        // Resolve one — a slot frees up
        assert!(queue.resolve(PromptResolution {
            id: "req-0".to_string(),
            approved: true,
            scope: Scope::AllowOnce,
        }));

        // Now a new push succeeds
        assert!(queue
            .push(
                "req-new".to_string(),
                "site".to_string(),
                "signEvent".to_string(),
                json!({}),
            )
            .is_ok());
    }

    #[tokio::test]
    async fn global_cap_blocks_many_sites() {
        let queue = PromptQueue::new();
        // Spread MAX_PENDING_TOTAL across many sites, each under the
        // per-site cap. Needs at least MAX_PENDING_TOTAL / MAX_PENDING_PER_SITE
        // sites = 128 / 16 = 8 sites each with 16 prompts.
        let sites_needed = MAX_PENDING_TOTAL / MAX_PENDING_PER_SITE;
        for site_idx in 0..sites_needed {
            for req_idx in 0..MAX_PENDING_PER_SITE {
                queue
                    .push(
                        format!("s{site_idx}-r{req_idx}"),
                        format!("site-{site_idx}"),
                        "signEvent".to_string(),
                        json!({}),
                    )
                    .unwrap();
            }
        }
        assert_eq!(queue.snapshot().len(), MAX_PENDING_TOTAL);

        // A new site (zero pending) still hits the global cap
        let err = queue
            .push(
                "fresh".to_string(),
                "new-site".to_string(),
                "signEvent".to_string(),
                json!({}),
            )
            .unwrap_err();
        assert_eq!(err, PushError::GlobalLimitExceeded);
    }

    #[test]
    fn push_error_display_includes_limits() {
        // Make sure the error messages surface the actual numbers, so the
        // content page's error log is actionable.
        let per_site = PushError::PerSiteLimitExceeded.to_string();
        assert!(per_site.contains(&MAX_PENDING_PER_SITE.to_string()));
        let global = PushError::GlobalLimitExceeded.to_string();
        assert!(global.contains(&MAX_PENDING_TOTAL.to_string()));
    }
}
