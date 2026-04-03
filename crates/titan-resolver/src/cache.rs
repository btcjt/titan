//! Disk cache for resolved content.
//!
//! ## Cache strategy (from CLAUDE.md)
//!
//! - Manifests (kind 15128/35128): 5 min TTL
//! - Relay lists (kind 10002): 1 hour TTL
//! - Blobs: forever (content-addressed, immutable)
//! - Blossom server lists (kind 10063): 1 hour TTL

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tracing::debug;

/// TTL for cached relay lists (kind 10002).
pub const RELAY_LIST_TTL: Duration = Duration::from_secs(60 * 60); // 1 hour

/// TTL for cached manifests (kind 15128/35128).
pub const MANIFEST_TTL: Duration = Duration::from_secs(5 * 60); // 5 minutes

/// TTL for cached Blossom server lists (kind 10063).
pub const BLOSSOM_LIST_TTL: Duration = Duration::from_secs(60 * 60); // 1 hour

/// Disk-based content cache.
pub struct DiskCache {
    base_dir: PathBuf,
}

impl DiskCache {
    /// Create a new disk cache at the given directory.
    /// Creates the directory structure if it doesn't exist.
    pub fn new(base_dir: PathBuf) -> Result<Self, CacheError> {
        fs::create_dir_all(base_dir.join("blobs")).map_err(CacheError::Io)?;
        fs::create_dir_all(base_dir.join("manifests")).map_err(CacheError::Io)?;
        fs::create_dir_all(base_dir.join("relay-lists")).map_err(CacheError::Io)?;
        fs::create_dir_all(base_dir.join("blossom-lists")).map_err(CacheError::Io)?;
        Ok(Self { base_dir })
    }

    /// Get a cached blob by SHA256 hash. Blobs never expire.
    pub fn get_blob(&self, hash: &str) -> Option<Vec<u8>> {
        let path = self.base_dir.join("blobs").join(hash);
        read_if_exists(&path)
    }

    /// Store a blob. The filename is the SHA256 hash.
    pub fn put_blob(&self, hash: &str, data: &[u8]) -> Result<(), CacheError> {
        let path = self.base_dir.join("blobs").join(hash);
        fs::write(&path, data).map_err(CacheError::Io)?;
        debug!("cached blob {hash} ({} bytes)", data.len());
        Ok(())
    }

    /// Get a cached manifest for a pubkey (hex). Returns `None` if expired or absent.
    pub fn get_manifest(&self, pubkey_hex: &str) -> Option<Vec<u8>> {
        let path = self.base_dir.join("manifests").join(pubkey_hex);
        read_if_fresh(&path, MANIFEST_TTL)
    }

    /// Store a manifest event (serialized JSON) keyed by pubkey hex.
    pub fn put_manifest(&self, pubkey_hex: &str, data: &[u8]) -> Result<(), CacheError> {
        let path = self.base_dir.join("manifests").join(pubkey_hex);
        fs::write(&path, data).map_err(CacheError::Io)?;
        debug!("cached manifest for {pubkey_hex}");
        Ok(())
    }

    /// Get a cached relay list for a pubkey (hex). Returns `None` if expired or absent.
    pub fn get_relay_list(&self, pubkey_hex: &str) -> Option<Vec<u8>> {
        let path = self.base_dir.join("relay-lists").join(pubkey_hex);
        read_if_fresh(&path, RELAY_LIST_TTL)
    }

    /// Store a relay list (serialized JSON) keyed by pubkey hex.
    pub fn put_relay_list(&self, pubkey_hex: &str, data: &[u8]) -> Result<(), CacheError> {
        let path = self.base_dir.join("relay-lists").join(pubkey_hex);
        fs::write(&path, data).map_err(CacheError::Io)?;
        debug!("cached relay list for {pubkey_hex}");
        Ok(())
    }

    /// Get a cached Blossom server list for a pubkey (hex).
    pub fn get_blossom_list(&self, pubkey_hex: &str) -> Option<Vec<u8>> {
        let path = self.base_dir.join("blossom-lists").join(pubkey_hex);
        read_if_fresh(&path, BLOSSOM_LIST_TTL)
    }

    /// Store a Blossom server list keyed by pubkey hex.
    pub fn put_blossom_list(&self, pubkey_hex: &str, data: &[u8]) -> Result<(), CacheError> {
        let path = self.base_dir.join("blossom-lists").join(pubkey_hex);
        fs::write(&path, data).map_err(CacheError::Io)?;
        debug!("cached blossom list for {pubkey_hex}");
        Ok(())
    }
}

/// Read a file if it exists, regardless of age.
fn read_if_exists(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

/// Read a file if it exists and was modified within the given TTL.
fn read_if_fresh(path: &Path, ttl: Duration) -> Option<Vec<u8>> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;

    if age > ttl {
        debug!("cache expired: {}", path.display());
        return None;
    }

    fs::read(path).ok()
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blob_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::new(dir.path().to_path_buf()).unwrap();

        assert!(cache.get_blob("abc123").is_none());

        cache.put_blob("abc123", b"hello world").unwrap();
        let data = cache.get_blob("abc123").unwrap();
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn manifest_cache_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::new(dir.path().to_path_buf()).unwrap();

        cache.put_manifest("pubkey1", b"manifest data").unwrap();
        assert!(cache.get_manifest("pubkey1").is_some());

        // Blobs don't expire, but manifests do — we can't easily test TTL expiry
        // in a unit test without mocking time, so we just verify the roundtrip.
    }

    #[test]
    fn relay_list_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::new(dir.path().to_path_buf()).unwrap();

        cache.put_relay_list("pubkey1", b"relay data").unwrap();
        let data = cache.get_relay_list("pubkey1").unwrap();
        assert_eq!(data, b"relay data");
    }

    #[test]
    fn blossom_list_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::new(dir.path().to_path_buf()).unwrap();

        cache.put_blossom_list("pubkey1", b"blossom data").unwrap();
        let data = cache.get_blossom_list("pubkey1").unwrap();
        assert_eq!(data, b"blossom data");
    }

    #[test]
    fn missing_cache_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::new(dir.path().to_path_buf()).unwrap();

        assert!(cache.get_blob("nonexistent").is_none());
        assert!(cache.get_manifest("nonexistent").is_none());
        assert!(cache.get_relay_list("nonexistent").is_none());
        assert!(cache.get_blossom_list("nonexistent").is_none());
    }
}
