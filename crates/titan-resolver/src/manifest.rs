//! NIP-5A site manifest parsing and path resolution.
//!
//! A manifest is a Nostr event (kind 15128 or 35128) that maps file paths
//! to SHA256 blob hashes. Tags look like:
//!
//! ```json
//! ["path", "/index.html", "abc123...sha256hex"]
//! ["path", "/style.css", "def456...sha256hex"]
//! ```

use nostr_sdk::prelude::*;
use std::collections::HashMap;

/// A parsed nsite manifest — maps paths to blob hashes.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The pubkey of the site owner.
    pub pubkey: PublicKey,
    /// Unix timestamp of the manifest event.
    pub created_at: u64,
    /// Map of path → SHA256 hash (hex-encoded).
    pub files: HashMap<String, String>,
    /// Blossom servers listed in the manifest's "server" tags.
    pub servers: Vec<String>,
}

impl Manifest {
    /// Parse a Nostr event into a Manifest.
    pub fn from_event(event: &Event) -> Result<Self, ManifestError> {
        let kind = event.kind.as_u16();
        if kind != 15128 && kind != 35128 {
            return Err(ManifestError::WrongKind(kind));
        }

        let mut files = HashMap::new();
        let mut servers = Vec::new();

        for tag in event.tags.iter() {
            let values: Vec<&str> = tag.as_slice().iter().map(|s| s.as_str()).collect();

            match values.first().copied() {
                Some("path") if values.len() >= 3 => {
                    let path = values[1].to_string();
                    let hash = values[2].to_string();
                    files.insert(path, hash);
                }
                // Also handle "/" shorthand used by some nsite implementations
                Some("/") if values.len() >= 2 => {
                    let path = values[0].to_string();
                    let hash = values[1].to_string();
                    files.insert(path, hash);
                }
                Some("server") if values.len() >= 2 => {
                    servers.push(values[1].to_string());
                }
                _ => {}
            }
        }

        if files.is_empty() {
            return Err(ManifestError::Empty);
        }

        Ok(Self {
            pubkey: event.pubkey,
            created_at: event.created_at.as_u64(),
            files,
            servers,
        })
    }

    /// Resolve a request path to a blob SHA256 hash.
    ///
    /// Tries the exact path, then falls back to `/index.html` for directory paths.
    pub fn resolve_path(&self, path: &str) -> Option<&str> {
        // Normalize: ensure leading slash
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };

        // Exact match
        if let Some(hash) = self.files.get(&path) {
            return Some(hash.as_str());
        }

        // Directory: try appending index.html
        if path.ends_with('/') || !path.contains('.') {
            let index_path = if path.ends_with('/') {
                format!("{path}index.html")
            } else {
                format!("{path}/index.html")
            };
            if let Some(hash) = self.files.get(&index_path) {
                return Some(hash.as_str());
            }
        }

        // Root fallback
        if path == "/" {
            if let Some(hash) = self.files.get("/index.html") {
                return Some(hash.as_str());
            }
        }

