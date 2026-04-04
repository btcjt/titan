//! Titan — a native nsite:// browser for the Nostr web.
//!
//! Named after Titan, moon of Saturn.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;
use titan_bitcoin::rpc::{BitcoinRpc, RpcConfig};
use titan_bitcoin::store::NameStore;
use titan_resolver::Resolver;
use tokio::sync::{OnceCell, RwLock};
use tracing::{info, warn};

/// Shared app state accessible from Tauri commands.
struct AppState {
    resolver: OnceCell<Resolver>,
    /// Bitcoin name index (SQLite). Wrapped in std::sync::Mutex because
    /// rusqlite::Connection is !Send — only held briefly for lookups.
    name_store: std::sync::Mutex<Option<NameStore>>,
    /// Bitcoin Core RPC config (for name registration/transfer).
    rpc_config: Option<RpcConfig>,
    /// Current navigation context — pubkey + site name for sub-resource resolution.
    current_nav: RwLock<Option<NavContext>>,
    cache_dir: PathBuf,
}

impl AppState {
    /// Get or initialize the resolver (lock-free after first call).
    async fn resolver(&self) -> Result<&Resolver, String> {
        self.resolver
            .get_or_try_init(|| async {
                info!("initializing resolver...");
                Resolver::new(self.cache_dir.clone())
                    .await
                    .map_err(|e| format!("Failed to initialize resolver: {e}"))
            })
            .await
    }
}

#[derive(Clone)]
struct NavContext {
    pubkey: [u8; 32],
    site_name: Option<String>,
}

/// Response payload sent back to the frontend.
#[derive(Serialize)]
struct NavigateResponse {
    /// The URL to load in the iframe (nsite-content://localhost/<path>).
    content_url: String,
}

/// Parsed nsite host — pubkey + optional site name.
#[derive(Debug)]
struct ParsedHost {
    pubkey: [u8; 32],
    site_name: Option<String>,
}

/// Tauri command: navigate to an nsite host + path.
///
/// Parses the host, sets the navigation context, and returns a content URL.
/// The protocol handler does all actual resolution — no double-fetch.
#[tauri::command]
async fn navigate(
    state: State<'_, Arc<AppState>>,
    host: String,
    path: String,
) -> Result<NavigateResponse, String> {
    // Ensure resolver is initialized (fast no-op after first call)
    let resolver = state.resolver().await?;

    // Parse host into pubkey + optional site name
    // For Bitcoin names: try Nostr index first (fast, no Bitcoin Core needed),
    // fall back to local SQLite index
    let parsed = {
        let store_lock = state.name_store.lock().unwrap();
        parse_host_sync(&host, store_lock.as_ref())
    };
    let parsed = match parsed {
        Ok(p) => p,
        Err(_) => {
            // Sync parse failed — try Nostr-based lookup for valid names
            if let Ok(name) = titan_types::TitanName::new(&host) {
                match resolver.lookup_name(name.as_str()).await {
                    Ok(Some(pubkey)) => {
                        info!("resolved '{host}' via Nostr index");
                        ParsedHost {
                            pubkey,
                            site_name: Some(host.clone()),
                        }
                    }
                    Ok(None) => {
                        return Err(format!("Name '{host}' is not registered."));
                    }
                    Err(e) => {
                        return Err(format!("Name lookup failed: {e}"));
                    }
                }
            } else {
                return Err(format!("Invalid nsite address: {host}"));
            }
        }
    };
    let path = if path.is_empty() || path == "/" {
        "/".to_string()
    } else {
        path
    };

    info!(
        "navigating to {host}{path}{}",
        parsed
            .site_name
            .as_ref()
            .map(|n| format!(" (site: {n})"))
            .unwrap_or_default()
    );

    // Store nav context for sub-resource resolution (RwLock: fast reads, rare writes)
    {
        let mut nav = state.current_nav.write().await;
        *nav = Some(NavContext {
            pubkey: parsed.pubkey,
            site_name: parsed.site_name.clone(),
        });
    }

    let content_url = format!("nsite-content://localhost{path}");

    Ok(NavigateResponse { content_url })
}

// ── Name Manager Commands ──

#[derive(Serialize)]
struct NameLookupResult {
    name: String,
    available: bool,
    pubkey: Option<String>,
    owner_address: Option<String>,
    txid: Option<String>,
    block_height: Option<u64>,
}

