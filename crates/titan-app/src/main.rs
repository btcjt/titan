//! Titan — a native nsite:// browser for the Nostr web.
//!
//! Two-webview architecture:
//! - Chrome webview (top): address bar, nav buttons, status bar
//! - Content webview (bottom): nsite content via nsite-content:// protocol
//!
//! Named after Titan, moon of Saturn.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use titan_resolver::Resolver;
use tokio::sync::OnceCell;
use tracing::{debug, info, warn};

/// Shared app state.
struct AppState {
    resolver: OnceCell<Resolver>,
    cache_dir: PathBuf,
}

impl AppState {
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

/// Parsed nsite host — pubkey + optional site name.
#[derive(Debug)]
struct ParsedHost {
    pubkey: [u8; 32],
    site_name: Option<String>,
}

// ── Tauri Commands ──

/// Navigate to an nsite URL. Resolves the name, then navigates the content webview.
/// Returns the display URL for the chrome address bar.
#[tauri::command]
async fn navigate(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    url: String,
) -> Result<String, String> {
    let resolver = state.resolver().await?;

    // Parse "titan/path" or "npub1.../path" or just "titan"
    let url = url.trim().replace("nsite://", "");
    let (host, path) = match url.find('/') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url.as_str(), "/"),
    };

    // Resolve host to pubkey
    let parsed = match parse_host_sync(host) {
        Ok(p) => p,
        Err(_) => {
            // Try Nostr-based name lookup
            if let Ok(name) = titan_types::TitanName::new(host) {
                match resolver.lookup_name(name.as_str()).await {
                    Ok(Some(pubkey)) => {
                        info!("resolved '{host}' via Nostr index");
                        ParsedHost {
                            pubkey,
                            site_name: Some(host.to_string()),
                        }
                    }
                    Ok(None) => return Err(format!("Name '{host}' is not registered.")),
                    Err(e) => return Err(format!("Name lookup failed: {e}")),
                }
            } else {
                return Err(format!("Invalid nsite address: {host}"));
            }
        }
    };

    let pubkey_hex = hex::encode(parsed.pubkey);
    let content_host = match &parsed.site_name {
        Some(name) => format!("{}.{}", pubkey_hex, name),
        None => pubkey_hex,
    };
    let content_url = format!("nsite-content://{}{}", content_host, path);

    info!("navigating to {host}{path}");

    // Navigate the content webview
    if let Some(content) = app.get_webview("content") {
        let _ = content.navigate(content_url.parse().unwrap());
    }

    // Return display URL for address bar
    let display = format!("{}{}", host, if path == "/" { "" } else { path });
    Ok(display)
}

