//! Nostr relay interaction and Blossom blob fetching for Titan.
//!
//! Resolution flow:
//!
//! **Bitcoin name** (`nsite://westernbtc`):
//!   name → pubkey (SQLite) → relay list (kind 10002) → manifest (kind 35128, d=name)
//!   → path → blob (Blossom, SHA256-verified) → render
//!
//! **Direct npub** (`nsite://npub1...`):
//!   pubkey → relay list (kind 10002) → manifest (kind 15128, root)
//!   → path → blob (Blossom, SHA256-verified) → render
//!
//! The address type determines the manifest kind: names use kind 35128 (addressable,
//! d-tag = registered name), npubs use kind 15128 (root, one per pubkey).
//! All steps are cached to disk with appropriate TTLs.

pub mod blossom;
pub mod cache;
pub mod manifest;
pub mod relay;

use blossom::BlossomClient;
use cache::DiskCache;
use manifest::Manifest;
use nostr_sdk::prelude::*;
use relay::RelayPool;
use std::path::PathBuf;
use tracing::{debug, info};

/// Hardcoded fallback relays.
pub const FALLBACK_RELAYS: &[&str] = &[
    "wss://relay.westernbtc.com",
    "wss://relay.primal.net",
    "wss://relay.damus.io",
];

/// Fallback Blossom servers for blob fetching.
pub const FALLBACK_BLOSSOM_SERVERS: &[&str] = &[
    "https://blossom.westernbtc.com",
    "https://nostr.build",
];

/// Resolved content ready for rendering.
pub struct ResolvedContent {
    /// The raw bytes of the resolved file.
    pub data: Vec<u8>,
    /// The SHA256 hash of the content.
    pub hash: String,
    /// The path that was resolved.
    pub path: String,
}

/// Top-level resolver that orchestrates the full nsite resolution flow.
pub struct Resolver {
    relays: RelayPool,
    blossom: BlossomClient,
    cache: DiskCache,
}