        None
    }

    /// Build a manifest from nsite v1 per-file events (kind 34128).
    ///
    /// V1 model: each file is a separate event with:
    /// - `d` tag = file path (relative, no leading slash)
    /// - `x` or `sha256` tag = content SHA256 hash
    pub fn from_v1_events(events: &[Event]) -> Result<Self, ManifestError> {
        if events.is_empty() {
            return Err(ManifestError::Empty);
        }

        let mut files = HashMap::new();
        let mut servers = Vec::new();
        let mut pubkey = None;
        let mut latest_created_at: u64 = 0;

        for event in events {
            if pubkey.is_none() {
                pubkey = Some(event.pubkey);
            }
            if event.created_at.as_u64() > latest_created_at {
                latest_created_at = event.created_at.as_u64();
            }

            let tags: Vec<Vec<&str>> = event
                .tags
                .iter()
                .map(|t| t.as_slice().iter().map(|s| s.as_str()).collect())
                .collect();

            // Extract file path from d tag
            let path = tags.iter().find_map(|t| {
                if t.len() >= 2 && t[0] == "d" {
                    Some(t[1])
                } else {
                    None
                }
            });

            // Extract hash from x or sha256 tag
            let hash = tags.iter().find_map(|t| {
                if t.len() >= 2 && (t[0] == "x" || t[0] == "sha256") {
                    Some(t[1])
                } else {
                    None
                }
            });

            if let (Some(path), Some(hash)) = (path, hash) {
                // Normalize: ensure leading slash
                let normalized = if path.starts_with('/') {
                    path.to_string()
                } else {
                    format!("/{path}")
                };
                files.insert(normalized, hash.to_string());
            }

            // Collect server hints
            for t in &tags {
                if t.len() >= 2 && t[0] == "server" && !servers.contains(&t[1].to_string()) {
                    servers.push(t[1].to_string());
                }
            }
        }

        if files.is_empty() {
            return Err(ManifestError::Empty);
        }

        Ok(Self {
            pubkey: pubkey.unwrap(),
            created_at: latest_created_at,
            files,
            servers,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("wrong event kind: {0} (expected 15128 or 35128)")]
    WrongKind(u16),
    #[error("manifest has no file entries")]
    Empty,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_manifest_event(tags: Vec<Tag>) -> Event {
        let keys = Keys::generate();
        EventBuilder::new(Kind::Custom(15128), "")
            .tags(tags)
            .sign_with_keys(&keys)
            .unwrap()
    }

    fn path_tag(path: &str, hash: &str) -> Tag {
        Tag::parse(["path".to_string(), path.to_string(), hash.to_string()]).unwrap()
    }

    fn server_tag(url: &str) -> Tag {
        Tag::parse(["server".to_string(), url.to_string()]).unwrap()
    }

    #[test]
    fn parse_manifest() {
        let event = make_manifest_event(vec![
            path_tag("/index.html", "aabbccdd"),
            path_tag("/style.css", "11223344"),
            server_tag("https://blossom.example.com"),
        ]);

        let manifest = Manifest::from_event(&event).unwrap();
        assert_eq!(manifest.files.len(), 2);
        assert_eq!(manifest.files["/index.html"], "aabbccdd");
        assert_eq!(manifest.files["/style.css"], "11223344");
        assert_eq!(manifest.servers, vec!["https://blossom.example.com"]);
    }

    #[test]
    fn resolve_exact_path() {
        let event = make_manifest_event(vec![
            path_tag("/index.html", "aabb"),
            path_tag("/blog/post.html", "ccdd"),
        ]);
        let manifest = Manifest::from_event(&event).unwrap();

        assert_eq!(manifest.resolve_path("/blog/post.html"), Some("ccdd"));
    }

    #[test]
    fn resolve_root() {
        let event = make_manifest_event(vec![path_tag("/index.html", "aabb")]);
        let manifest = Manifest::from_event(&event).unwrap();

        assert_eq!(manifest.resolve_path("/"), Some("aabb"));
    }

    #[test]
    fn resolve_directory_index() {
        let event = make_manifest_event(vec![path_tag("/blog/index.html", "ccdd")]);
        let manifest = Manifest::from_event(&event).unwrap();

        assert_eq!(manifest.resolve_path("/blog"), Some("ccdd"));
        assert_eq!(manifest.resolve_path("/blog/"), Some("ccdd"));
    }

    #[test]
    fn resolve_not_found() {
        let event = make_manifest_event(vec![path_tag("/index.html", "aabb")]);
        let manifest = Manifest::from_event(&event).unwrap();

        assert_eq!(manifest.resolve_path("/nonexistent.html"), None);
    }

    #[test]
    fn reject_wrong_kind() {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::TextNote, "hello")
            .sign_with_keys(&keys)
            .unwrap();

        assert!(Manifest::from_event(&event).is_err());
    }

    #[test]
    fn reject_empty_manifest() {
        let event = make_manifest_event(vec![server_tag("https://example.com")]);
        assert!(Manifest::from_event(&event).is_err());
    }
}
