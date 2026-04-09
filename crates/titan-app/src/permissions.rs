//! Per-site permission model for the built-in signer.
//!
//! Sensitive NIP-07 methods (signEvent, nip04.encrypt/decrypt,
//! nip44.encrypt/decrypt) require user approval before executing. Approvals
//! are scoped per site + per method and persisted to a JSON file in the
//! data directory.
//!
//! Scopes:
//! - `AllowOnce`: approved for this single request, no persistence
//! - `AllowSession`: approved until the signer is locked or app restarts
//! - `AllowAlways`: persisted, auto-approve forever
//! - `DenyAlways`: persisted, auto-deny silently
//!
//! A method call without a stored permission triggers a prompt. The user
//! picks a scope; `AllowAlways`/`DenyAlways` get written to disk.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Methods that require permission checking. Read-only methods like
/// getPublicKey / getRelays are always allowed when unlocked.
pub const SENSITIVE_METHODS: &[&str] = &[
    "signEvent",
    "nip04.encrypt",
    "nip04.decrypt",
    "nip44.encrypt",
    "nip44.decrypt",
];

pub fn is_sensitive(method: &str) -> bool {
    SENSITIVE_METHODS.contains(&method)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    AllowOnce,
    AllowSession,
    AllowAlways,
    DenyAlways,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    pub site: String,
    pub method: String,
    pub scope: Scope,
    pub created_at: u64,
}

/// Decision returned by the permission check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Auto-approve without prompting.
    Allow,
    /// Auto-deny without prompting.
    Deny,
    /// User must be prompted.
    NeedApproval,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PersistedPermissions {
    #[serde(default)]
    permissions: Vec<Permission>,
}

/// Persistent store backed by a JSON file on disk, plus an in-memory
/// session-scoped map for `AllowSession` approvals that don't survive a
/// lock or restart.
pub struct Permissions {
    data_dir: PathBuf,
    /// Persistent permissions keyed by (site, method). Only AllowAlways
    /// and DenyAlways live here.
    persisted: Mutex<HashMap<(String, String), Permission>>,
    /// Session permissions (AllowSession). Cleared when the signer is
    /// locked or the app restarts.
    session: Mutex<HashMap<(String, String), Permission>>,
}

impl Permissions {
    /// Load permissions from the data directory. Missing file is not an error.
    pub fn load(data_dir: PathBuf) -> Self {
        let path = permissions_path(&data_dir);
        let persisted = match std::fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str::<PersistedPermissions>(&json) {
                Ok(p) => p.permissions,
                Err(e) => {
                    tracing::warn!("failed to parse permissions.json: {e}");
                    vec![]
                }
            },
            Err(_) => vec![],
        };

        let mut map = HashMap::new();
        for p in persisted {
            map.insert((p.site.clone(), p.method.clone()), p);
        }

        Self {
            data_dir,
            persisted: Mutex::new(map),
            session: Mutex::new(HashMap::new()),
        }
    }

    /// Check whether a (site, method) pair has a stored decision.
    pub fn check(&self, site: &str, method: &str) -> Decision {
        // Non-sensitive methods are always allowed (dispatch still enforces
        // that the signer is unlocked).
        if !is_sensitive(method) {
            return Decision::Allow;
        }

        let key = (site.to_string(), method.to_string());

        // Check persisted permissions first
        if let Some(perm) = self.persisted.lock().unwrap().get(&key) {
            return match perm.scope {
                Scope::AllowAlways => Decision::Allow,
                Scope::DenyAlways => Decision::Deny,
                // These shouldn't be persisted, but handle defensively
                _ => Decision::NeedApproval,
            };
        }

        // Then session permissions
        if let Some(perm) = self.session.lock().unwrap().get(&key) {
            if perm.scope == Scope::AllowSession {
                return Decision::Allow;
            }
        }

        Decision::NeedApproval
    }

    /// Record a decision made by the user in the approval prompt.
    pub fn record(&self, site: &str, method: &str, scope: Scope) {
        let perm = Permission {
            site: site.to_string(),
            method: method.to_string(),
            scope,
            created_at: unix_timestamp(),
        };
        let key = (site.to_string(), method.to_string());

        match scope {
            Scope::AllowOnce => {
                // One-shot — nothing to store
            }
            Scope::AllowSession => {
                self.session.lock().unwrap().insert(key, perm);
            }
            Scope::AllowAlways | Scope::DenyAlways => {
                self.persisted.lock().unwrap().insert(key, perm);
                self.save_to_disk();
            }
        }
    }

    /// List all persisted permissions for UI display.
    pub fn list_persisted(&self) -> Vec<Permission> {
        self.persisted.lock().unwrap().values().cloned().collect()
    }

    /// Revoke a single persisted permission.
    pub fn revoke(&self, site: &str, method: &str) {
        let removed = self
            .persisted
            .lock()
            .unwrap()
            .remove(&(site.to_string(), method.to_string()))
            .is_some();
        if removed {
            self.save_to_disk();
        }
    }

    /// Revoke all permissions for a site.
    pub fn revoke_site(&self, site: &str) {
        let mut guard = self.persisted.lock().unwrap();
        let before = guard.len();
        guard.retain(|(s, _), _| s != site);
        let changed = guard.len() != before;
        drop(guard);
        if changed {
            self.save_to_disk();
        }
    }

    /// Clear all session permissions (called when the signer locks).
    pub fn clear_session(&self) {
        self.session.lock().unwrap().clear();
    }

    fn save_to_disk(&self) {
        let perms: Vec<Permission> = self.persisted.lock().unwrap().values().cloned().collect();
        let payload = PersistedPermissions { permissions: perms };
        let path = permissions_path(&self.data_dir);
        match serde_json::to_string_pretty(&payload) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!("failed to write permissions.json: {e}");
                }
            }
            Err(e) => tracing::warn!("failed to serialize permissions: {e}"),
        }
    }
}

