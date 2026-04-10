//! Bookmark store backed by a Titan-specific Nostr event kind (10129),
//! with a local JSON cache for offline use and cold-start responsiveness.
//!
//! ## Why a dedicated kind instead of NIP-51 kind 10003
//!
//! NIP-51 kind 10003 is spec'd for bookmarking Nostr *events* (`e` and
//! `a` tags). Titan bookmarks are URL references to a novel protocol
//! (`nsite://`), which doesn't fit cleanly. Mixing Titan entries into
//! kind 10003 would collide with generic NIP-51 readers that legitimately
//! expect only event references, and would leave us unable to filter
//! for Titan-specific lists on relays.
//!
//! Titan mints **kind 10129** (replaceable, one per author) as its own
//! bookmark kind. This follows the existing Titan convention of `xx129`
//! suffixes on Nostr kinds (see NSIT: 35129/15129/1129). See
//! `docs/titan-bookmarks.md` for the full protocol spec.
//!
//! ## Wire format
//!
//! ```jsonc
//! {
//!   "kind": 10129,
//!   "tags": [],                        // empty — everything is private
//!   "content": "<NIP-44 v2 ciphertext encrypted to self>"
//! }
//! ```
//!
//! The decrypted `.content` is a JSON array of bookmark rows:
//!
//! ```jsonc
//! [
//!   ["bookmark", "nsite://titan", "Titan", 1775829991],
//!   ["bookmark", "nsite://westernbtc", "Western BTC", 1775829992]
//! ]
//! ```
//!
//! Each row is `["bookmark", url, title, created_at]`:
//! - Row 0: literal `"bookmark"` type discriminator. Future row types
//!   (`"folder"`, `"separator"`, etc.) can be added without bumping the
//!   kind — unknown row types are skipped.
//! - Row 1: full URL with `nsite://` scheme (never stripped).
//! - Row 2: user-assigned title.
//! - Row 3: unix seconds when the bookmark was added, as an integer.
//!
//! ## Threat model
//!
//! Bookmarks are NIP-44 encrypted to the user's own pubkey, so anyone
//! who can read events from the user's relays sees opaque ciphertext.
//! Only the holder of the nsec can decrypt. Relays still see the event
//! metadata (kind, author, created_at, size) — they can tell *that*
//! a user has bookmarks and roughly how many, but not what they are.
//!
//! ## Local cache
//!
//! Every successful publish mirrors the plaintext list to
//! `data_dir/bookmarks.json` so the bookmarks bar paints instantly
//! at startup before the relay round-trip. The cache is also the
//! offline source of truth — if all relays are unreachable, the user
//! can still read and edit (edits get queued for the next publish).
//!
//! ## Migration from v0.1.4 (bare JSON array)
//!
//! v0.1.4 stored bookmarks as a plain JSON array in the same path. On
//! first launch of a version that understands the new format, an
//! unwrapped legacy array is upgraded transparently and the caller
//! is told to publish a fresh kind 10129 event (handled in main.rs).
//!
//! No backwards compat is maintained for the brief v0.1.5-dev window
//! where this file shipped as kind 10003 — that event (if published)
//! is abandoned on the relay and ages out naturally.

use crate::signer::Signer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::{debug, info, warn};

/// Titan Bookmarks event kind. Replaceable (one per author).
/// See `docs/titan-bookmarks.md` for the full protocol spec.
pub const BOOKMARKS_KIND: u16 = 10129;

/// Row type discriminator for a bookmark entry inside the encrypted
/// payload. Future row types (e.g. `"folder"`) can be added without
/// bumping the kind — unknown types are skipped on decode.
const ROW_TYPE_BOOKMARK: &str = "bookmark";

/// One bookmark entry. Mirrors the existing chrome.js shape so the UI
/// doesn't need any changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    pub created_at: u64,
}

/// Versioned wrapper persisted to `bookmarks.json`. The version field
/// lets future schema changes happen without breaking old installs —
/// older binaries will fail to parse, fall back to an empty list, and
/// the next publish from a newer binary will overwrite.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedBookmarks {
    /// Schema version. Always 1 today.
    version: u32,
    /// The plaintext bookmark list (mirror of what was last published
    /// to Nostr, or what's queued to be published).
    bookmarks: Vec<Bookmark>,
    /// Whether the local copy is ahead of the published event. Set
    /// when a write happens while the relay round-trip fails or when
    /// the signer is unavailable.
    #[serde(default)]
    pending_publish: bool,
}

