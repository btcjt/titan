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
    /// should await.
    pub fn push(
        &self,
        id: String,
        site: String,
        method: String,
        params: Value,
    ) -> oneshot::Receiver<PromptResult> {
        let (tx, rx) = oneshot::channel();
        let req = PendingRequest {
            id,
            site,
            method,
            params,
            responder: tx,
        };
        self.pending.lock().unwrap().push(req);
        rx
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
        let rx = queue.push(
            "req-1".to_string(),
            "titan".to_string(),
            "signEvent".to_string(),
            json!({"kind": 1, "content": "hi"}),
        );

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
        let rx1 = queue.push(
            "a".to_string(),
            "site1".to_string(),
            "signEvent".to_string(),
            json!({}),
        );
        let rx2 = queue.push(
            "b".to_string(),
            "site2".to_string(),
            "nip44.encrypt".to_string(),
            json!({}),
        );

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
        let _rx1 = queue.push(
            "a".to_string(),
            "x".to_string(),
            "signEvent".to_string(),
            json!({}),
        );
        let _rx2 = queue.push(
            "b".to_string(),
            "x".to_string(),
            "signEvent".to_string(),
            json!({}),
        );

        let snap = queue.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].id, "a");
        assert_eq!(snap[1].id, "b");
    }
}