fn permissions_path(data_dir: &Path) -> PathBuf {
    data_dir.join("permissions.json")
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "titan-perm-test-{}-{}-{}",
            std::process::id(),
            unix_timestamp(),
            n
        ));
        // Clean up any stale directory from a previous run before creating fresh
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sensitive_methods_list() {
        assert!(is_sensitive("signEvent"));
        assert!(is_sensitive("nip04.encrypt"));
        assert!(is_sensitive("nip04.decrypt"));
        assert!(is_sensitive("nip44.encrypt"));
        assert!(is_sensitive("nip44.decrypt"));
        assert!(!is_sensitive("getPublicKey"));
        assert!(!is_sensitive("getRelays"));
    }

    #[test]
    fn non_sensitive_always_allowed() {
        let perms = Permissions::load(temp_dir());
        assert_eq!(perms.check("titan", "getPublicKey"), Decision::Allow);
        assert_eq!(perms.check("titan", "getRelays"), Decision::Allow);
    }

    #[test]
    fn unknown_site_needs_approval() {
        let perms = Permissions::load(temp_dir());
        assert_eq!(perms.check("titan", "signEvent"), Decision::NeedApproval);
    }

    #[test]
    fn allow_always_persists_and_auto_approves() {
        let dir = temp_dir();
        {
            let perms = Permissions::load(dir.clone());
            perms.record("titan", "signEvent", Scope::AllowAlways);
            assert_eq!(perms.check("titan", "signEvent"), Decision::Allow);
        }
        // Reload from disk
        {
            let perms = Permissions::load(dir.clone());
            assert_eq!(perms.check("titan", "signEvent"), Decision::Allow);
        }
    }

    #[test]
    fn deny_always_persists_and_auto_denies() {
        let dir = temp_dir();
        {
            let perms = Permissions::load(dir.clone());
            perms.record("malicious", "signEvent", Scope::DenyAlways);
        }
        let perms = Permissions::load(dir);
        assert_eq!(perms.check("malicious", "signEvent"), Decision::Deny);
    }

    #[test]
    fn allow_session_does_not_persist() {
        let dir = temp_dir();
        {
            let perms = Permissions::load(dir.clone());
            perms.record("titan", "signEvent", Scope::AllowSession);
            assert_eq!(perms.check("titan", "signEvent"), Decision::Allow);
        }
        // Reload — session state is gone
        let perms = Permissions::load(dir);
        assert_eq!(perms.check("titan", "signEvent"), Decision::NeedApproval);
    }

    #[test]
    fn allow_once_does_not_persist_or_remember() {
        let perms = Permissions::load(temp_dir());
        perms.record("titan", "signEvent", Scope::AllowOnce);
        // AllowOnce is consumed immediately — the next check needs approval again
        assert_eq!(perms.check("titan", "signEvent"), Decision::NeedApproval);
    }

    #[test]
    fn clear_session_wipes_session_permissions() {
        let perms = Permissions::load(temp_dir());
        perms.record("titan", "signEvent", Scope::AllowSession);
        assert_eq!(perms.check("titan", "signEvent"), Decision::Allow);
        perms.clear_session();
        assert_eq!(perms.check("titan", "signEvent"), Decision::NeedApproval);
    }

    #[test]
    fn revoke_removes_permission() {
        let perms = Permissions::load(temp_dir());
        perms.record("titan", "signEvent", Scope::AllowAlways);
        assert_eq!(perms.check("titan", "signEvent"), Decision::Allow);
        perms.revoke("titan", "signEvent");
        assert_eq!(perms.check("titan", "signEvent"), Decision::NeedApproval);
    }

    #[test]
    fn revoke_site_removes_all_methods() {
        let perms = Permissions::load(temp_dir());
        perms.record("titan", "signEvent", Scope::AllowAlways);
        perms.record("titan", "nip44.encrypt", Scope::AllowAlways);
        perms.record("other", "signEvent", Scope::AllowAlways);
        perms.revoke_site("titan");
        assert_eq!(perms.check("titan", "signEvent"), Decision::NeedApproval);
        assert_eq!(perms.check("titan", "nip44.encrypt"), Decision::NeedApproval);
        assert_eq!(perms.check("other", "signEvent"), Decision::Allow);
    }

    #[test]
    fn list_persisted_excludes_session() {
        let perms = Permissions::load(temp_dir());
        perms.record("always", "signEvent", Scope::AllowAlways);
        perms.record("session", "signEvent", Scope::AllowSession);
        let list = perms.list_persisted();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].site, "always");
    }

    #[test]
    fn scopes_serialize_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&Scope::AllowAlways).unwrap(),
            "\"allow_always\""
        );
        assert_eq!(
            serde_json::to_string(&Scope::DenyAlways).unwrap(),
            "\"deny_always\""
        );
    }
}