impl PersistedBookmarks {
    fn new(bookmarks: Vec<Bookmark>) -> Self {
        Self {
            version: 1,
            bookmarks,
            pending_publish: false,
        }
    }
}

/// Result of loading the persisted bookmarks file. The variant tells the
/// caller whether a one-time migration publish should happen.
#[derive(Debug)]
pub enum LoadOutcome {
    /// Fresh install — no file on disk.
    Empty,
    /// New-format file. Already aware of the Nostr backend.
    NewFormat {
        bookmarks: Vec<Bookmark>,
        pending_publish: bool,
    },
    /// Legacy v0.1.4 format. Caller should publish to Nostr once and
    /// then write the wrapped format on disk.
    Legacy { bookmarks: Vec<Bookmark> },
}

/// Bookmark store. Wraps an in-memory Vec, the on-disk mirror, and
/// (when a signer is available and unlocked) the NIP-51 publish path.
pub struct BookmarkStore {
    data_dir: PathBuf,
    bookmarks: Mutex<Vec<Bookmark>>,
    /// Marks the in-memory copy as ahead of any successful relay
    /// publish. Set to true on every mutation, cleared after a
    /// successful `publish_to_nostr` call.
    pending_publish: Mutex<bool>,
}

impl BookmarkStore {
    /// Load bookmarks from `data_dir/bookmarks.json`. Returns the store
    /// and a `LoadOutcome` describing whether a migration is needed.
    pub fn load(data_dir: PathBuf) -> (Self, LoadOutcome) {
        let path = bookmarks_path(&data_dir);
        let outcome = match std::fs::read_to_string(&path) {
            Ok(json) => {
                // Try the new format first.
                if let Ok(p) = serde_json::from_str::<PersistedBookmarks>(&json) {
                    LoadOutcome::NewFormat {
                        bookmarks: p.bookmarks,
                        pending_publish: p.pending_publish,
                    }
                } else if let Ok(legacy) = serde_json::from_str::<Vec<Bookmark>>(&json) {
                    // Legacy v0.1.4 format — bare JSON array.
                    info!(
                        "loaded {} legacy bookmark(s) from {}, will migrate",
                        legacy.len(),
                        path.display()
                    );
                    LoadOutcome::Legacy { bookmarks: legacy }
                } else {
                    warn!(
                        "failed to parse bookmarks.json at {}, starting empty",
                        path.display()
                    );
                    LoadOutcome::Empty
                }
            }
            Err(_) => LoadOutcome::Empty,
        };

        let (bookmarks, pending) = match &outcome {
            LoadOutcome::Empty => (vec![], false),
            LoadOutcome::NewFormat {
                bookmarks,
                pending_publish,
            } => (bookmarks.clone(), *pending_publish),
            LoadOutcome::Legacy { bookmarks } => (bookmarks.clone(), true),
        };

        let store = Self {
            data_dir,
            bookmarks: Mutex::new(bookmarks),
            pending_publish: Mutex::new(pending),
        };
        (store, outcome)
    }

    /// Snapshot of the current bookmark list.
    pub fn list(&self) -> Vec<Bookmark> {
        self.bookmarks.lock().unwrap().clone()
    }

    /// Whether a URL is currently bookmarked.
    pub fn contains(&self, url: &str) -> bool {
        self.bookmarks.lock().unwrap().iter().any(|b| b.url == url)
    }

    /// Add a bookmark if not already present. Marks the store as
    /// pending publish. Returns true if a new entry was added.
    pub fn add(&self, url: String, title: String) -> bool {
        let mut guard = self.bookmarks.lock().unwrap();
        if guard.iter().any(|b| b.url == url) {
            return false;
        }
        guard.push(Bookmark {
            url,
            title,
            created_at: now_secs(),
        });
        let snapshot = guard.clone();
        drop(guard);
        self.mark_dirty_and_save(snapshot);
        true
    }

    /// Remove a bookmark by URL. Returns true if anything was removed.
    pub fn remove(&self, url: &str) -> bool {
        let mut guard = self.bookmarks.lock().unwrap();
        let before = guard.len();
        guard.retain(|b| b.url != url);
        let changed = guard.len() != before;
        if changed {
            let snapshot = guard.clone();
            drop(guard);
            self.mark_dirty_and_save(snapshot);
        }
        changed
    }

