//! In-memory audit log for the built-in signer.
//!
//! Records every sensitive NIP-07 request the signer handles, including
//! the site origin, method, event kind (for signEvent), outcome
//! (approved / denied / failed), and the permission scope that applied.
//!
//! The log is capped at `MAX_ENTRIES` and newest-first. It lives only in
//! memory — persisting across restarts would require write-heavy logging
//! on the hot path, and the log is meant for real-time user review, not
//! long-term forensics.
//!
//! The log is NOT cleared when the signer locks — users can still audit
//! past activity after locking, and re-locking wouldn't serve any
//! security purpose (the log contains no secrets, only metadata).

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The request was executed (possibly after an approval prompt or via
    /// a cached AllowAlways/AllowSession scope).
    Approved,
    /// The user explicitly denied the request in the approval prompt.
    Denied,
    /// A stored DenyAlways permission rejected the request without
    /// prompting.
    AutoDenied,
    /// The signer was locked or misconfigured at the time of the request.
    SignerLocked,
    /// The approval prompt timed out (no response within 60s).
    TimedOut,
    /// The request was otherwise rejected by the dispatcher (bad params,
    /// signing error, etc).
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub site: String,
    pub method: String,
    /// Kind number for signEvent, None for other methods.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<u16>,
    pub outcome: Outcome,
    /// Permission scope that applied ("allow_once", "allow_session",
    /// "allow_always", "deny_always", or None for read-only methods).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

pub struct AuditLog {
    entries: Mutex<Vec<AuditEntry>>,
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditLog {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::with_capacity(MAX_ENTRIES)),
        }
    }

    /// Record a single audit entry. Newest entries are at index 0.
    /// If the log exceeds `MAX_ENTRIES`, the oldest entry is dropped.
    pub fn record(
        &self,
        site: impl Into<String>,
        method: impl Into<String>,
        kind: Option<u16>,
        outcome: Outcome,
        scope: Option<String>,
    ) {
        let entry = AuditEntry {
            timestamp: unix_timestamp(),
            site: site.into(),
            method: method.into(),
            kind,
            outcome,
            scope,
        };
        let mut guard = self.entries.lock().unwrap();
        guard.insert(0, entry);
        if guard.len() > MAX_ENTRIES {
            guard.truncate(MAX_ENTRIES);
        }
    }

    /// Snapshot of all entries, newest first.
    pub fn list(&self) -> Vec<AuditEntry> {
        self.entries.lock().unwrap().clone()
    }

    /// Clear all entries.
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }

    /// Current entry count.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_entries_newest_first() {
        let log = AuditLog::new();
        log.record("alpha", "signEvent", Some(1), Outcome::Approved, Some("allow_once".to_string()));
        log.record("beta", "nip44.encrypt", None, Outcome::Denied, Some("allow_once".to_string()));

        let entries = log.list();
        assert_eq!(entries.len(), 2);
        // Newest first
        assert_eq!(entries[0].site, "beta");
        assert_eq!(entries[1].site, "alpha");
    }

    #[test]
    fn caps_at_max_entries() {
        let log = AuditLog::new();
        for i in 0..MAX_ENTRIES + 50 {
            log.record(
                format!("site{i}"),
                "signEvent",
                Some(1),
                Outcome::Approved,
                None,
            );
        }
        assert_eq!(log.len(), MAX_ENTRIES);
        let entries = log.list();
        // Most recent is site{N-1}, oldest kept is site{50}
        assert_eq!(entries[0].site, format!("site{}", MAX_ENTRIES + 49));
        assert_eq!(entries[MAX_ENTRIES - 1].site, "site50");
    }

    #[test]
    fn clear_wipes_all_entries() {
        let log = AuditLog::new();
        log.record("a", "signEvent", None, Outcome::Approved, None);
        log.record("b", "signEvent", None, Outcome::Approved, None);
        assert_eq!(log.len(), 2);
        log.clear();
        assert_eq!(log.len(), 0);
        assert!(log.list().is_empty());
    }

    #[test]
    fn kind_is_optional_in_entry() {
        let log = AuditLog::new();
        log.record("s", "signEvent", Some(0), Outcome::Approved, None);
        log.record("s", "nip44.encrypt", None, Outcome::Approved, None);

        let entries = log.list();
        assert_eq!(entries[0].kind, None);
        assert_eq!(entries[1].kind, Some(0));
    }

    #[test]
    fn outcome_serializes_to_snake_case() {
        assert_eq!(serde_json::to_string(&Outcome::Approved).unwrap(), "\"approved\"");
        assert_eq!(serde_json::to_string(&Outcome::AutoDenied).unwrap(), "\"auto_denied\"");
        assert_eq!(serde_json::to_string(&Outcome::SignerLocked).unwrap(), "\"signer_locked\"");
        assert_eq!(serde_json::to_string(&Outcome::TimedOut).unwrap(), "\"timed_out\"");
    }

    #[test]
    fn entry_omits_none_fields_in_json() {
        let log = AuditLog::new();
        log.record("s", "getPublicKey", None, Outcome::Approved, None);
        let entry = &log.list()[0];
        let json = serde_json::to_string(entry).unwrap();
        assert!(!json.contains("kind"));
        assert!(!json.contains("scope"));
    }
}