/// Errors from the resolution flow.
#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    #[error("relay error: {0}")]
    Relay(#[from] relay::RelayError),
    #[error("manifest error: {0}")]
    Manifest(#[from] manifest::ManifestError),
    #[error("blossom error: {0}")]
    Blossom(#[from] blossom::BlossomError),
    #[error("cache error: {0}")]
    Cache(#[from] cache::CacheError),
    #[error("manifest not found for pubkey {0}")]
    ManifestNotFound(String),
    #[error("path not found in manifest: {0}")]
    PathNotFound(String),
}

impl Resolver {
    /// Create a new resolver, connecting to fallback relays and setting up the cache.
    pub async fn new(cache_dir: PathBuf) -> Result<Self, ResolverError> {
        let relays = RelayPool::connect(FALLBACK_RELAYS).await?;
        let blossom = BlossomClient::new();
        let cache = DiskCache::new(cache_dir)?;
        Ok(Self {
            relays,
            blossom,
            cache,
        })
    }

    /// Resolve a pubkey + path to content bytes.
    ///
    /// `site_name` controls which manifest kind is queried:
    /// - `Some("westernbtc")` → kind 35128, d-tag = "westernbtc" (Bitcoin name)
    /// - `None` → kind 15128 (direct npub, root manifest)
    ///
    /// This is the main entry point for the Tauri protocol handler.
    pub async fn resolve(
        &self,
        pubkey: &[u8; 32],
        path: &str,
        site_name: Option<&str>,
    ) -> Result<ResolvedContent, ResolverError> {
        let pubkey_hex = hex::encode(pubkey);
        let public_key = PublicKey::from_slice(pubkey)
            .map_err(|e| relay::RelayError::Fetch(e.to_string()))?;

        // Step 1: Get the manifest (cached or fresh)
        let manifest = self.get_manifest(&public_key, &pubkey_hex, site_name).await?;

        // Step 2: Resolve the path to a blob hash
        let hash = manifest
            .resolve_path(path)
            .ok_or_else(|| ResolverError::PathNotFound(path.to_string()))?
            .to_string();

        // Step 3: Fetch the blob (cached or fresh)
        let data = self
            .get_blob(&hash, &public_key, &pubkey_hex, &manifest)
            .await?;

        Ok(ResolvedContent {
            data,
            hash,
            path: path.to_string(),
        })
    }

    /// Get or fetch a manifest for a pubkey.
    async fn get_manifest(
        &self,
        pubkey: &PublicKey,
        pubkey_hex: &str,
        site_name: Option<&str>,
    ) -> Result<Manifest, ResolverError> {
        // Cache key includes site name for addressable manifests
        let cache_key = match site_name {
            Some(name) => format!("{pubkey_hex}:{name}"),
            None => pubkey_hex.to_string(),
        };

        // Check cache
        if let Some(cached) = self.cache.get_manifest(&cache_key) {
            if let Ok(event) = serde_json::from_slice::<Event>(&cached) {
                if let Ok(manifest) = Manifest::from_event(&event) {
                    debug!("using cached manifest for {cache_key}");
                    return Ok(manifest);
                }
            }
        }

        // Discover pubkey's relays and add them to the pool
        let relay_urls = self.get_relay_list(pubkey, pubkey_hex).await;
        if !relay_urls.is_empty() {
            self.relays.add_relays(&relay_urls).await;
        }

        // Fetch manifest from relays
        let event = self
            .relays
            .fetch_manifest(pubkey, site_name)
            .await?
            .ok_or_else(|| ResolverError::ManifestNotFound(pubkey_hex.to_string()))?;

        // Cache the event
        if let Ok(json) = serde_json::to_vec(&event) {
            let _ = self.cache.put_manifest(&cache_key, &json);
        }

        let manifest = Manifest::from_event(&event)?;
        info!(
            "fetched manifest for {pubkey_hex}: {} file(s)",
            manifest.files.len()
        );
        Ok(manifest)
    }

    /// Get or fetch the relay list for a pubkey.
    async fn get_relay_list(&self, pubkey: &PublicKey, pubkey_hex: &str) -> Vec<String> {
        // Check cache
        if let Some(cached) = self.cache.get_relay_list(pubkey_hex) {
            if let Ok(urls) = serde_json::from_slice::<Vec<String>>(&cached) {
                if !urls.is_empty() {
                    debug!("using cached relay list for {pubkey_hex}");
                    return urls;
                }
            }
        }

        // Fetch from relays
        let urls = self.relays.fetch_relay_list(pubkey).await.unwrap_or_default();

        // Cache
        if let Ok(json) = serde_json::to_vec(&urls) {
            let _ = self.cache.put_relay_list(pubkey_hex, &json);
        }

        urls
    }

    /// Get or fetch a blob by hash.
    async fn get_blob(
        &self,
        hash: &str,
        pubkey: &PublicKey,
        pubkey_hex: &str,
        manifest: &Manifest,
    ) -> Result<Vec<u8>, ResolverError> {
        // Check cache — blobs never expire
        if let Some(cached) = self.cache.get_blob(hash) {
            debug!("using cached blob {hash}");
            return Ok(cached);
        }

        // Build server list: manifest servers → pubkey's blossom list → fallbacks
        let mut servers: Vec<String> = manifest.servers.clone();

        // Add pubkey's blossom server list
        let blossom_servers = self.get_blossom_list(pubkey, pubkey_hex).await;
        for s in blossom_servers {
            if !servers.contains(&s) {
                servers.push(s);
            }
        }

        // Add fallbacks
        for s in FALLBACK_BLOSSOM_SERVERS {
            let s = s.to_string();
            if !servers.contains(&s) {
                servers.push(s);
            }
        }

        // Fetch and verify
        let data = self.blossom.fetch_blob(hash, &servers).await?;

        // Cache
        let _ = self.cache.put_blob(hash, &data);

        Ok(data)
    }

    /// Get or fetch the Blossom server list for a pubkey.
    async fn get_blossom_list(&self, pubkey: &PublicKey, pubkey_hex: &str) -> Vec<String> {
        // Check cache
        if let Some(cached) = self.cache.get_blossom_list(pubkey_hex) {
            if let Ok(urls) = serde_json::from_slice::<Vec<String>>(&cached) {
                if !urls.is_empty() {
                    debug!("using cached blossom list for {pubkey_hex}");
                    return urls;
                }
            }
        }

        let urls = self
            .relays
            .fetch_blossom_servers(pubkey)
            .await
            .unwrap_or_default();

        if let Ok(json) = serde_json::to_vec(&urls) {
            let _ = self.cache.put_blossom_list(pubkey_hex, &json);
        }

        urls
    }

    /// Shut down relay connections (takes ownership).
    pub async fn shutdown(self) -> Result<(), ResolverError> {
        self.relays.shutdown().await?;
        Ok(())
    }

    /// Disconnect relay connections (borrow-friendly).
    pub async fn disconnect(&self) -> Result<(), ResolverError> {
        self.relays.disconnect().await?;
        Ok(())
    }
}