#[tauri::command]
async fn go_back(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(wv) = app.get_webview("content") {
        wv.eval("history.back()").map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn go_forward(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(wv) = app.get_webview("content") {
        wv.eval("history.forward()").map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn refresh(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(wv) = app.get_webview("content") {
        wv.eval("location.reload()").map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── Host Parsing ──

fn parse_host_sync(host: &str) -> Result<ParsedHost, String> {
    if host.starts_with("npub1") {
        return Ok(ParsedHost {
            pubkey: decode_npub(host)?,
            site_name: None,
        });
    }

    if host.len() == 64 && host.chars().all(|c| c.is_ascii_hexdigit()) {
        let bytes = hex::decode(host).map_err(|e| format!("Invalid hex pubkey: {e}"))?;
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&bytes);
        return Ok(ParsedHost {
            pubkey: pk,
            site_name: None,
        });
    }

    // Base36 pubkey + optional site name
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

fn decode_npub(npub: &str) -> Result<[u8; 32], String> {
    use nostr_sdk::prelude::*;
    let public_key =
        PublicKey::from_bech32(npub).map_err(|e| format!("Invalid npub: {e}"))?;
    Ok(public_key.to_bytes())
}

fn decode_base36_pubkey(s: &str) -> Result<[u8; 32], String> {
    let bytes = bigint_from_base36(s)?;
    if bytes.len() > 32 {
        return Err("base36 value too large".to_string());
    }
    let mut pubkey = [0u8; 32];
    let offset = 32 - bytes.len();
    pubkey[offset..].copy_from_slice(&bytes);
    Ok(pubkey)
}

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

// ── Protocol Handler Helpers ──

/// Parse a nsite-content:// host into (pubkey, site_name).
/// Format: {pubkey_hex} or {pubkey_hex}.{site_name}
fn parse_content_host(host: &str) -> Option<([u8; 32], Option<String>)> {
    if host == "internal" || host == "localhost" {
        return None;
    }

    // Check for {hex}.{name} format
    if host.len() > 64 && host.as_bytes().get(64) == Some(&b'.') {
        let hex_part = &host[..64];
        let name_part = &host[65..];
        if let Ok(bytes) = hex::decode(hex_part) {
            if bytes.len() == 32 {
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&bytes);
                return Some((pk, Some(name_part.to_string())));
            }
        }
    }

    // Plain {hex} format
    if host.len() == 64 {
        if let Ok(bytes) = hex::decode(host) {
            if bytes.len() == 32 {
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&bytes);
                return Some((pk, None));
            }
        }
    }

    None
}

/// Convert a nsite-content:// URL back to a display URL for the address bar.
fn content_url_to_display(url: &tauri::Url) -> Option<String> {
    let host = url.host_str()?;
    if host == "internal" {
        return None;
    }

    let path = url.path();

    // Extract display name from {hex}.{name} format
    if host.len() > 64 && host.as_bytes().get(64) == Some(&b'.') {
        let name = &host[65..];
        let display_path = if path == "/" { "" } else { path };
        return Some(format!("{}{}", name, display_path));
    }

    // Plain hex — show as npub
    if host.len() == 64 {
        if let Ok(bytes) = hex::decode(host) {
            if bytes.len() == 32 {
                use nostr_sdk::prelude::*;
                if let Ok(pk) = PublicKey::from_slice(&bytes) {
                    let npub = pk.to_bech32().unwrap_or_else(|_| host.to_string());
                    let display_path = if path == "/" { "" } else { path };
                    return Some(format!("{}{}", npub, display_path));
                }
            }
        }
    }

    Some(host.to_string())
}

/// Guess content type from file extension.
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

// ── Internal Pages ──

fn welcome_page() -> Vec<u8> {
    r#"<!DOCTYPE html>
<html><head><meta charset="UTF-8"><style>
body { margin:0; background:#0a0a0a; color:#e8e0d4; font-family:-apple-system,system-ui,sans-serif;
  display:flex; flex-direction:column; align-items:center; justify-content:center; min-height:100vh; }
img { width:120px; height:120px; border-radius:50%; filter:drop-shadow(0 0 30px #d48f0030); margin-bottom:16px; }
h1 { font-size:32px; font-weight:300; letter-spacing:6px; text-transform:uppercase; margin:0 0 8px; }
p { color:#8a7f70; font-size:14px; font-style:italic; }
</style></head><body>
<h1>Titan</h1>
<p>A native browser for the Nostr web</p>
</body></html>"#.as_bytes().to_vec()
}

fn error_page(msg: &str) -> Vec<u8> {
    format!(r#"<!DOCTYPE html>
<html><head><meta charset="UTF-8"><style>
body {{ margin:0; background:#0a0a0a; color:#e8e0d4; font-family:-apple-system,system-ui,sans-serif;
  display:flex; flex-direction:column; align-items:center; justify-content:center; min-height:100vh; }}
.icon {{ font-size:48px; color:#a06800; margin-bottom:12px; }}
h2 {{ font-size:20px; font-weight:400; margin:0 0 8px; }}
p {{ color:#8a7f70; font-size:13px; max-width:400px; text-align:center; }}
</style></head><body>
<div class="icon">&#x26A0;</div>
<h2>Navigation Failed</h2>
<p>{}</p>
</body></html>"#, html_escape(msg)).into_bytes()
}

fn loading_page() -> Vec<u8> {
    r#"<!DOCTYPE html>
<html><head><meta charset="UTF-8"><style>
body { margin:0; background:#0a0a0a; color:#e8e0d4; font-family:-apple-system,system-ui,sans-serif;
  display:flex; flex-direction:column; align-items:center; justify-content:center; min-height:100vh; }
.spinner { width:36px; height:36px; border:3px solid #2a2520; border-top-color:#d48f00;
  border-radius:50%; animation:spin 0.8s linear infinite; margin-bottom:16px; }
@keyframes spin { to { transform:rotate(360deg); } }
p { color:#8a7f70; font-size:13px; }
</style></head><body>
<div class="spinner"></div>
<p>Resolving...</p>
</body></html>"#.as_bytes().to_vec()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

// ── Main ──

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
        cache_dir,
    });

    let protocol_state = state.clone();

    tauri::Builder::default()
        .manage(state)
        .register_asynchronous_uri_scheme_protocol("nsite-content", move |_ctx, request, responder| {
            let state = protocol_state.clone();

            tauri::async_runtime::spawn(async move {
                let uri = request.uri();
                let host = uri.host().unwrap_or("internal");
                let path = uri.path();
                let path = if path.is_empty() { "/" } else { path };

                debug!("protocol: {host}{path}");

                // Internal pages
                if host == "internal" {
                    let (body, ct) = match path {
                        "/welcome" | "/" => (welcome_page(), "text/html"),
                        "/loading" => (loading_page(), "text/html"),
                        p if p.starts_with("/error") => {
                            let msg = uri.query()
                                .and_then(|q| q.strip_prefix("msg="))
                                .unwrap_or("Unknown error");
                            (error_page(msg), "text/html")
                        }
                        _ => (welcome_page(), "text/html"),
                    };
                    responder.respond(
                        tauri::http::Response::builder()
                            .status(200)
                            .header("content-type", ct)
                            .body(body)
                            .unwrap(),
                    );
                    return;
                }

                // Parse site identity from host
                let (pubkey, site_name) = match parse_content_host(host) {
                    Some(p) => p,
                    None => {
                        responder.respond(
                            tauri::http::Response::builder()
                                .status(404)
                                .header("content-type", "text/html")
                                .body(error_page("Invalid content host"))
                                .unwrap(),
                        );
                        return;
                    }
                };

                // Resolve content
                let resolver = match state.resolver().await {
                    Ok(r) => r,
                    Err(e) => {
                        responder.respond(
                            tauri::http::Response::builder()
                                .status(500)
                                .header("content-type", "text/html")
                                .body(error_page(&e))
                                .unwrap(),
                        );
                        return;
                    }
                };

                match resolver.resolve(&pubkey, path, site_name.as_deref()).await {
                    Ok(content) => {
                        let content_type = guess_content_type(path);
                        responder.respond(
                            tauri::http::Response::builder()
                                .status(200)
                                .header("content-type", &content_type)
                                .header("access-control-allow-origin", "*")
                                .body(content.data)
                                .unwrap(),
                        );
                    }
                    Err(e) => {
                        warn!("failed to resolve {host}{path}: {e}");
                        responder.respond(
                            tauri::http::Response::builder()
                                .status(404)
                                .header("content-type", "text/html")
                                .body(error_page(&format!("{e}")))
                                .unwrap(),
                        );
                    }
                }
            });
        })
        .invoke_handler(tauri::generate_handler![navigate, go_back, go_forward, refresh])
        .setup(|app| {
            let window = app.get_window("main").unwrap();
            let window_size = window.inner_size().unwrap();

            let chrome_height = 72.0;

            // Content webview — fills the space below the chrome toolbar
            let app_handle = app.handle().clone();
            let app_handle2 = app.handle().clone();
            let _content_webview = window.add_child(
                tauri::webview::WebviewBuilder::new(
                    "content",
                    tauri::WebviewUrl::External("nsite-content://internal/welcome".parse().unwrap()),
                )
                .auto_resize()
                .on_navigation(move |url| {
                    let scheme = url.scheme();

                    // Allow nsite-content:// (same-site nav + sub-resources)
                    if scheme == "nsite-content" {
                        return true;
                    }

                    // Block and handle nsite:// links asynchronously
                    if scheme == "nsite" {
                        let url_str = url.to_string();
                        let handle = app_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            info!("intercepted nsite:// link: {url_str}");
                            let cleaned = url_str.replace("nsite://", "");
                            if let Some(content_wv) = handle.get_webview("content") {
                                let _ = content_wv.navigate("nsite-content://internal/loading".parse().unwrap());
                            }
                            let _ = handle.emit("nsite-link-clicked", &cleaned);
                        });
                        return false;
                    }

                    // Block everything else (http, https, etc.)
                    debug!("blocked navigation to {url}");
                    false
                })
                .on_page_load(move |_webview, payload| {
                    if let tauri::webview::PageLoadEvent::Finished = payload.event() {
                        let url = payload.url();
                        if let Some(display) = content_url_to_display(url) {
                            let _ = app_handle2.emit("page-loaded", &display);
                        }
                    }
                }),
                tauri::LogicalPosition::new(0.0, chrome_height),
                tauri::LogicalSize::new(
                    window_size.width as f64,
                    window_size.height as f64 - chrome_height,
                ),
            )?;

            Ok(())
        })
        .on_window_event(|_window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                info!("window closed");
            }
        })
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
        assert_eq!(guess_content_type("/manifest.webmanifest"), "application/manifest+json");
    }

    #[test]
    fn parse_content_host_hex() {
        let hex = "ab".repeat(32);
        let (pk, name) = parse_content_host(&hex).unwrap();
        assert_eq!(pk, [0xab; 32]);
        assert!(name.is_none());
    }

    #[test]
    fn parse_content_host_hex_with_name() {
        let hex = "ab".repeat(32);
        let host = format!("{}.titan", hex);
        let (pk, name) = parse_content_host(&host).unwrap();
        assert_eq!(pk, [0xab; 32]);
        assert_eq!(name.as_deref(), Some("titan"));
    }

    #[test]
    fn parse_content_host_internal() {
        assert!(parse_content_host("internal").is_none());
        assert!(parse_content_host("localhost").is_none());
    }

    #[test]
    fn resolve_npub_sync() {
        let parsed =
            parse_host_sync("npub10qdp2fc9ta6vraczxrcs8prqnv69fru2k6s2dj48gqjcylulmtjsg9arpj")
                .unwrap();
        assert_eq!(
            hex::encode(parsed.pubkey),
            "781a1527055f74c1f70230f10384609b34548f8ab6a0a6caa74025827f9fdae5"
        );
    }

    #[test]
    fn name_goes_to_nostr() {
        assert!(parse_host_sync("westernbtc").is_err());
        assert!(parse_host_sync("titan").is_err());
    }
}