#[derive(Serialize)]
struct IndexStats {
    connected: bool,
    block_height: Option<u64>,
    block_hash: Option<String>,
}

#[derive(Serialize)]
struct RegisterResult {
    txid: String,
    name: String,
    fee_sats: u64,
}

/// Look up a name in the Bitcoin name index.
#[tauri::command]
async fn lookup_name(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<NameLookupResult, String> {
    let name = titan_types::TitanName::new(&name)
        .map_err(|e| format!("Invalid name: {e}"))?;

    let store_lock = state.name_store.lock().unwrap();
    let store = store_lock
        .as_ref()
        .ok_or("Name index not available")?;

    match store.get_name(&name).map_err(|e| format!("{e}"))? {
        Some(record) => Ok(NameLookupResult {
            name: record.name.to_string(),
            available: false,
            pubkey: Some(hex::encode(record.pubkey)),
            owner_address: Some(record.owner_address),
            txid: Some(record.txid),
            block_height: Some(record.block_height),
        }),
        None => Ok(NameLookupResult {
            name: name.to_string(),
            available: true,
            pubkey: None,
            owner_address: None,
            txid: None,
            block_height: None,
        }),
    }
}

/// Get the current indexer sync state.
#[tauri::command]
async fn get_index_stats(
    state: State<'_, Arc<AppState>>,
) -> Result<IndexStats, String> {
    let store_lock = state.name_store.lock().unwrap();
    let store = match store_lock.as_ref() {
        Some(s) => s,
        None => {
            return Ok(IndexStats {
                connected: false,
                block_height: None,
                block_hash: None,
            })
        }
    };

    let sync = store
        .get_sync_state()
        .map_err(|e| format!("{e}"))?;

    match sync {
        Some(s) => Ok(IndexStats {
            connected: true,
            block_height: Some(s.block_height),
            block_hash: Some(s.block_hash),
        }),
        None => Ok(IndexStats {
            connected: true,
            block_height: None,
            block_hash: None,
        }),
    }
}

/// Register a name on Bitcoin. Requires Bitcoin Core wallet access.
#[tauri::command]
async fn register_name(
    state: State<'_, Arc<AppState>>,
    name: String,
    pubkey_hex: String,
) -> Result<RegisterResult, String> {
    let name = titan_types::TitanName::new(&name)
        .map_err(|e| format!("Invalid name: {e}"))?;

    let pubkey_bytes = hex::decode(&pubkey_hex)
        .map_err(|e| format!("Invalid pubkey hex: {e}"))?;
    if pubkey_bytes.len() != 32 {
        return Err("Pubkey must be 32 bytes (64 hex chars)".to_string());
    }
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&pubkey_bytes);

    // Check if already registered
    {
        let store_lock = state.name_store.lock().unwrap();
        if let Some(store) = store_lock.as_ref() {
            if let Ok(Some(_)) = store.get_name(&name) {
                return Err(format!("Name '{}' is already registered", name));
            }
        }
    }

    let rpc_config = state
        .rpc_config
        .as_ref()
        .ok_or("Bitcoin Core RPC not configured. Set BITCOIN_RPC_URL, BITCOIN_RPC_USER, BITCOIN_RPC_PASS environment variables.")?;

    let rpc = BitcoinRpc::new(rpc_config.clone());

    let result = titan_bitcoin::tx::register_name(&rpc, &name, &pubkey)
        .await
        .map_err(|e| format!("{e}"))?;

    Ok(RegisterResult {
        txid: result.txid,
        name: result.name,
        fee_sats: (result.fee_btc * 1e8) as u64,
    })
}