    /// Rename an existing bookmark. Returns true if the entry existed
    /// and the title actually changed.
    pub fn rename(&self, url: &str, title: String) -> bool {
        let mut guard = self.bookmarks.lock().unwrap();
        let entry = match guard.iter_mut().find(|b| b.url == url) {
            Some(b) => b,
            None => return false,
        };
        if entry.title == title {
            return false;
        }
        entry.title = title;
        let snapshot = guard.clone();
        drop(guard);
        self.mark_dirty_and_save(snapshot);
        true
    }

    /// Replace the in-memory list with one freshly fetched from Nostr.
    /// Used after `apply_remote` decrypts the latest kind 10003 event.
    /// Does NOT mark dirty (the remote IS the truth here).
    pub fn replace_from_remote(&self, bookmarks: Vec<Bookmark>) {
        *self.bookmarks.lock().unwrap() = bookmarks.clone();
        *self.pending_publish.lock().unwrap() = false;
        let payload = PersistedBookmarks::new(bookmarks);
        save_to_disk(&self.data_dir, &payload);
    }

    /// Whether the in-memory list has unpublished changes.
    /// Currently consumed by tests; a future "sync status" badge in the
    /// bookmarks panel will use this from the runtime path.
    #[allow(dead_code)]
    pub fn is_pending_publish(&self) -> bool {
        *self.pending_publish.lock().unwrap()
    }

    /// Mark the store as in sync with Nostr. Called after a successful
    /// publish. Also rewrites the disk mirror with `pending_publish: false`.
    pub fn mark_published(&self) {
        *self.pending_publish.lock().unwrap() = false;
        let snapshot = self.bookmarks.lock().unwrap().clone();
        let payload = PersistedBookmarks::new(snapshot);
        save_to_disk(&self.data_dir, &payload);
    }

    fn mark_dirty_and_save(&self, snapshot: Vec<Bookmark>) {
        *self.pending_publish.lock().unwrap() = true;
        let mut payload = PersistedBookmarks::new(snapshot);
        payload.pending_publish = true;
        save_to_disk(&self.data_dir, &payload);
    }

    /// Encode the current bookmark list as a Titan Bookmarks event
    /// (kind 10129). The bookmarks live as `["bookmark", url, title,
    /// created_at]` rows inside the encrypted `.content` field; the
    /// public tags array is empty (Titan defaults to private).
    ///
    /// Requires an unlocked signer. Returns the signed event ready to
    /// hand to `Resolver::publish_event`.
    pub fn build_event(&self, signer: &Signer) -> Result<nostr_sdk::Event, String> {
        let pubkey = signer
            .get_pubkey()
            .ok_or_else(|| "Signer is locked".to_string())?;
        let snapshot = self.bookmarks.lock().unwrap().clone();
        let plaintext = encode_bookmarks_payload(&snapshot);
        let ciphertext = signer.chrome_nip44_encrypt(&pubkey, &plaintext)?;
        // Public tags array stays empty — all entries are private.
        signer.chrome_sign_event(BOOKMARKS_KIND, ciphertext, vec![])
    }

    /// Decrypt and parse a Titan Bookmarks event (kind 10129) fetched
    /// from relays into a bookmark list. Used by the startup sync path.
    pub fn parse_remote_event(
        signer: &Signer,
        event: &nostr_sdk::Event,
    ) -> Result<Vec<Bookmark>, String> {
        if event.kind != nostr_sdk::Kind::Custom(BOOKMARKS_KIND) {
            return Err(format!(
                "expected kind {}, got {}",
                BOOKMARKS_KIND,
                event.kind.as_u16()
            ));
        }
        let author_hex = event.pubkey.to_hex();
        let plaintext = signer.chrome_nip44_decrypt(&author_hex, &event.content)?;
        decode_bookmarks_payload(&plaintext)
    }
}

fn bookmarks_path(data_dir: &Path) -> PathBuf {
    data_dir.join("bookmarks.json")
}

fn save_to_disk(data_dir: &Path, payload: &PersistedBookmarks) {
    let path = bookmarks_path(data_dir);
    match serde_json::to_string_pretty(payload) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                warn!("failed to write {}: {e}", path.display());
            }
        }
        Err(e) => warn!("failed to serialize bookmarks: {e}"),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Normalize a bookmark URL to always include the `nsite://` scheme.
/// Titan's internal state historically stores URLs scheme-stripped
/// (e.g. `"titan"` instead of `"nsite://titan"`), so we add the scheme
/// at serialization time to keep the wire format self-describing.
fn normalize_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("nsite://") {
        trimmed.to_string()
    } else {
        format!("nsite://{trimmed}")
    }
}

