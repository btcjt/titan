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
];

/// Default NSIT indexer pubkey (signs kind 35129/15129 events).
pub const INDEXER_PUBKEY_HEX: &str =
    "bec1a370130fed4fb9f78f9efc725b35104d827470e75573558a87a9ac5cde44";

/// Default discovery relays.
pub const DEFAULT_DISCOVERY_RELAYS: &[&str] = &[
    "wss://purplepag.es",
    "wss://user.kindpag.es",
];

/// Configuration for the resolver.
#[derive(Debug, Clone)]
pub struct ResolverConfig {
    pub relays: Vec<String>,
    pub discovery_relays: Vec<String>,
    pub blossom_servers: Vec<String>,
    pub indexer_pubkey: String,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            relays: FALLBACK_RELAYS.iter().map(|s| s.to_string()).collect(),
            discovery_relays: DEFAULT_DISCOVERY_RELAYS.iter().map(|s| s.to_string()).collect(),
            blossom_servers: FALLBACK_BLOSSOM_SERVERS.iter().map(|s| s.to_string()).collect(),
            indexer_pubkey: INDEXER_PUBKEY_HEX.to_string(),
        }
    }
}

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
    config: ResolverConfig,
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
    /// Create a new resolver with default config.
    pub async fn new(cache_dir: PathBuf) -> Result<Self, ResolverError> {
        Self::new_with_config(cache_dir, ResolverConfig::default()).await
    }

    /// Create a new resolver with custom config.
    pub async fn new_with_config(cache_dir: PathBuf, config: ResolverConfig) -> Result<Self, ResolverError> {
        let relay_strs: Vec<&str> = config.relays.iter().map(|s| s.as_str()).collect();
        let discovery_strs: Vec<&str> = config.discovery_relays.iter().map(|s| s.as_str()).collect();
        let relays = RelayPool::connect_with_discovery(&relay_strs, &discovery_strs).await?;
        let blossom = BlossomClient::new();
        let cache = DiskCache::new(cache_dir)?;
        Ok(Self {
            relays,
            blossom,
            cache,
            config,
        })
    }

    /// Look up a Bitcoin name via the Nostr-published NSIT index (kind 35129).
    /// Uses race-then-linger for fast results from the first relay that responds.
    /// Returns the 32-byte pubkey if the name is found, or None.
    pub async fn lookup_name(&self, name: &str) -> Result<Option<[u8; 32]>, ResolverError> {
        let indexer_pubkey = PublicKey::from_hex(&self.config.indexer_pubkey)
            .map_err(|e| relay::RelayError::Fetch(e.to_string()))?;

        let record = self.relays.lookup_nsit_name(name, &indexer_pubkey).await?;

        match record {
            Some(r) => {
                let bytes = hex::decode(&r.pubkey_hex)
                    .map_err(|e| ResolverError::ManifestNotFound(e.to_string()))?;
                if bytes.len() != 32 {
                    return Err(ResolverError::ManifestNotFound(
                        "invalid pubkey length in NSIT record".to_string(),
                    ));
                }
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&bytes);
                Ok(Some(pk))
            }
            None => Ok(None),
        }
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
        let t0 = std::time::Instant::now();
        let manifest = self.get_manifest(&public_key, &pubkey_hex, site_name).await?;
        debug!("manifest fetched in {:?}", t0.elapsed());

        // Step 2: Resolve the path to a blob hash
        let hash = manifest
            .resolve_path(path)
            .ok_or_else(|| ResolverError::PathNotFound(path.to_string()))?
            .to_string();

        // Step 3: Fetch the blob (cached or fresh)
        let t1 = std::time::Instant::now();
        let data = self
            .get_blob(&hash, &public_key, &pubkey_hex, &manifest)
            .await?;
        debug!("blob fetched in {:?} (hash: {})", t1.elapsed(), &hash[..12]);

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

        // Fetch manifest and relay list concurrently — don't block manifest
        // on relay discovery since we already have fallback relays connected
        let t_manifest = std::time::Instant::now();
        let (manifest_result, relay_urls) = tokio::join!(
            self.relays.fetch_manifest(pubkey, site_name),
            self.get_relay_list(pubkey, pubkey_hex),
        );
        debug!("manifest + relay list fetched in {:?}", t_manifest.elapsed());

        // Add discovered relays for future queries
        if !relay_urls.is_empty() {
            self.relays.add_relays(&relay_urls).await;
        }

        if let Some(event) = manifest_result? {
            // Cache the event
            if let Ok(json) = serde_json::to_vec(&event) {
                let _ = self.cache.put_manifest(&cache_key, &json);
            }

            let manifest = Manifest::from_event(&event)?;
            info!(
                "fetched v2 manifest for {pubkey_hex}: {} file(s)",
                manifest.files.len()
            );
            return Ok(manifest);
        }

        // Fall back to v1 (kind 34128 per-file events)
        let v1_events = self.relays.fetch_v1_file_events(pubkey).await?;
        if !v1_events.is_empty() {
            let manifest = Manifest::from_v1_events(&v1_events)?;
            info!(
                "assembled v1 manifest for {pubkey_hex}: {} file(s)",
                manifest.files.len()
            );
            return Ok(manifest);
        }

        Err(ResolverError::ManifestNotFound(pubkey_hex.to_string()))
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

        // Build server list: manifest servers → fallbacks → blossom list (if needed)
        let mut servers: Vec<String> = manifest.servers.clone();

        // Add configured blossom servers
        for s in &self.config.blossom_servers {
            if !servers.contains(s) {
                servers.push(s.clone());
            }
        }

        // Only query kind 10063 if we have no servers at all (unlikely with fallbacks)
        if servers.is_empty() {
            let blossom_servers = self.get_blossom_list(pubkey, pubkey_hex).await;
            for s in blossom_servers {
                if !servers.contains(&s) {
                    servers.push(s);
                }
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