/// Parse a host string into a pubkey and optional site name.
///
/// The address type determines the site model:
/// - **`npub1...`** → direct pubkey, no site name → kind 15128 (root manifest)
/// - **`<64-char hex>`** → direct pubkey, no site name → kind 15128
/// - **`<name>`** → Bitcoin name → pubkey via index, name IS the site name → kind 35128
///
/// One name = one site. Register another name for another site.
/// Synchronous host parsing — handles npub, hex, base36, and local SQLite lookup.
/// For Bitcoin names not found locally, returns Err so the caller can try Nostr.
fn parse_host_sync(host: &str, name_store: Option<&NameStore>) -> Result<ParsedHost, String> {
    // npub1... bech32 → direct pubkey, root manifest (kind 15128)
    if host.starts_with("npub1") {
        return Ok(ParsedHost {
            pubkey: decode_npub(host)?,
            site_name: None,
        });
    }

    // 64-char hex pubkey → direct pubkey, root manifest (kind 15128)
    if host.len() == 64 && host.chars().all(|c| c.is_ascii_hexdigit()) {
        let bytes = hex::decode(host).map_err(|e| format!("Invalid hex pubkey: {e}"))?;
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&bytes);
        return Ok(ParsedHost {
            pubkey: pk,
            site_name: None,
        });
    }

    // Base36 pubkey + optional site name (nsite.lol compat, for testing)
    if host.len() >= 50 {
        let (b36, name) = host.split_at(50);
        if b36.chars().all(|c| c.is_ascii_alphanumeric()) {
            if let Ok(pubkey) = decode_base36_pubkey(b36) {
                let site_name = if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                };
                return Ok(ParsedHost { pubkey, site_name });
            }
        }
    }

    // Bitcoin name → the name IS the site identifier (kind 35128, d=name)
    let name = titan_types::TitanName::new(host)
        .map_err(|_| format!("Invalid nsite address: {host}"))?;

    let store = name_store.ok_or_else(|| {
        format!(
            "Name '{host}' is valid but the Bitcoin name index is not connected. \
             Use an npub for now."
        )
    })?;

    let pubkey = store
        .resolve(&name)
        .map_err(|e| format!("Index lookup failed: {e}"))?
        .ok_or_else(|| format!("Name '{host}' is not registered on Bitcoin."))?;

    Ok(ParsedHost {
        pubkey,
        site_name: Some(host.to_string()),
    })
}

/// Decode an npub1... bech32 string to a 32-byte pubkey.
fn decode_npub(npub: &str) -> Result<[u8; 32], String> {
    use nostr_sdk::prelude::*;
    let public_key =
        PublicKey::from_bech32(npub).map_err(|e| format!("Invalid npub: {e}"))?;
    Ok(public_key.to_bytes())
}

/// Decode a 50-character base36 string to a 32-byte pubkey.
fn decode_base36_pubkey(s: &str) -> Result<[u8; 32], String> {
    let bytes = bigint_from_base36(s)?;
    if bytes.len() > 32 {
        return Err("base36 value too large for 32 bytes".to_string());
    }
    let mut pubkey = [0u8; 32];
    let offset = 32 - bytes.len();
    pubkey[offset..].copy_from_slice(&bytes);
    Ok(pubkey)
}

/// Convert a base36 string to big-endian bytes.
fn bigint_from_base36(s: &str) -> Result<Vec<u8>, String> {
    let mut result: Vec<u8> = vec![0];

    for ch in s.chars() {
        let digit = match ch {
            '0'..='9' => (ch as u8) - b'0',
            'a'..='z' => (ch as u8) - b'a' + 10,
            'A'..='Z' => (ch as u8) - b'A' + 10,
            _ => return Err(format!("invalid base36 character: {ch}")),
        } as u16;

        // Multiply result by 36
        let mut carry: u16 = 0;
        for byte in result.iter_mut().rev() {
            let v = (*byte as u16) * 36 + carry;
            *byte = (v & 0xFF) as u8;
            carry = v >> 8;
        }
        while carry > 0 {
            result.insert(0, (carry & 0xFF) as u8);
            carry >>= 8;
        }

        // Add digit
        let mut carry = digit;
        for byte in result.iter_mut().rev() {
            let v = (*byte as u16) + carry;
            *byte = (v & 0xFF) as u8;
            carry = v >> 8;
        }
        while carry > 0 {
            result.insert(0, (carry & 0xFF) as u8);
            carry >>= 8;
        }
    }

    while result.len() > 1 && result[0] == 0 {
        result.remove(0);
    }

    Ok(result)
}

/// Script injected into HTML responses to intercept nsite:// link clicks.
/// Posts a message to the parent window so the browser chrome can handle navigation.
const LINK_INTERCEPT_SCRIPT: &str = r#"<script>
document.addEventListener('click', function(e) {
  var el = e.target;
  while (el && el.tagName !== 'A') el = el.parentElement;
  if (!el || !el.href) return;
  if (el.href.startsWith('nsite://')) {
    e.preventDefault();
    window.parent.postMessage({ type: 'nsite-navigate', url: el.href }, '*');
  }
}, true);
</script>"#;