/// Strip the `nsite://` scheme from an incoming URL so internal state
/// stays in the legacy host-only form. This keeps the rest of the
/// codebase (navigate(), toggle_bookmark, display_url matching, etc.)
/// working unchanged.
fn denormalize_url(raw: &str) -> String {
    raw.strip_prefix("nsite://").unwrap_or(raw).to_string()
}

/// Serialize the bookmark list as the JSON array shape that gets
/// NIP-44 encrypted into the kind 10129 event `.content`. Each entry
/// is a row of the form: `["bookmark", url, title, created_at]`.
///
/// The row uses mixed types (string discriminator, string URL, string
/// title, integer timestamp) which is why this is `Vec<Value>` rather
/// than `Vec<Vec<String>>`. The type discriminator lets future row
/// kinds (`"folder"`, `"separator"`, etc.) coexist without bumping
/// the event kind.
fn encode_bookmarks_payload(bookmarks: &[Bookmark]) -> String {
    let rows: Vec<Value> = bookmarks
        .iter()
        .map(|b| {
            Value::Array(vec![
                Value::String(ROW_TYPE_BOOKMARK.to_string()),
                Value::String(normalize_url(&b.url)),
                Value::String(b.title.clone()),
                Value::Number(b.created_at.into()),
            ])
        })
        .collect();
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
}

