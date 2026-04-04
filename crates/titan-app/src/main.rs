//! Titan — a native nsite:// browser for the Nostr web.
//!
//! Named after Titan, moon of Saturn.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;
use titan_resolver::Resolver;
use tokio::sync::{OnceCell, RwLock};
use tracing::{info, warn};

/// Shared app state accessible from Tauri commands.
struct AppState {
    resolver: OnceCell<Resolver>,
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
/// Parses the host, resolves names via Nostr, sets the navigation context,
/// and returns a content URL. The protocol handler does all actual content fetching.
#[tauri::command]
async fn navigate(
    state: State<'_, Arc<AppState>>,
    host: String,
    path: String,
) -> Result<NavigateResponse, String> {
    let resolver = state.resolver().await?;

    // Parse host — npub/hex/base36 are instant, names go to Nostr
    let parsed = match parse_host_sync(&host) {
        Ok(p) => p,
        Err(_) => {
            // Not an npub/hex/base36 — try as a Bitcoin name via Nostr index
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

    // Store nav context for sub-resource resolution
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

/// Synchronous host parsing — handles npub, hex, and base36.
/// Bitcoin names return Err so the caller routes to Nostr lookup.
fn parse_host_sync(host: &str) -> Result<ParsedHost, String> {
    // npub1... bech32
    if host.starts_with("npub1") {
        return Ok(ParsedHost {
            pubkey: decode_npub(host)?,
            site_name: None,
        });
    }

    // 64-char hex pubkey
    if host.len() == 64 && host.chars().all(|c| c.is_ascii_hexdigit()) {
        let bytes = hex::decode(host).map_err(|e| format!("Invalid hex pubkey: {e}"))?;
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&bytes);
        return Ok(ParsedHost {
            pubkey: pk,
            site_name: None,
        });
    }

    // Base36 pubkey + optional site name (nsite.lol compat)
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

    Err(format!("Not a direct pubkey: {host}"))
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

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "titan=info".parse().unwrap()),
        )
        .init();

    info!("starting Titan browser");

    let cache_dir = directories::ProjectDirs::from("com", "titan", "browser")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".titan-cache"));

    let state = Arc::new(AppState {
        resolver: OnceCell::new(),
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

            tauri::async_runtime::spawn(async move {
                let path = request.uri().path();
                let path = if path.is_empty() { "/" } else { path };

                info!("protocol request: {path}");

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

                match resolver.resolve(&nav.pubkey, path, nav.site_name.as_deref()).await {
                    Ok(content) => {
                        let content_type = guess_content_type(path);
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
        .invoke_handler(tauri::generate_handler![navigate])
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
        let parsed = parse_host_sync(&hex).unwrap();
        assert_eq!(parsed.pubkey, [0xab; 32]);
        assert!(parsed.site_name.is_none());
    }

    #[test]
    fn resolve_npub() {
        let parsed =
            parse_host_sync("npub10qdp2fc9ta6vraczxrcs8prqnv69fru2k6s2dj48gqjcylulmtjsg9arpj")
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
            parse_host_sync("2zrgjemvgxppn2jwgm61w6yrqqlcmm8njvhby68a9cj7ooo5ph").unwrap();
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
    fn name_goes_to_nostr() {
        // Valid names should return Err from sync parse (routed to Nostr lookup)
        assert!(parse_host_sync("westernbtc").is_err());
        assert!(parse_host_sync("titan").is_err());
    }
}