/// Inject the link interceptor script into an HTML response body.
fn inject_link_interceptor(html: &[u8]) -> Vec<u8> {
    let html_str = String::from_utf8_lossy(html);
    // Insert before </head> if present, otherwise before </body>, otherwise append
    if let Some(pos) = html_str.find("</head>") {
        let mut result = html_str[..pos].to_string();
        result.push_str(LINK_INTERCEPT_SCRIPT);
        result.push_str(&html_str[pos..]);
        result.into_bytes()
    } else if let Some(pos) = html_str.find("</body>") {
        let mut result = html_str[..pos].to_string();
        result.push_str(LINK_INTERCEPT_SCRIPT);
        result.push_str(&html_str[pos..]);
        result.into_bytes()
    } else {
        let mut result = html_str.into_owned();
        result.push_str(LINK_INTERCEPT_SCRIPT);
        result.into_bytes()
    }
}

/// Guess the content type from a file path extension.
fn guess_content_type(path: &str) -> String {
    let path_lower = path.to_lowercase();
    if path_lower == "/" || path_lower.ends_with('/') || !path_lower.contains('.') {
        return "text/html".to_string();
    }
    match path_lower.rsplit('.').next().unwrap_or("") {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "application/javascript",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Start the background Bitcoin block indexer.
///
/// Attempts to connect to Bitcoin Core RPC. If unavailable, logs a warning
/// and returns — the browser still works for npub-based navigation.
/// If connected, syncs to tip then polls every 30 seconds for new blocks.
fn start_background_indexer(db_path: String, rpc_config: Option<RpcConfig>) {
    use titan_bitcoin::indexer::Indexer;

    let rpc_config = match rpc_config {
        Some(c) => c,
        None => {
            info!("no Bitcoin RPC credentials configured — indexer disabled");
            return;
        }
    };

    // Block height to start scanning from on first run.
    // NSIT registrations can't exist before the protocol was deployed.
    // Default: block 943614 (just before the first titan registration).
    let start_height: u64 = std::env::var("BITCOIN_START_HEIGHT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(943614);

    tauri::async_runtime::spawn(async move {
        let rpc = BitcoinRpc::new(rpc_config);

        // Check if Bitcoin Core is reachable
        match rpc.get_blockchain_info().await {
            Ok(info) => {
                if info.chain != "main" {
                    warn!("Bitcoin Core is on chain '{}', expected 'main' — indexer disabled", info.chain);
                    return;
                }
                if info.initial_block_download {
                    warn!("Bitcoin Core is still syncing (IBD) — indexer will wait");
                }
                info!(
                    "Bitcoin Core connected: chain={}, height={}, headers={}",
                    info.chain, info.blocks, info.headers
                );
            }
            Err(e) => {
                info!("Bitcoin Core not available ({e}) — name indexing disabled, npub navigation still works");
                return;
            }
        }

        // Open a separate DB connection for the indexer (SQLite supports concurrent readers)
        let store = match NameStore::open(&db_path) {
            Ok(s) => s,
            Err(e) => {
                warn!("failed to open indexer DB: {e}");
                return;
            }
        };

        let mut indexer = Indexer::new(rpc, store);

        // Set start height on first run (no existing sync state)
        if indexer.store().get_sync_state().ok().flatten().is_none() {
            info!("indexer: first run, setting start height to {start_height}");
            if let Err(e) = indexer.set_start_height(start_height).await {
                warn!("failed to set start height: {e}");
                return;
            }
        }

        // Initial sync
        match indexer.sync_to_tip().await {
            Ok(n) => {
                if n > 0 {
                    info!("indexer: synced {n} block(s)");
                } else {
                    info!("indexer: already at tip");
                }
            }
            Err(e) => {
                warn!("indexer sync error: {e}");
            }
        }

        // Poll every 30 seconds
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            match indexer.sync_to_tip().await {
                Ok(n) => {
                    if n > 0 {
                        info!("indexer: synced {n} new block(s)");
                    }
                }
                Err(e) => {
                    warn!("indexer poll error: {e}");
                }
            }
        }
    });
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "titan=info".parse().unwrap()),
        )
        .init();

    info!("starting Titan browser");

    let project_dirs = directories::ProjectDirs::from("com", "titan", "browser");
    let cache_dir = project_dirs
        .as_ref()
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".titan-cache"));
    let data_dir = project_dirs
        .as_ref()
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".titan-data"));

    // Initialize the Bitcoin name index (SQLite)
    let name_store = match std::fs::create_dir_all(&data_dir) {
        Ok(_) => {
            let db_path = data_dir.join("names.db");
            match NameStore::open(db_path.to_str().unwrap_or("names.db")) {
                Ok(store) => {
                    info!("name index opened at {}", db_path.display());
                    Some(store)
                }
                Err(e) => {
                    warn!("failed to open name index: {e} — name resolution disabled");
                    None
                }
            }
        }
        Err(e) => {
            warn!("failed to create data directory: {e} — name resolution disabled");
            None
        }
    };

    // Build RPC config from env (shared by indexer and name manager commands)
    let rpc_url = std::env::var("BITCOIN_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8332".to_string());
    let rpc_user = std::env::var("BITCOIN_RPC_USER").unwrap_or_default();
    let rpc_pass = std::env::var("BITCOIN_RPC_PASS").unwrap_or_default();
    let rpc_wallet = std::env::var("BITCOIN_RPC_WALLET").ok();

    let rpc_config = if !rpc_user.is_empty() {
        Some(RpcConfig {
            url: rpc_url,
            user: rpc_user,
            password: rpc_pass,
            wallet: rpc_wallet,
        })
    } else {
        None
    };

    // Try to start the background Bitcoin indexer
    let db_path_str = data_dir.join("names.db");
    let db_path_for_indexer = db_path_str.to_str().unwrap_or("names.db").to_string();
    start_background_indexer(db_path_for_indexer, rpc_config.clone());

    let state = Arc::new(AppState {
        resolver: OnceCell::new(),
        name_store: std::sync::Mutex::new(name_store),
        rpc_config,
        current_nav: RwLock::new(None),
        cache_dir,
    });

    let protocol_state = state.clone();
    let shutdown_state = state.clone();

    tauri::Builder::default()
        .manage(state)
        .on_window_event(move |_window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                let state = shutdown_state.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(resolver) = state.resolver.get() {
                        info!("shutting down relay connections...");
                        let _ = resolver.disconnect().await;
                    }
                });
            }
        })
        .register_asynchronous_uri_scheme_protocol("nsite-content", move |_ctx, request, responder| {
            let state = protocol_state.clone();

            // Each sub-resource request runs concurrently — no mutex contention
            tauri::async_runtime::spawn(async move {
                let path = request.uri().path();
                let path = if path.is_empty() { "/" } else { path };

                info!("protocol request: {path}");

                // Read nav context (RwLock: many concurrent readers)
                let nav = {
                    let lock = state.current_nav.read().await;
                    lock.clone()
                };

                let nav = match nav {
                    Some(n) => n,
                    None => {
                        responder.respond(
                            tauri::http::Response::builder()
                                .status(404)
                                .body(b"No active navigation".to_vec())
                                .unwrap(),
                        );
                        return;
                    }
                };

                // Get resolver (lock-free after init)
                let resolver = match state.resolver().await {
                    Ok(r) => r,
                    Err(e) => {
                        responder.respond(
                            tauri::http::Response::builder()
                                .status(500)
                                .body(format!("Resolver error: {e}").into_bytes())
                                .unwrap(),
                        );
                        return;
                    }
                };

                // Resolve the path — runs concurrently with other sub-resource requests
                match resolver.resolve(&nav.pubkey, path, nav.site_name.as_deref()).await {
                    Ok(content) => {
                        let content_type = guess_content_type(path);
                        // Inject link interceptor into HTML responses
                        let body = if content_type == "text/html" {
                            inject_link_interceptor(&content.data)
                        } else {
                            content.data
                        };
                        responder.respond(
                            tauri::http::Response::builder()
                                .status(200)
                                .header("content-type", &content_type)
                                .header("access-control-allow-origin", "*")
                                .body(body)
                                .unwrap(),
                        );
                    }
                    Err(e) => {
                        warn!("failed to resolve {path}: {e}");
                        responder.respond(
                            tauri::http::Response::builder()
                                .status(404)
                                .header("content-type", "text/plain")
                                .body(format!("Not found: {e}").into_bytes())
                                .unwrap(),
                        );
                    }
                }
            });
        })
        .invoke_handler(tauri::generate_handler![navigate, lookup_name, get_index_stats, register_name])
        .run(tauri::generate_context!())
        .expect("failed to run Titan");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_inference() {
        assert_eq!(guess_content_type("/"), "text/html");
        assert_eq!(guess_content_type("/index.html"), "text/html");
        assert_eq!(guess_content_type("/style.css"), "text/css");
        assert_eq!(guess_content_type("/app.js"), "application/javascript");
        assert_eq!(guess_content_type("/photo.png"), "image/png");
        assert_eq!(guess_content_type("/data.json"), "application/json");
        assert_eq!(guess_content_type("/unknown.xyz"), "application/octet-stream");
        assert_eq!(guess_content_type("/blog"), "text/html");
        assert_eq!(
            guess_content_type("/manifest.webmanifest"),
            "application/manifest+json"
        );
    }

    #[test]
    fn resolve_hex_pubkey() {
        let hex = "ab".repeat(32);
        let parsed = parse_host_sync(&hex, None).unwrap();
        assert_eq!(parsed.pubkey, [0xab; 32]);
        assert!(parsed.site_name.is_none());
    }

    #[test]
    fn resolve_npub() {
        let parsed =
            parse_host_sync("npub10qdp2fc9ta6vraczxrcs8prqnv69fru2k6s2dj48gqjcylulmtjsg9arpj", None)
                .unwrap();
        assert_eq!(
            hex::encode(parsed.pubkey),
            "781a1527055f74c1f70230f10384609b34548f8ab6a0a6caa74025827f9fdae5"
        );
        assert!(parsed.site_name.is_none());
    }

    #[test]
    fn resolve_base36_with_site_name() {
        let parsed = parse_host_sync(
            "2zrgjemvgxppn2jwgm61w6yrqqlcmm8njvhby68a9cj7ooo5phshakespeare",
            None,
        )
        .unwrap();
        assert_eq!(
            hex::encode(parsed.pubkey),
            "781a1527055f74c1f70230f10384609b34548f8ab6a0a6caa74025827f9fdae5"
        );
        assert_eq!(parsed.site_name.as_deref(), Some("shakespeare"));
    }

    #[test]
    fn resolve_base36_without_site_name() {
        let parsed =
            parse_host_sync("2zrgjemvgxppn2jwgm61w6yrqqlcmm8njvhby68a9cj7ooo5ph", None).unwrap();
        assert_eq!(
            hex::encode(parsed.pubkey),
            "781a1527055f74c1f70230f10384609b34548f8ab6a0a6caa74025827f9fdae5"
        );
        assert!(parsed.site_name.is_none());
    }

    #[test]
    fn bigint_base36_decode() {
        let bytes =
            bigint_from_base36("2zrgjemvgxppn2jwgm61w6yrqqlcmm8njvhby68a9cj7ooo5ph").unwrap();
        let mut padded = vec![0u8; 32 - bytes.len()];
        padded.extend_from_slice(&bytes);
        assert_eq!(
            hex::encode(&padded),
            "781a1527055f74c1f70230f10384609b34548f8ab6a0a6caa74025827f9fdae5"
        );
    }

    #[test]
    fn resolve_name_without_store() {
        // Without a name store, valid names error gracefully
        let err = parse_host_sync("westernbtc", None).unwrap_err();
        assert!(err.contains("not connected"));
    }

    #[test]
    fn resolve_name_with_store() {
        let store = NameStore::open_memory().unwrap();
        let pk = [0xab; 32];
        store
            .insert_name(
                &titan_types::TitanName::new("westernbtc").unwrap(),
                &pk,
                "bc1qtest",
                "tx1",
                800_000,
            )
            .unwrap();

        let parsed = parse_host_sync("westernbtc", Some(&store)).unwrap();
        assert_eq!(parsed.pubkey, pk);
        assert_eq!(parsed.site_name.as_deref(), Some("westernbtc"));
    }

    #[test]
    fn resolve_name_not_registered() {
        let store = NameStore::open_memory().unwrap();
        let err = parse_host_sync("doesnotexist", Some(&store)).unwrap_err();
        assert!(err.contains("not registered"));
    }
}