/// Inverse of `encode_bookmarks_payload`. Tolerant of unknown row
/// types (skips anything that isn't a `"bookmark"` row) so future
/// schema extensions don't break older readers.
fn decode_bookmarks_payload(json: &str) -> Result<Vec<Bookmark>, String> {
    let rows: Vec<Value> = serde_json::from_str(json)
        .map_err(|e| format!("invalid bookmarks payload: {e}"))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let arr = match row.as_array() {
            Some(a) => a,
            None => {
                debug!("skipping non-array bookmark row: {row:?}");
                continue;
            }
        };
        // Expected shape: ["bookmark", url, title, created_at]
        if arr.len() < 4 {
            debug!("skipping short bookmark row: {arr:?}");
            continue;
        }
        if arr[0].as_str() != Some(ROW_TYPE_BOOKMARK) {
            debug!("skipping unknown row type: {:?}", arr[0]);
            continue;
        }
        let url = match arr[1].as_str() {
            Some(s) => denormalize_url(s),
            None => {
                debug!("bookmark row url not a string: {arr:?}");
                continue;
            }
        };
        let title = arr[2].as_str().unwrap_or("").to_string();
        // created_at is an integer in the new format, but be lenient
        // about stringified numbers in case a future row variant
        // passes the value as a string.
        let created_at = arr[3]
            .as_u64()
            .or_else(|| arr[3].as_str().and_then(|s| s.parse::<u64>().ok()))
            .unwrap_or(0);
        out.push(Bookmark {
            url,
            title,
            created_at,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "titan-bookmarks-test-{}-{}-{}",
            std::process::id(),
            now_secs(),
            n
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(url: &str, title: &str) -> Bookmark {
        Bookmark {
            url: url.to_string(),
            title: title.to_string(),
            created_at: 1_700_000_000,
        }
    }

    #[test]
    fn empty_load_outcome() {
        let (store, outcome) = BookmarkStore::load(temp_dir());
        assert!(matches!(outcome, LoadOutcome::Empty));
        assert!(store.list().is_empty());
        assert!(!store.is_pending_publish());
    }

    #[test]
    fn legacy_format_load_triggers_migration() {
        let dir = temp_dir();
        // Write the v0.1.4 format: a bare JSON array.
        let legacy = vec![sample("nsite://titan", "Titan")];
        std::fs::write(
            bookmarks_path(&dir),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();

        let (store, outcome) = BookmarkStore::load(dir);
        match outcome {
            LoadOutcome::Legacy { bookmarks } => assert_eq!(bookmarks.len(), 1),
            _ => panic!("expected legacy outcome"),
        }
        // Legacy load is "dirty" because it hasn't been published yet
        assert!(store.is_pending_publish());
    }

    #[test]
    fn new_format_round_trip() {
        let dir = temp_dir();
        let (store, _) = BookmarkStore::load(dir.clone());
        store.add("nsite://titan".to_string(), "Titan".to_string());
        store.add("nsite://westernbtc".to_string(), "WBTC".to_string());

        // Reload from disk — should come back as new-format
        let (store2, outcome) = BookmarkStore::load(dir);
        assert!(matches!(outcome, LoadOutcome::NewFormat { .. }));
        assert_eq!(store2.list().len(), 2);
        // Pending stayed true because nothing was marked published
        assert!(store2.is_pending_publish());
    }

    #[test]
    fn add_dedupes_by_url() {
        let (store, _) = BookmarkStore::load(temp_dir());
        assert!(store.add("nsite://titan".to_string(), "Titan".to_string()));
        // Second add with same URL is a no-op even with a different title
        assert!(!store.add("nsite://titan".to_string(), "Other".to_string()));
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn remove_returns_false_for_unknown() {
        let (store, _) = BookmarkStore::load(temp_dir());
        store.add("nsite://titan".to_string(), "Titan".to_string());
        assert!(!store.remove("nsite://nope"));
        assert!(store.remove("nsite://titan"));
        assert!(store.list().is_empty());
    }

    #[test]
    fn rename_only_marks_dirty_when_title_changes() {
        let (store, _) = BookmarkStore::load(temp_dir());
        store.add("nsite://titan".to_string(), "Titan".to_string());
        store.mark_published();
        assert!(!store.is_pending_publish());

        // No-op rename — title is identical
        assert!(!store.rename("nsite://titan", "Titan".to_string()));
        assert!(!store.is_pending_publish());

        // Real rename — flips the dirty bit
        assert!(store.rename("nsite://titan", "Titan Browser".to_string()));
        assert!(store.is_pending_publish());
    }

    #[test]
    fn mark_published_clears_pending() {
        let (store, _) = BookmarkStore::load(temp_dir());
        store.add("nsite://titan".to_string(), "Titan".to_string());
        assert!(store.is_pending_publish());
        store.mark_published();
        assert!(!store.is_pending_publish());
    }

    #[test]
    fn replace_from_remote_overwrites_local() {
        let (store, _) = BookmarkStore::load(temp_dir());
        store.add("nsite://local".to_string(), "Local".to_string());
        let remote = vec![
            sample("nsite://titan", "Titan"),
            sample("nsite://westernbtc", "WBTC"),
        ];
        store.replace_from_remote(remote);
        assert_eq!(store.list().len(), 2);
        assert!(!store.contains("nsite://local"));
        // Remote is the truth — pending should be cleared
        assert!(!store.is_pending_publish());
    }

    #[test]
    fn encode_decode_round_trip() {
        // Internal URLs stay scheme-stripped (legacy host form). The
        // encoder normalizes to `nsite://` on the wire and the decoder
        // strips it back — so the round trip should be lossless.
        let bookmarks = vec![
            Bookmark {
                url: "titan".to_string(),
                title: "Titan".to_string(),
                created_at: 1_700_000_000,
            },
            Bookmark {
                url: "westernbtc/page".to_string(),
                title: "WBTC home".to_string(),
                created_at: 1_700_000_500,
            },
        ];
        let payload = encode_bookmarks_payload(&bookmarks);
        let decoded = decode_bookmarks_payload(&payload).unwrap();
        assert_eq!(decoded, bookmarks);
    }

    #[test]
    fn encode_writes_full_nsite_scheme_on_wire() {
        // The on-wire payload must include the `nsite://` prefix even
        // though internal state stores the scheme-stripped form.
        // Regression test for the normalization layer.
        let bookmarks = vec![Bookmark {
            url: "titan".to_string(),
            title: "Titan".to_string(),
            created_at: 1_700_000_000,
        }];
        let payload = encode_bookmarks_payload(&bookmarks);
        assert!(
            payload.contains("nsite://titan"),
            "payload should carry full scheme: {payload}"
        );
        // And the created_at must be an integer, not a stringified
        // number — prevents a regression to the old stringy format.
        assert!(
            payload.contains("1700000000"),
            "payload should carry int timestamp: {payload}"
        );
        assert!(
            !payload.contains("\"1700000000\""),
            "payload should NOT stringify the timestamp: {payload}"
        );
    }

    #[test]
    fn encode_preserves_already_normalized_url() {
        // If a caller happens to pass a bookmark whose URL already
        // starts with `nsite://`, we should not double-prefix it.
        let bookmarks = vec![Bookmark {
            url: "nsite://titan".to_string(),
            title: "Titan".to_string(),
            created_at: 1_700_000_000,
        }];
        let payload = encode_bookmarks_payload(&bookmarks);
        assert!(payload.contains("nsite://titan"));
        assert!(!payload.contains("nsite://nsite://"));
    }

    #[test]
    fn decode_skips_unknown_row_types() {
        // Mix of valid bookmark rows and future row types the decoder
        // doesn't understand yet. Unknown types should be skipped,
        // valid rows should come through. This is the forward-compat
        // contract — future Titan versions can add row types without
        // breaking current readers.
        let payload = r#"[
            ["folder","Work",1700000000],
            ["bookmark","nsite://titan","Titan",1700000000],
            ["separator"],
            ["bookmark","nsite://wbtc","WBTC",1700000200],
            ["unknown","foo","bar"]
        ]"#;
        let decoded = decode_bookmarks_payload(payload).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].url, "titan");
        assert_eq!(decoded[1].url, "wbtc");
    }

    #[test]
    fn decode_skips_short_rows() {
        // A bookmark row with fewer than 4 elements is malformed
        // and must be skipped, not crash the decoder.
        let payload = r#"[
            ["bookmark"],
            ["bookmark","nsite://titan"],
            ["bookmark","nsite://valid","Valid",1700000000]
        ]"#;
        let decoded = decode_bookmarks_payload(payload).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].url, "valid");
    }

    #[test]
    fn decode_tolerates_stringified_timestamp() {
        // Defensive: if a future row type passes the timestamp as a
        // string, the decoder should still parse it. Protects us from
        // a careless encoder change.
        let payload = r#"[["bookmark","nsite://titan","Titan","1700000000"]]"#;
        let decoded = decode_bookmarks_payload(payload).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].created_at, 1_700_000_000);
    }

    #[test]
    fn decode_invalid_json_errors() {
        let err = decode_bookmarks_payload("not json").unwrap_err();
        assert!(err.contains("invalid bookmarks payload"));
    }

    #[test]
    fn decode_skips_non_array_rows() {
        let payload = r#"["not-an-array",{"also":"not"},["bookmark","nsite://titan","Titan",1700000000]]"#;
        let decoded = decode_bookmarks_payload(payload).unwrap();
        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn normalize_url_adds_scheme() {
        assert_eq!(normalize_url("titan"), "nsite://titan");
        assert_eq!(normalize_url("nsite://titan"), "nsite://titan");
        // Whitespace should be trimmed but scheme added
        assert_eq!(normalize_url("  titan  "), "nsite://titan");
    }

    #[test]
    fn denormalize_url_strips_scheme() {
        assert_eq!(denormalize_url("nsite://titan"), "titan");
        assert_eq!(denormalize_url("titan"), "titan");
        assert_eq!(denormalize_url("nsite://westernbtc/page"), "westernbtc/page");
    }

    #[test]
    fn build_event_uses_kind_10129() {
        // Lock the wire kind down so a careless change can't silently
        // start publishing under a different kind number.
        assert_eq!(BOOKMARKS_KIND, 10129);
    }

    #[test]
    fn build_event_and_parse_round_trip_with_signer() {
        // End-to-end: BookmarkStore.build_event() encrypts to the user,
        // BookmarkStore::parse_remote_event() decrypts it back.
        use nostr_sdk::prelude::Keys;

        // Build a test signer manually (avoids touching keychain)
        let test_hex = "1111111111111111111111111111111111111111111111111111111111111111";
        let keys = Keys::parse(test_hex).expect("test key parses");
        let signer = crate::signer::Signer::__test_unlocked(keys);

        let (store, _) = BookmarkStore::load(temp_dir());
        store.add("titan".to_string(), "Titan".to_string());
        store.add("wbtc".to_string(), "WBTC".to_string());

        let event = store.build_event(&signer).expect("build event");
        assert_eq!(event.kind, nostr_sdk::Kind::Custom(BOOKMARKS_KIND));
        // Public tags are empty — privacy-by-default
        assert!(event.tags.is_empty());

        let decoded = BookmarkStore::parse_remote_event(&signer, &event).expect("decode");
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].url, "titan");
        assert_eq!(decoded[0].title, "Titan");
        assert_eq!(decoded[1].url, "wbtc");
    }

    #[test]
    fn parse_remote_event_rejects_wrong_kind() {
        use nostr_sdk::prelude::Keys;
        let test_hex = "1111111111111111111111111111111111111111111111111111111111111111";
        let keys = Keys::parse(test_hex).unwrap();
        let signer = crate::signer::Signer::__test_unlocked(keys);

        // Sign a kind 1 event and try to parse it as a bookmark event.
        let wrong = signer
            .chrome_sign_event(1, "not bookmarks".to_string(), vec![])
            .expect("sign");
        let err = BookmarkStore::parse_remote_event(&signer, &wrong).unwrap_err();
        assert!(err.contains("expected kind 10129"));
    }
}
