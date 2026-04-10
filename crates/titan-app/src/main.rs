//! Titan — a native nsite:// browser for the Nostr web.
//!
//! Multi-webview architecture:
//! - Chrome webview (top): address bar, nav buttons, tab strip, side panels
//! - Tab webviews (bottom): one per tab, nsite content via nsite-content:// protocol
//!
//! Named after Titan, moon of Saturn.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audit_log;
mod bookmarks;
mod devtools;
mod log_forward;
mod nip07;
mod permissions;
mod prompt_queue;
mod signer;

use audit_log::AuditLog;
use permissions::Permissions;
use prompt_queue::PromptQueue;
use serde::{Deserialize, Serialize};
use signer::Signer;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use titan_resolver::Resolver;
use tokio::sync::OnceCell;
use tracing::{debug, info, warn};

/// JS injected into every content webview at page-start. Exposes `window.nostr`
/// (NIP-07) backed by Titan's built-in signer. Requests are routed via the
/// `titan-nostr://` async protocol handler.
const WINDOW_NOSTR_INJECTION: &str = r#"
(function() {
    if (window.nostr && window.nostr.__titan) return;

    var reqCounter = 0;
    function nextId() {
        reqCounter += 1;
        return 'r' + Date.now() + '_' + reqCounter;
    }

    async function call(method, params) {
        var id = nextId();
        var body = JSON.stringify({ id: id, method: method, params: params || null });
        var resp;
        try {
            resp = await fetch('titan-nostr://rpc', {
                method: 'POST',
                headers: { 'content-type': 'application/json' },
                body: body,
            });
        } catch (e) {
            throw new Error('Titan signer unreachable: ' + e);
        }
        var data;
        try {
            data = await resp.json();
        } catch (e) {
            throw new Error('Titan signer returned invalid JSON');
        }
        if (data.error) {
            throw new Error(data.error);
        }
        return data.result;
    }

    window.nostr = {
        __titan: true,
        getPublicKey: function() { return call('getPublicKey', null); },
        signEvent: function(event) { return call('signEvent', event); },
        getRelays: function() { return call('getRelays', null); },
        nip04: {
            encrypt: function(pubkey, plaintext) {
                return call('nip04.encrypt', { pubkey: pubkey, plaintext: plaintext });
            },
            decrypt: function(pubkey, ciphertext) {
                return call('nip04.decrypt', { pubkey: pubkey, ciphertext: ciphertext });
            },
        },
        nip44: {
            encrypt: function(pubkey, plaintext) {
                return call('nip44.encrypt', { pubkey: pubkey, plaintext: plaintext });
            },
            decrypt: function(pubkey, ciphertext) {
                return call('nip44.decrypt', { pubkey: pubkey, ciphertext: ciphertext });
            },
        },
    };
})();
"#;

/// A browser tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Tab {
    id: u32,
    label: String,
    display_url: String,
    title: String,
}

/// Tab state returned to JS.
#[derive(Debug, Clone, Serialize)]
struct TabsPayload {
    tabs: Vec<Tab>,
    active_tab: u32,
}

/// Page-loaded event payload (includes tab identity).
#[derive(Debug, Clone, Serialize)]
struct PageLoadedPayload {
    tab_label: String,
    url: String,
}

/// Console message forwarded from content webview.
#[derive(Debug, Clone, Serialize)]
struct ConsolePayload {
    level: String,
    message: String,
    tab_label: String,
}

// `Bookmark` lives in the `bookmarks` module along with the
// NIP-51-backed BookmarkStore. Re-imported here so existing call sites
// (Tauri commands, the bookmarks_page renderer) can stay terse.
use bookmarks::{Bookmark, BookmarkStore, LoadOutcome};

/// Browser settings, persisted to settings.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Settings {
    /// Nostr relays for content resolution (manifests, relay lists, etc.)
    relays: Vec<String>,
    /// NIP-65 discovery relays for relay list lookups
    discovery_relays: Vec<String>,
    /// Blossom servers for blob fetching
    blossom_servers: Vec<String>,
    /// NSIT indexer pubkey (hex) for name lookups
    indexer_pubkey: String,
    /// Default homepage
    homepage: String,
    /// Side panel width in CSS pixels. Persisted across sessions so a
    /// user who dragged it wider (e.g. for the dev console) stays at
    /// that size. Clamped at load time to the sane range below.
    #[serde(default = "default_side_panel_width")]
    side_panel_width: u32,
}

const DEFAULT_SIDE_PANEL_WIDTH: u32 = 280;
const MIN_SIDE_PANEL_WIDTH: u32 = 280;
const MAX_SIDE_PANEL_WIDTH: u32 = 1400;

fn default_side_panel_width() -> u32 {
    DEFAULT_SIDE_PANEL_WIDTH
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            relays: vec![
                "wss://relay.westernbtc.com".to_string(),
                "wss://relay.primal.net".to_string(),
                "wss://relay.damus.io".to_string(),
            ],
            discovery_relays: vec![
                "wss://purplepag.es".to_string(),
                "wss://user.kindpag.es".to_string(),
            ],
            blossom_servers: vec![
                "https://blossom.westernbtc.com".to_string(),
            ],
            indexer_pubkey: "bec1a370130fed4fb9f78f9efc725b35104d827470e75573558a87a9ac5cde44".to_string(),
            homepage: "titan".to_string(),
            side_panel_width: DEFAULT_SIDE_PANEL_WIDTH,
        }
    }
}

/// Shared app state.
struct AppState {
    resolver: OnceCell<Resolver>,
    cache_dir: PathBuf,
    data_dir: PathBuf,
    bookmarks: BookmarkStore,
    settings: std::sync::Mutex<Settings>,
    tabs: std::sync::Mutex<Vec<Tab>>,
    active_tab: std::sync::Mutex<u32>,
    next_tab_id: std::sync::Mutex<u32>,
    signer: Signer,
    permissions: Permissions,
    prompt_queue: PromptQueue,
    audit_log: AuditLog,
    devtools: devtools::DevtoolsState,
}

impl AppState {
    async fn resolver(&self) -> Result<&Resolver, String> {
        self.resolver
            .get_or_try_init(|| async {
                info!("initializing resolver...");
                let settings = self.settings.lock().unwrap().clone();
                let config = titan_resolver::ResolverConfig {
                    relays: settings.relays,
                    discovery_relays: settings.discovery_relays,
                    blossom_servers: settings.blossom_servers,
                    indexer_pubkey: settings.indexer_pubkey,
                };
                Resolver::new_with_config(self.cache_dir.clone(), config)
                    .await
                    .map_err(|e| format!("Failed to initialize resolver: {e}"))
            })
            .await
    }
}

/// Get the webview label of the active tab.
fn active_webview_label(state: &AppState) -> Option<String> {
    let active = *state.active_tab.lock().unwrap();
    let tabs = state.tabs.lock().unwrap();
    tabs.iter().find(|t| t.id == active).map(|t| t.label.clone())
}

/// Rewrite a `nsite-content://` URL to the form WebView2 (Windows) expects.
///
/// Windows WebView2 doesn't support custom URL schemes natively. Wry works
/// around this by rewriting `nsite-content://foo/bar` to
/// `http://nsite-content.foo/bar` internally and registering a filter for
/// that prefix. But wry only applies the rewrite at webview creation time —
/// when we call `webview.navigate()` later, the raw `nsite-content://` URL
/// is passed straight through, and WebView2 silently drops it.
///
/// On macOS and Linux, `nsite-content://` is a real custom scheme with full
/// native support, so no rewrite is needed.
#[cfg(target_os = "windows")]
fn platform_navigate_url(nsite_content_url: &str) -> String {
    if let Some(rest) = nsite_content_url.strip_prefix("nsite-content://") {
        format!("http://nsite-content.{rest}")
    } else {
        nsite_content_url.to_string()
    }
}

#[cfg(not(target_os = "windows"))]
fn platform_navigate_url(nsite_content_url: &str) -> String {
    nsite_content_url.to_string()
}

/// Extract the "site origin" for a tab — the first path segment of its
/// display URL (e.g. "titan" or "npub1..."). Used as the permission key
/// so different paths on the same site share permissions.
fn tab_site_for_label(state: &AppState, webview_label: &str) -> Option<String> {
    let tabs = state.tabs.lock().unwrap();
    let tab = tabs.iter().find(|t| t.label == webview_label)?;
    let url = &tab.display_url;
    if url.is_empty() {
        return None;
    }
    let host = match url.find('/') {
        Some(i) => &url[..i],
        None => url.as_str(),
    };
    Some(host.to_string())
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

    let active_label = active_webview_label(&state).ok_or("No active tab")?;

    // Internal pages (bookmarks, etc.)
    if url == "internal:bookmarks" {
        if let Some(content) = app.get_webview(&active_label) {
            let url = platform_navigate_url("nsite-content://internal/bookmarks");
            let _ = content.navigate(url.parse().unwrap());
        }
        return Ok("bookmarks".to_string());
    }

    // Internal error page
    if url.starts_with("internal:error:") {
        let msg = &url["internal:error:".len()..];
        if let Some(content) = app.get_webview(&active_label) {
            let error_url = platform_navigate_url(&format!("nsite-content://internal/error?msg={}", msg));
            let _ = content.navigate(error_url.parse().unwrap());
        }
        return Ok("error".to_string());
    }

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
    let content_url = platform_navigate_url(&format!("nsite-content://{}{}", content_host, path));

    info!("navigating to {host}{path}");

    // Navigate the active tab's webview
    if let Some(content) = app.get_webview(&active_label) {
        let _ = content.navigate(content_url.parse().unwrap());
    }

    // Return display URL for address bar
    let display = format!("{}{}", host, if path == "/" { "" } else { path });

    // Update tab state
    {
        let active_id = *state.active_tab.lock().unwrap();
        let mut tabs = state.tabs.lock().unwrap();
        if let Some(tab) = tabs.iter_mut().find(|t| t.id == active_id) {
            tab.display_url = display.clone();
            tab.title = host.to_string();
        }
    }

    Ok(display)
}

/// Resize the active tab's webview to accommodate panels.
/// Called from chrome JS when panels open/close.
#[tauri::command]
async fn resize_content(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    top: f64,
    right: f64,
) -> Result<(), String> {
    if let Some(label) = active_webview_label(&state) {
        if let Some(window) = app.get_window("main") {
            if let Some(content) = app.get_webview(&label) {
                let scale = window.scale_factor().unwrap_or(1.0);
                let phys = window.inner_size().map_err(|e| e.to_string())?;
                let lw = phys.width as f64 / scale;
                let lh = phys.height as f64 / scale;

                let _ = content.set_position(tauri::LogicalPosition::new(0.0, top));
                let _ = content.set_size(tauri::LogicalSize::new(lw - right, lh - top));
            }
        }
    }
    Ok(())
}

#[tauri::command]
async fn open_console(app: tauri::AppHandle) -> Result<(), String> {
    let _ = app.emit("open-panel", "console");
    Ok(())
}

#[tauri::command]
async fn focus_address_bar(app: tauri::AppHandle) -> Result<(), String> {
    let _ = app.emit("focus-address-bar", ());
    Ok(())
}

#[tauri::command]
async fn toggle_bookmark_cmd(app: tauri::AppHandle) -> Result<(), String> {
    let _ = app.emit("toggle-bookmark", ());
    Ok(())
}

#[tauri::command]
async fn go_back(app: tauri::AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if let Some(label) = active_webview_label(&state) {
        if let Some(wv) = app.get_webview(&label) {
            wv.eval("history.back()").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
async fn go_forward(app: tauri::AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if let Some(label) = active_webview_label(&state) {
        if let Some(wv) = app.get_webview(&label) {
            wv.eval("history.forward()").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
async fn console_eval(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    code: String,
) -> Result<(), String> {
    let label = active_webview_label(&state).ok_or("No active tab")?;
    let wv = app.get_webview(&label).ok_or("Active tab not found")?;

    // Wrap user code in an async IIFE. Send the stringified result back
    // via titan-cmd://console-result/<level>/<encoded>. We use 'info' for
    // success and 'error' for thrown exceptions.
    let code_json = serde_json::to_string(&code).map_err(|e| e.to_string())?;
    let wrapper = format!(
        r#"(async function() {{
    function __send(level, value) {{
        var text;
        try {{
            if (typeof value === 'string') text = value;
            else if (value === undefined) text = 'undefined';
            else if (value === null) text = 'null';
            else text = JSON.stringify(value, null, 2);
        }} catch (e) {{
            try {{ text = String(value); }} catch (_) {{ text = '[unserializable]'; }}
        }}
        var a = document.createElement('a');
        a.href = 'titan-cmd://console-result/' + level + '/' + encodeURIComponent(text);
        a.click();
    }}
    try {{
        var __code = {code};
        // Build an async function whose body is `return (USER_CODE);`.
        // If that fails to parse (e.g. the user typed a statement like
        // `let x = 1`), fall back to using the code as the body directly.
        var AsyncFunction = Object.getPrototypeOf(async function(){{}}).constructor;
        var fn;
        try {{
            fn = new AsyncFunction('return (' + __code + ');');
        }} catch (_) {{
            fn = new AsyncFunction(__code);
        }}
        var result = await fn.call(window);
        __send('info', result);
    }} catch (err) {{
        var msg;
        if (err == null) msg = 'null';
        else if (typeof err === 'string') msg = err;
        else if (err.message) msg = (err.name || 'Error') + ': ' + err.message + (err.stack ? '\n' + err.stack : '');
        else if (err.stack) msg = err.stack;
        else {{
            try {{ msg = JSON.stringify(err); }} catch (_) {{ msg = String(err); }}
        }}
        __send('error', msg);
    }}
}})();"#,
        code = code_json
    );

    wv.eval(&wrapper).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn refresh(app: tauri::AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if let Some(label) = active_webview_label(&state) {
        if let Some(wv) = app.get_webview(&label) {
            wv.eval("location.reload()").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ── Updater Commands ──

#[derive(Debug, Clone, Serialize)]
struct UpdateInfo {
    available: bool,
    current_version: String,
    new_version: Option<String>,
    notes: Option<String>,
    date: Option<String>,
}

#[tauri::command]
async fn check_for_update(app: tauri::AppHandle) -> Result<UpdateInfo, String> {
    use tauri_plugin_updater::UpdaterExt;
    let current = app.package_info().version.to_string();
    let updater = app
        .updater()
        .map_err(|e| format!("updater init failed: {e}"))?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("update check failed: {e}"))?;

    match update {
        Some(u) => Ok(UpdateInfo {
            available: true,
            current_version: current,
            new_version: Some(u.version.clone()),
            notes: u.body.clone(),
            date: u.date.map(|d| d.to_string()),
        }),
        None => Ok(UpdateInfo {
            available: false,
            current_version: current,
            new_version: None,
            notes: None,
            date: None,
        }),
    }
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    info!("install_update: command entered");

    let updater = app.updater().map_err(|e| {
        let msg = format!("updater init failed: {e}");
        warn!("install_update: {msg}");
        msg
    })?;
    info!("install_update: updater initialized");

    let update_opt = updater.check().await.map_err(|e| {
        let msg = format!("update check failed: {e}");
        warn!("install_update: {msg}");
        msg
    })?;
    info!("install_update: check complete, update_present={}", update_opt.is_some());

    let update = update_opt.ok_or_else(|| {
        let msg = "no update available".to_string();
        warn!("install_update: {msg}");
        msg
    })?;

    info!("install_update: downloading update {}", update.version);

    let total_bytes = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let total_bytes_cb = total_bytes.clone();

    update
        .download_and_install(
            move |chunk, total| {
                let t = total.unwrap_or(0);
                total_bytes_cb.fetch_add(chunk as u64, std::sync::atomic::Ordering::Relaxed);
                if chunk > 0 {
                    debug!("install_update: downloaded {} / {}", total_bytes_cb.load(std::sync::atomic::Ordering::Relaxed), t);
                }
            },
            || {
                info!("install_update: download complete, installing");
            },
        )
        .await
        .map_err(|e| {
            let msg = format!("install failed: {e}");
            warn!("install_update: {msg}");
            msg
        })?;

    info!("install_update: install complete, restarting");
    app.restart();
}

// ── Signer Commands ──

#[derive(Debug, Clone, Serialize)]
struct SignerStatus {
    has_identity: bool,
    unlocked: bool,
    pubkey: Option<String>,
}

#[tauri::command]
async fn signer_status(state: State<'_, Arc<AppState>>) -> Result<SignerStatus, String> {
    Ok(SignerStatus {
        has_identity: state.signer.has_identity(),
        unlocked: state.signer.is_unlocked(),
        pubkey: state.signer.get_pubkey(),
    })
}

#[tauri::command]
async fn signer_create(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    state.signer.create_new()
}

#[tauri::command]
async fn signer_import(
    state: State<'_, Arc<AppState>>,
    secret: String,
) -> Result<String, String> {
    state.signer.import(&secret)
}

#[tauri::command]
async fn signer_unlock(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    state.signer.unlock()
}

#[tauri::command]
async fn signer_lock(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.signer.lock();
    state.permissions.clear_session();
    state.prompt_queue.deny_all();
    Ok(())
}

#[tauri::command]
async fn signer_delete(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.signer.delete()
}

#[tauri::command]
async fn signer_reveal_nsec(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    state.signer.reveal_nsec()
}

// ── Permission & Prompt Commands ──

#[tauri::command]
async fn signer_pending_prompts(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<prompt_queue::PendingRequestSnapshot>, String> {
    Ok(state.prompt_queue.snapshot())
}

/// Hide the active tab's content webview so the chrome can render a
/// modal on top of it. Content webviews are native views stacked above
/// the chrome, so CSS z-index can't reach them.
#[tauri::command]
async fn hide_content_webview(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    if let Some(label) = active_webview_label(&state) {
        if let Some(wv) = app.get_webview(&label) {
            let _ = wv.set_size(tauri::LogicalSize::new(0.0, 0.0));
        }
    }
    Ok(())
}

/// Restore the active tab's content webview to normal size after a
/// modal has been dismissed.
#[tauri::command]
async fn show_content_webview(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    top: f64,
    right: f64,
) -> Result<(), String> {
    if let Some(label) = active_webview_label(&state) {
        if let Some(window) = app.get_window("main") {
            if let Some(wv) = app.get_webview(&label) {
                let scale = window.scale_factor().unwrap_or(1.0);
                let phys = window.inner_size().map_err(|e| e.to_string())?;
                let lw = phys.width as f64 / scale;
                let lh = phys.height as f64 / scale;
                let _ = wv.set_position(tauri::LogicalPosition::new(0.0, top));
                let _ = wv.set_size(tauri::LogicalSize::new(lw - right, lh - top));
            }
        }
    }
    Ok(())
}

#[tauri::command]
async fn signer_resolve_prompt(
    state: State<'_, Arc<AppState>>,
    resolution: prompt_queue::PromptResolution,
) -> Result<(), String> {
    let ok = state.prompt_queue.resolve(resolution);
    if ok {
        Ok(())
    } else {
        Err("No pending prompt with that id".to_string())
    }
}

#[tauri::command]
async fn signer_list_permissions(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<permissions::Permission>, String> {
    Ok(state.permissions.list_persisted())
}

#[tauri::command]
async fn signer_revoke_permission(
    state: State<'_, Arc<AppState>>,
    site: String,
    method: String,
) -> Result<(), String> {
    state.permissions.revoke(&site, &method);
    Ok(())
}

#[tauri::command]
async fn signer_revoke_site(
    state: State<'_, Arc<AppState>>,
    site: String,
) -> Result<(), String> {
    state.permissions.revoke_site(&site);
    Ok(())
}

#[tauri::command]
async fn signer_audit_log(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<audit_log::AuditEntry>, String> {
    Ok(state.audit_log.list())
}

#[tauri::command]
async fn signer_clear_audit_log(
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    state.audit_log.clear();
    Ok(())
}

// ── Devtools Commands ──

#[tauri::command]
async fn devtools_network_snapshot(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<devtools::NetworkEvent>, String> {
    Ok(state.devtools.snapshot())
}

#[tauri::command]
async fn devtools_network_clear(
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    state.devtools.clear();
    Ok(())
}

#[tauri::command]
async fn devtools_set_network_recording(
    state: State<'_, Arc<AppState>>,
    recording: bool,
) -> Result<(), String> {
    state.devtools.set_recording(recording);
    Ok(())
}

#[tauri::command]
async fn devtools_get_network_recording(
    state: State<'_, Arc<AppState>>,
) -> Result<bool, String> {
    Ok(state.devtools.is_recording())
}

/// Read localStorage + sessionStorage + cookies from the active tab's
/// content webview. Works by injecting a small JS reader via
/// `webview.eval()` that serializes the storage state into a titan-cmd
/// URL and clicks a synthetic anchor to send it back. The result
/// arrives asynchronously as a `devtools-storage` Tauri event; the UI
/// caller awaits that event after invoking this command.
#[tauri::command]
async fn devtools_read_storage(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let label = active_webview_label(&state).ok_or("No active tab")?;
    let wv = app.get_webview(&label).ok_or("Active tab not found")?;

    // JS reader: collect localStorage + sessionStorage + cookies and
    // POST the result back as JSON via titan-cmd://devtools-storage/
    // The navigation handler parses the body and emits the Tauri
    // event that the UI listens for. We wrap the whole thing in a
    // try/catch so a single exception can't break the reader.
    let reader = r#"(function() {
        function collect(storage) {
            var out = [];
            try {
                for (var i = 0; i < storage.length; i++) {
                    var k = storage.key(i);
                    try { out.push([k, storage.getItem(k)]); }
                    catch (e) { out.push([k, '[unreadable]']); }
                }
            } catch (e) {}
            return out;
        }
        function parseCookies() {
            var out = [];
            try {
                var raw = document.cookie || '';
                if (!raw) return out;
                var parts = raw.split(';');
                for (var i = 0; i < parts.length; i++) {
                    var p = parts[i].trim();
                    var eq = p.indexOf('=');
                    if (eq < 0) out.push([p, '']);
                    else out.push([p.slice(0, eq), p.slice(eq + 1)]);
                }
            } catch (e) {}
            return out;
        }
        var payload;
        try {
            payload = {
                origin: location.origin || '',
                href: location.href || '',
                local: collect(window.localStorage),
                session: collect(window.sessionStorage),
                cookies: parseCookies(),
            };
        } catch (e) {
            payload = { origin: '', href: '', local: [], session: [], cookies: [], error: String(e) };
        }
        var a = document.createElement('a');
        a.href = 'titan-cmd://devtools-storage/' + encodeURIComponent(JSON.stringify(payload));
        a.click();
    })();"#;

    wv.eval(reader).map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a single key from localStorage / sessionStorage / cookies
/// in the active tab's content webview. `kind` must be one of
/// "local", "session", or "cookie".
#[tauri::command]
async fn devtools_delete_storage_key(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    kind: String,
    key: String,
) -> Result<(), String> {
    let label = active_webview_label(&state).ok_or("No active tab")?;
    let wv = app.get_webview(&label).ok_or("Active tab not found")?;
    // Serialize the key through serde so any quotes or backslashes
    // get escaped safely before interpolation.
    let key_json = serde_json::to_string(&key).map_err(|e| e.to_string())?;
    let code = match kind.as_str() {
        "local" => format!("try {{ window.localStorage.removeItem({key_json}); }} catch (e) {{}}"),
        "session" => {
            format!("try {{ window.sessionStorage.removeItem({key_json}); }} catch (e) {{}}")
        }
        "cookie" => {
            // Expire by setting an already-past date on the same path
            format!(
                "try {{ document.cookie = {key_json} + '=; expires=Thu, 01 Jan 1970 00:00:00 GMT; path=/'; }} catch (e) {{}}"
            )
        }
        _ => return Err(format!("unknown storage kind: {kind}")),
    };
    wv.eval(&code).map_err(|e| e.to_string())?;
    Ok(())
}

/// Clear an entire storage kind ("local", "session", or "cookies")
/// in the active tab's content webview. Used by the "Clear all"
/// buttons in the Application tab headers.
#[tauri::command]
async fn devtools_clear_storage(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    kind: String,
) -> Result<(), String> {
    let label = active_webview_label(&state).ok_or("No active tab")?;
    let wv = app.get_webview(&label).ok_or("Active tab not found")?;
    let code = match kind.as_str() {
        "localStorage" => "try { window.localStorage.clear(); } catch (e) {}".to_string(),
        "sessionStorage" => "try { window.sessionStorage.clear(); } catch (e) {}".to_string(),
        "cookies" => {
            // Expire every cookie on the current origin. Best-effort:
            // cookies scoped to a different path won't all clear.
            r#"try {
                var cookies = (document.cookie || '').split(';');
                for (var i = 0; i < cookies.length; i++) {
                    var eq = cookies[i].indexOf('=');
                    var name = eq > -1 ? cookies[i].substr(0, eq).trim() : cookies[i].trim();
                    document.cookie = name + '=; expires=Thu, 01 Jan 1970 00:00:00 GMT; path=/';
                }
            } catch (e) {}"#
                .to_string()
        }
        _ => return Err(format!("unknown storage kind: {kind}")),
    };
    wv.eval(&code).map_err(|e| e.to_string())?;
    Ok(())
}

// ── Site Info ──

#[derive(Debug, Clone, Serialize, Default)]
struct ProfileInfo {
    name: Option<String>,
    display_name: Option<String>,
    about: Option<String>,
    picture: Option<String>,
    nip05: Option<String>,
    lud16: Option<String>,
    website: Option<String>,
    updated_at: u64,
}

#[derive(Debug, Clone, Serialize)]
struct RelayEntry {
    url: String,
    marker: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SiteInfo {
    Name {
        name: String,
        pubkey: String,
        npub: String,
        owner_txid: String,
        owner_vout: u32,
        txid: String,
        block_height: u64,
        profile: Option<ProfileInfo>,
        relays: Vec<RelayEntry>,
    },
    Npub {
        pubkey: String,
        npub: String,
        profile: Option<ProfileInfo>,
        relays: Vec<RelayEntry>,
    },
    Internal,
}

async fn fetch_profile_and_relays(
    resolver: &titan_resolver::Resolver,
    pubkey: &[u8; 32],
) -> (Option<ProfileInfo>, Vec<RelayEntry>) {
    let profile_fut = resolver.fetch_profile(pubkey);
    let relays_fut = resolver.fetch_relay_list_for_pubkey(pubkey);
    let (profile_res, relays_res) = tokio::join!(profile_fut, relays_fut);

    let profile = profile_res.ok().flatten().map(|p| ProfileInfo {
        name: p.name,
        display_name: p.display_name,
        about: p.about,
        picture: p.picture,
        nip05: p.nip05,
        lud16: p.lud16,
        website: p.website,
        updated_at: p.updated_at,
    });

    let relays = relays_res
        .unwrap_or_default()
        .into_iter()
        .map(|r| RelayEntry {
            url: r.url,
            marker: r.marker,
        })
        .collect();

    (profile, relays)
}

#[tauri::command]
async fn get_site_info(
    state: State<'_, Arc<AppState>>,
    url: String,
) -> Result<SiteInfo, String> {
    let cleaned = url.trim().replace("nsite://", "");
    if cleaned.is_empty() || cleaned.starts_with("internal") {
        return Ok(SiteInfo::Internal);
    }

    let host = match cleaned.find('/') {
        Some(i) => &cleaned[..i],
        None => cleaned.as_str(),
    };

    let resolver = state.resolver().await?;

    // If it's an npub, decode it directly
    if host.starts_with("npub1") {
        let pk = decode_npub(host)?;
        let pubkey_hex = hex::encode(pk);
        let (profile, relays) = fetch_profile_and_relays(resolver, &pk).await;
        return Ok(SiteInfo::Npub {
            pubkey: pubkey_hex,
            npub: host.to_string(),
            profile,
            relays,
        });
    }

    // If it's a hex pubkey
    if host.len() == 64 && host.chars().all(|c| c.is_ascii_hexdigit()) {
        use nostr_sdk::prelude::*;
        let bytes = hex::decode(host).map_err(|e| e.to_string())?;
        let mut pk_arr = [0u8; 32];
        pk_arr.copy_from_slice(&bytes);
        let pk = PublicKey::from_slice(&bytes).map_err(|e| e.to_string())?;
        let npub = pk.to_bech32().unwrap_or_else(|_| host.to_string());
        let (profile, relays) = fetch_profile_and_relays(resolver, &pk_arr).await;
        return Ok(SiteInfo::Npub {
            pubkey: host.to_lowercase(),
            npub,
            profile,
            relays,
        });
    }

    // Otherwise try name lookup
    let record = resolver
        .lookup_name_record(host)
        .await
        .map_err(|e| e.to_string())?;

    match record {
        Some(r) => {
            use nostr_sdk::prelude::*;
            let bytes = hex::decode(&r.pubkey_hex).map_err(|e| e.to_string())?;
            let mut pk_arr = [0u8; 32];
            pk_arr.copy_from_slice(&bytes);
            let pk = PublicKey::from_slice(&bytes).map_err(|e| e.to_string())?;
            let npub = pk.to_bech32().unwrap_or_else(|_| r.pubkey_hex.clone());
            let (profile, relays) = fetch_profile_and_relays(resolver, &pk_arr).await;
            Ok(SiteInfo::Name {
                name: r.name,
                pubkey: r.pubkey_hex,
                npub,
                owner_txid: r.owner_txid,
                owner_vout: r.owner_vout,
                txid: r.txid,
                block_height: r.block_height,
                profile,
                relays,
            })
        }
        None => Err(format!("Name '{host}' not found")),
    }
}

// ── Bookmarks ──

fn settings_path(data_dir: &PathBuf) -> PathBuf {
    data_dir.join("settings.json")
}

fn load_settings(data_dir: &PathBuf) -> Settings {
    let path = settings_path(data_dir);
    let mut settings: Settings = match std::fs::read_to_string(&path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => Settings::default(),
    };
    // Clamp the loaded side panel width so a hand-edited settings.json
    // can't render the panel unusable. Values below MIN_SIDE_PANEL_WIDTH
    // (e.g. 0 from a malformed file) get bumped up to the default; the
    // max is a soft cap to stop a runaway value from shoving the
    // content webview entirely off-screen.
    if settings.side_panel_width < MIN_SIDE_PANEL_WIDTH
        || settings.side_panel_width > MAX_SIDE_PANEL_WIDTH
    {
        settings.side_panel_width = DEFAULT_SIDE_PANEL_WIDTH;
    }
    settings
}

fn save_settings(data_dir: &PathBuf, settings: &Settings) {
    let path = settings_path(data_dir);
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(&path, json);
    }
}

/// Build a kind 10003 event from the current bookmark list and publish
/// it to the configured relays. Marks the store as in sync on success.
/// Used by mutating commands and by the startup migration path.
///
/// Failures are logged but not propagated to the user — bookmark
/// edits always succeed locally, and the next online publish will
/// catch up. The store's `pending_publish` flag tracks this state.
async fn publish_bookmarks_to_nostr(state: &Arc<AppState>) {
    if !state.signer.is_unlocked() {
        debug!("publish_bookmarks_to_nostr: signer locked, skipping");
        return;
    }
    let event = match state.bookmarks.build_event(&state.signer) {
        Ok(e) => e,
        Err(e) => {
            warn!("failed to build bookmark event: {e}");
            return;
        }
    };
    let resolver = match state.resolver().await {
        Ok(r) => r,
        Err(e) => {
            warn!("publish_bookmarks_to_nostr: resolver init failed: {e}");
            return;
        }
    };
    match resolver.publish_event(event).await {
        Ok(n) if n > 0 => {
            info!("published bookmark list to {n} relay(s)");
            state.bookmarks.mark_published();
            emit_bookmarks_changed();
        }
        Ok(_) => warn!("bookmark list rejected by all relays"),
        Err(e) => warn!("failed to publish bookmark list: {e}"),
    }
}

/// Background startup sync for bookmarks. Handles legacy migration,
/// pending-publish flush, and remote pull. Logs but never panics; the
/// chrome bookmarks panel always paints from the local cache first.
async fn sync_bookmarks_on_startup(state: Arc<AppState>, outcome: LoadOutcome) {
    // Wait briefly for the resolver / signer to settle. Auto-unlock
    // already happened synchronously in main(), but the relay pool
    // initializes lazily on first .resolver() call.
    if !state.signer.is_unlocked() {
        debug!("bookmark sync: signer not unlocked, skipping");
        return;
    }

    match outcome {
        LoadOutcome::Empty => {
            // Nothing on disk — try to pull a remote list. If there is
            // no remote either, we stay empty.
            pull_remote_bookmarks(&state).await;
        }
        LoadOutcome::Legacy { .. } => {
            // One-time migration: publish the legacy bookmarks as a
            // fresh kind 10003. The store is already marked dirty by
            // load(), so publish_bookmarks_to_nostr will rewrite the
            // file in the new format on success.
            info!("bookmark sync: migrating legacy bookmarks.json to NIP-51 kind 10003");
            publish_bookmarks_to_nostr(&state).await;
        }
        LoadOutcome::NewFormat {
            pending_publish, ..
        } => {
            if pending_publish {
                // We had unpublished local edits from a previous offline
                // session — flush them first.
                info!("bookmark sync: flushing pending local edits");
                publish_bookmarks_to_nostr(&state).await;
            } else {
                // In sync at last shutdown — see if another device has
                // a newer list and merge it in.
                pull_remote_bookmarks(&state).await;
            }
        }
    }
}

/// Fetch the remote kind 10003 event, decrypt it, and replace the local
/// list if the remote is non-empty. Best-effort; failures are logged.
async fn pull_remote_bookmarks(state: &Arc<AppState>) {
    let pubkey_hex = match state.signer.get_pubkey() {
        Some(h) => h,
        None => return,
    };
    let pubkey: [u8; 32] = match hex::decode(&pubkey_hex)
        .ok()
        .and_then(|v| v.try_into().ok())
    {
        Some(p) => p,
        None => {
            warn!("pull_remote_bookmarks: own pubkey not 32 bytes");
            return;
        }
    };

    let resolver = match state.resolver().await {
        Ok(r) => r,
        Err(e) => {
            warn!("pull_remote_bookmarks: resolver init failed: {e}");
            return;
        }
    };

    let event = match resolver
        .fetch_replaceable_event(&pubkey, bookmarks::BOOKMARKS_KIND)
        .await
    {
        Ok(Some(e)) => e,
        Ok(None) => {
            // No remote bookmark list. If we have local bookmarks,
            // this is "local is ahead of remote" — push them up so
            // other devices (and future runs) see them. Common cases:
            //   1. First launch after switching from an older Titan
            //      kind (e.g. 10003 → 10129) where the previous event
            //      is orphaned on relays.
            //   2. User installed Titan on a new device with an
            //      existing nsec that never published bookmarks before.
            //   3. Fresh install with legacy bookmarks.json that was
            //      already migrated on a previous boot.
            // If the local list is also empty, there's genuinely
            // nothing to do — a fresh user on a fresh device.
            let local_count = state.bookmarks.list().len();
            if local_count > 0 {
                info!(
                    "pull_remote_bookmarks: no remote list but {local_count} local bookmark(s); publishing"
                );
                publish_bookmarks_to_nostr(state).await;
            } else {
                debug!("pull_remote_bookmarks: no remote list and no local bookmarks");
            }
            return;
        }
        Err(e) => {
            warn!("pull_remote_bookmarks: relay fetch failed: {e}");
            return;
        }
    };

    let decoded = match BookmarkStore::parse_remote_event(&state.signer, &event) {
        Ok(b) => b,
        Err(e) => {
            warn!("pull_remote_bookmarks: decrypt/parse failed: {e}");
            return;
        }
    };

    info!(
        "pull_remote_bookmarks: pulled {} entries from kind {} (created_at {})",
        decoded.len(),
        bookmarks::BOOKMARKS_KIND,
        event.created_at.as_u64()
    );
    state.bookmarks.replace_from_remote(decoded);
    emit_bookmarks_changed();
}

/// Emit a chrome event to refresh the bookmarks panel if it's open.
/// Best-effort — uses the globally registered AppHandle from log_forward
/// so background tasks don't have to thread one through.
fn emit_bookmarks_changed() {
    if let Some(handle) = log_forward::app_handle() {
        if let Err(e) = handle.emit("bookmarks-changed", ()) {
            debug!("emit bookmarks-changed failed: {e}");
        }
    }
}

#[tauri::command]
async fn add_bookmark(
    state: State<'_, Arc<AppState>>,
    url: String,
    title: String,
) -> Result<(), String> {
    state.bookmarks.add(url, title);
    publish_bookmarks_to_nostr(&state).await;
    Ok(())
}

#[tauri::command]
async fn remove_bookmark(
    state: State<'_, Arc<AppState>>,
    url: String,
) -> Result<(), String> {
    state.bookmarks.remove(&url);
    publish_bookmarks_to_nostr(&state).await;
    Ok(())
}

#[tauri::command]
async fn rename_bookmark(
    state: State<'_, Arc<AppState>>,
    url: String,
    title: String,
) -> Result<(), String> {
    state.bookmarks.rename(&url, title);
    publish_bookmarks_to_nostr(&state).await;
    Ok(())
}

#[tauri::command]
async fn list_bookmarks(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Bookmark>, String> {
    Ok(state.bookmarks.list())
}

#[tauri::command]
async fn is_bookmarked(
    state: State<'_, Arc<AppState>>,
    url: String,
) -> Result<bool, String> {
    Ok(state.bookmarks.contains(&url))
}

// ── Settings Commands ──

#[tauri::command]
async fn get_settings(
    state: State<'_, Arc<AppState>>,
) -> Result<Settings, String> {
    let settings = state.settings.lock().unwrap();
    Ok(settings.clone())
}

#[tauri::command]
async fn update_settings(
    state: State<'_, Arc<AppState>>,
    settings: Settings,
) -> Result<(), String> {
    let mut current = state.settings.lock().unwrap();
    *current = settings;
    save_settings(&state.data_dir, &current);
    Ok(())
}

/// Update just the side panel width. Called from the JS drag-to-resize
/// handler so it can persist the new size without round-tripping the
/// whole Settings struct (which would race with concurrent edits from
/// the Settings panel). The width is clamped to the sane range so a
/// hostile caller can't make the panel invisible or push the content
/// off-screen.
#[tauri::command]
async fn update_side_panel_width(
    state: State<'_, Arc<AppState>>,
    width: u32,
) -> Result<u32, String> {
    let clamped = width.clamp(MIN_SIDE_PANEL_WIDTH, MAX_SIDE_PANEL_WIDTH);
    let mut current = state.settings.lock().unwrap();
    current.side_panel_width = clamped;
    save_settings(&state.data_dir, &current);
    Ok(clamped)
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
    let raw_host = url.host_str()?;
    // On Windows the URL will be http://nsite-content.<host>/<path>
    // (wry workaround). Strip the nsite-content. prefix so the rest of
    // the function sees the original host.
    let host = raw_host.strip_prefix("nsite-content.").unwrap_or(raw_host);
    if host == "internal" {
        return None;
    }

    let path = url.path();

    // {hex}.{name} format — show the name portion as the display URL
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

/// Content Security Policy applied to every `nsite-content://` response.
///
/// Goal: keep the nsite ecosystem functional (pages routinely open Nostr
/// relay websockets, fetch NIP-05 / profile endpoints, load avatars) while
/// blocking the worst exfiltration channels and stopping script/plugin
/// injection.
///
/// - `connect-src 'self' titan-nostr: https: wss:` — `titan-nostr:` keeps
///   the injected `window.nostr` bridge working. `wss:` lets pages open
///   relay sockets directly (titan-nsite is a Next.js app that queries
///   relays from inside the webview — without this, every nsite with its
///   own Nostr client breaks). `https:` allows NIP-05, profile, and
///   Blossom lookups. Plain `http:` is excluded so at least plaintext
///   exfil is blocked.
/// - `script-src 'self' 'unsafe-inline' 'unsafe-eval'` — no external
///   scripts. Bundled sites (Next.js, Vite, etc.) need inline + eval.
/// - `img-src 'self' data: blob: https:` — allows external HTTPS images
///   (profile avatars, OG images). Tracking-pixel risk is accepted here
///   because we can't tell apart a legitimate avatar from a tracker.
/// - `object-src 'none'` / `frame-src 'self'` — no plugins, no cross-site
///   framing.
/// - `base-uri 'self'` / `form-action 'self'` — prevent base-tag hijack
///   and form-based exfil.
///
/// What this does NOT protect against: a compromised nsite can still
/// exfiltrate data by broadcasting it through a public relay or by
/// issuing an HTTPS request to an attacker server. The signer prompt
/// remains the last line of defense for sensitive event content — the
/// user must approve each `signEvent` that touches anything private.
const NSITE_CONTENT_CSP: &str = "default-src 'self'; \
    script-src 'self' 'unsafe-inline' 'unsafe-eval'; \
    style-src 'self' 'unsafe-inline'; \
    img-src 'self' data: blob: https:; \
    font-src 'self' data: https:; \
    connect-src 'self' titan-nostr: https: wss:; \
    frame-src 'self'; \
    object-src 'none'; \
    base-uri 'self'; \
    form-action 'self'";

/// Apply the same defense-in-depth security headers to every response from
/// the `nsite-content://` protocol handler. Centralized so we can't forget
/// one on a new response path.
///
/// Headers:
/// - `Content-Security-Policy`: see `NSITE_CONTENT_CSP`
/// - `X-Content-Type-Options: nosniff`: blocks MIME sniffing — a malicious
///   nsite can't label HTML as PNG and have the browser execute it
/// - `Referrer-Policy: no-referrer`: outbound links (to mempool.space etc.)
///   don't leak the nsite pubkey/host in the Referer header
/// - `Permissions-Policy`: disable powerful browser APIs by default
/// - `X-Frame-Options: SAMEORIGIN`: legacy clickjacking defense (redundant
///   with CSP `frame-src`, kept for older renderers)
fn apply_nsite_content_headers(
    builder: tauri::http::response::Builder,
) -> tauri::http::response::Builder {
    builder
        .header("content-security-policy", NSITE_CONTENT_CSP)
        .header("x-content-type-options", "nosniff")
        .header("referrer-policy", "no-referrer")
        .header(
            "permissions-policy",
            "camera=(), microphone=(), geolocation=(), payment=(), usb=(), \
             magnetometer=(), gyroscope=(), accelerometer=(), autoplay=(), \
             fullscreen=(self), midi=(), publickey-credentials-get=()",
        )
        .header("x-frame-options", "SAMEORIGIN")
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

fn bookmarks_page(bookmarks: &[Bookmark]) -> Vec<u8> {
    let mut items = String::new();
    if bookmarks.is_empty() {
        items.push_str(r#"<p style="color:#5a5348;text-align:center;padding:32px;">No bookmarks yet. Click the ☆ in the toolbar to save a site.</p>"#);
    } else {
        for b in bookmarks {
            items.push_str(&format!(
                r#"<a href="nsite://{url}" class="item">
                    <div class="title">{title}</div>
                    <div class="url">nsite://{url}</div>
                </a>"#,
                url = html_escape(&b.url),
                title = html_escape(&b.title),
            ));
        }
    }

    format!(r#"<!DOCTYPE html>
<html><head><meta charset="UTF-8"><style>
body {{ margin:0; background:#0a0a0a; color:#e8e0d4; font-family:-apple-system,system-ui,sans-serif; }}
.container {{ max-width:600px; margin:0 auto; padding:32px 24px; }}
h1 {{ font-size:14px; font-weight:400; color:#5a5348; letter-spacing:2px; text-transform:uppercase; margin-bottom:24px; }}
.item {{ display:block; padding:12px 16px; background:#131313; border:1px solid #221e1a; border-radius:6px;
  margin-bottom:8px; text-decoration:none; transition:border-color 0.15s; cursor:pointer; }}
.item:hover {{ border-color:#a06800; }}
.title {{ font-size:15px; color:#d48f00; margin-bottom:4px; }}
.url {{ font-size:12px; color:#5a5348; font-family:"SF Mono","Fira Code",monospace; }}
</style></head><body>
<div class="container">
<h1>Bookmarks</h1>
{items}
</div>
</body></html>"#, items = items).into_bytes()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            // Need exactly two hex digits after the `%`. If either byte
            // is missing or isn't a valid hex pair, emit the `%` and the
            // partial bytes literally.
            let hi = match chars.next() {
                Some(c) => c,
                None => {
                    result.push('%');
                    break;
                }
            };
            let lo = match chars.next() {
                Some(c) => c,
                None => {
                    result.push('%');
                    result.push(hi as char);
                    break;
                }
            };
            let hex = [hi, lo];
            if let Ok(s) = std::str::from_utf8(&hex) {
                if let Ok(val) = u8::from_str_radix(s, 16) {
                    result.push(val as char);
                    continue;
                }
            }
            result.push('%');
            result.push(hi as char);
            result.push(lo as char);
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

// ── Tab Commands ──

#[tauri::command]
async fn create_tab(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<TabsPayload, String> {
    let new_id = {
        let mut next = state.next_tab_id.lock().unwrap();
        let id = *next;
        *next += 1;
        id
    };
    let label = format!("tab-{}", new_id);

    // Hide current active tab
    if let Some(old_label) = active_webview_label(&state) {
        if let Some(wv) = app.get_webview(&old_label) {
            let _ = wv.set_size(tauri::LogicalSize::new(0.0, 0.0));
        }
    }

    // Create new webview
    let window = app.get_window("main").ok_or("No main window")?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let phys = window.inner_size().map_err(|e| e.to_string())?;
    let lw = phys.width as f64 / scale;
    let lh = phys.height as f64 / scale;
    let content_top = 82.0;

    create_tab_webview(
        &window, &app, &label,
        "nsite-content://internal/welcome",
        content_top, lw, lh - content_top,
    ).map_err(|e| e.to_string())?;

    let tab = Tab {
        id: new_id,
        label,
        display_url: String::new(),
        title: "New Tab".to_string(),
    };

    {
        let mut tabs = state.tabs.lock().unwrap();
        tabs.push(tab);
        *state.active_tab.lock().unwrap() = new_id;
    }

    // JS will call navigate() after createTab returns
    get_tabs_payload(&state)
}

#[tauri::command]
async fn close_tab(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    tab_id: u32,
) -> Result<TabsPayload, String> {
    let tabs_len = state.tabs.lock().unwrap().len();
    if tabs_len <= 1 {
        return get_tabs_payload(&state);
    }

    // Find the tab to close and determine new active
    let (label_to_close, new_active_id) = {
        let tabs = state.tabs.lock().unwrap();
        let active = *state.active_tab.lock().unwrap();
        let idx = tabs.iter().position(|t| t.id == tab_id).ok_or("Tab not found")?;
        let label = tabs[idx].label.clone();

        let new_active = if tab_id == active {
            // Switch to the next tab, or previous if closing the last
            if idx + 1 < tabs.len() {
                tabs[idx + 1].id
            } else {
                tabs[idx - 1].id
            }
        } else {
            active
        };
        (label, new_active)
    };

    // Destroy the webview
    if let Some(wv) = app.get_webview(&label_to_close) {
        let _ = wv.close();
    }

    // Remove from state and set new active
    {
        let mut tabs = state.tabs.lock().unwrap();
        tabs.retain(|t| t.id != tab_id);
        *state.active_tab.lock().unwrap() = new_active_id;
    }

    // Show the new active tab
    if let Some(label) = active_webview_label(&state) {
        if let Some(wv) = app.get_webview(&label) {
            let window = app.get_window("main").ok_or("No main window")?;
            let scale = window.scale_factor().unwrap_or(1.0);
            let phys = window.inner_size().map_err(|e| e.to_string())?;
            let lw = phys.width as f64 / scale;
            let lh = phys.height as f64 / scale;
            let content_top = 82.0;
            let _ = wv.set_position(tauri::LogicalPosition::new(0.0, content_top));
            let _ = wv.set_size(tauri::LogicalSize::new(lw, lh - content_top));
        }
    }

    get_tabs_payload(&state)
}

#[tauri::command]
async fn switch_tab(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    tab_id: u32,
) -> Result<TabsPayload, String> {
    let current_active = *state.active_tab.lock().unwrap();
    if tab_id == current_active {
        return get_tabs_payload(&state);
    }

    // Hide old active
    if let Some(old_label) = active_webview_label(&state) {
        if let Some(wv) = app.get_webview(&old_label) {
            let _ = wv.set_size(tauri::LogicalSize::new(0.0, 0.0));
        }
    }

    *state.active_tab.lock().unwrap() = tab_id;

    // Show new active
    if let Some(label) = active_webview_label(&state) {
        if let Some(wv) = app.get_webview(&label) {
            let window = app.get_window("main").ok_or("No main window")?;
            let scale = window.scale_factor().unwrap_or(1.0);
            let phys = window.inner_size().map_err(|e| e.to_string())?;
            let lw = phys.width as f64 / scale;
            let lh = phys.height as f64 / scale;
            let content_top = 82.0;
            let _ = wv.set_position(tauri::LogicalPosition::new(0.0, content_top));
            let _ = wv.set_size(tauri::LogicalSize::new(lw, lh - content_top));
        }
    }

    get_tabs_payload(&state)
}

#[tauri::command]
async fn get_tabs(
    state: State<'_, Arc<AppState>>,
) -> Result<TabsPayload, String> {
    get_tabs_payload(&state)
}

fn get_tabs_payload(state: &AppState) -> Result<TabsPayload, String> {
    let tabs = state.tabs.lock().unwrap().clone();
    let active_tab = *state.active_tab.lock().unwrap();
    Ok(TabsPayload { tabs, active_tab })
}

// ── Webview Factory ──

fn create_tab_webview(
    window: &tauri::Window,
    app_handle: &tauri::AppHandle,
    label: &str,
    url: &str,
    top: f64,
    width: f64,
    height: f64,
) -> Result<tauri::Webview, Box<dyn std::error::Error>> {
    let handle1 = app_handle.clone();
    let handle2 = app_handle.clone();
    let label_nav = label.to_string();
    let label_load = label.to_string();

    let webview = window.add_child(
        tauri::webview::WebviewBuilder::new(
            label,
            tauri::WebviewUrl::External(url.parse()?),
        )
        .initialization_script(WINDOW_NOSTR_INJECTION)
        .on_navigation(move |url| {
            let scheme = url.scheme();

            if scheme == "nsite-content" {
                return true;
            }

            // Windows WebView2 workaround: wry rewrites nsite-content://
            // URLs to http://nsite-content.<host>/<path>. Allow those
            // navigations through so the internal browser can load them.
            if (scheme == "http" || scheme == "https")
                && url.host_str()
                    .map(|h| h == "nsite-content" || h.starts_with("nsite-content."))
                    .unwrap_or(false)
            {
                return true;
            }

            if scheme == "titan-cmd" {
                let cmd = url.host_str().unwrap_or("");
                let handle = handle1.clone();
                match cmd {
                    "console" => { let _ = handle.emit("open-panel", "console"); }
                    "focus-address-bar" => { let _ = handle.emit("focus-address-bar", ()); }
                    "toggle-bookmark" => { let _ = handle.emit("toggle-bookmark", ()); }
                    "new-tab" => { let _ = handle.emit("new-tab", ()); }
                    "close-tab" => { let _ = handle.emit("close-tab", ()); }
                    "console-msg" => {
                        // Console message from content: titan-cmd://console-msg/<level>/<encoded-message>
                        let path = url.path();
                        let parts: Vec<&str> = path.splitn(3, '/').collect();
                        if parts.len() >= 3 {
                            let level = parts[1].to_string();
                            let message = url_decode(parts[2]);
                            let tab = label_nav.clone();
                            let _ = handle.emit("console-message", ConsolePayload {
                                level,
                                message,
                                tab_label: tab,
                            });
                        }
                    }
                    "console-result" => {
                        // Eval result from content: titan-cmd://console-result/<level>/<encoded>
                        let path = url.path();
                        let parts: Vec<&str> = path.splitn(3, '/').collect();
                        if parts.len() >= 3 {
                            let level = parts[1].to_string();
                            let message = url_decode(parts[2]);
                            let tab = label_nav.clone();
                            let _ = handle.emit("console-result", ConsolePayload {
                                level,
                                message,
                                tab_label: tab,
                            });
                        }
                    }
                    "net-event" => {
                        // Network capture from injected JS wrapper:
                        // titan-cmd://net-event/<encoded-json>
                        // The payload is a JSON object with url/method/
                        // status/etc. We parse it, stamp the tab label,
                        // and push into the devtools ring buffer. If
                        // the console panel is open with the Network
                        // tab active, the UI listens for a follow-up
                        // devtools-network-updated event and repaints.
                        let path = url.path();
                        let encoded = path.strip_prefix('/').unwrap_or(path);
                        let json = url_decode(encoded);
                        if let Some(state) = handle.try_state::<Arc<AppState>>() {
                            if let Some(event) = devtools::parse_js_event(&json, &label_nav) {
                                state.devtools.record_event(event);
                                let _ = handle.emit("devtools-network-updated", ());
                            }
                        }
                    }
                    "devtools-storage" => {
                        // Storage reader response:
                        // titan-cmd://devtools-storage/<encoded-json>
                        // The payload is already the full snapshot
                        // object that the UI wants; we just forward
                        // it as the `devtools-storage` Tauri event.
                        let path = url.path();
                        let encoded = path.strip_prefix('/').unwrap_or(path);
                        let json = url_decode(encoded);
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
                            let _ = handle.emit("devtools-storage", value);
                        }
                    }
                    c if c.starts_with("tab-") => {
                        if let Ok(n) = c[4..].parse::<u32>() {
                            let _ = handle.emit("switch-tab-number", n);
                        }
                    }
                    _ => {}
                }
                return false;
            }

            if scheme == "nsite" {
                let url_str = url.to_string();
                let handle = handle1.clone();
                let wv_label = label_nav.clone();
                tauri::async_runtime::spawn(async move {
                    info!("intercepted nsite:// link: {url_str}");
                    let cleaned = url_str.replace("nsite://", "");
                    if let Some(content_wv) = handle.get_webview(&wv_label) {
                        let loading_url = platform_navigate_url("nsite-content://internal/loading");
                        let _ = content_wv.navigate(loading_url.parse().unwrap());
                    }
                    let _ = handle.emit("nsite-link-clicked", &cleaned);
                });
                return false;
            }

            debug!("blocked navigation to {url}");
            false
        })
        .on_page_load(move |webview, payload| {
            if let tauri::webview::PageLoadEvent::Finished = payload.event() {
                let url = payload.url();
                if let Some(display) = content_url_to_display(url) {
                    let _ = handle2.emit("page-loaded", PageLoadedPayload {
                        tab_label: label_load.clone(),
                        url: display,
                    });
                }
                let _ = webview.eval(r#"
                    // Keyboard shortcuts
                    document.addEventListener('keydown', function(e) {
                        var cmd = null;
                        if ((e.metaKey && e.altKey && e.code === 'KeyK') ||
                            (e.ctrlKey && e.shiftKey && e.code === 'KeyK')) cmd = 'console';
                        if ((e.metaKey || e.ctrlKey) && e.code === 'KeyL') cmd = 'focus-address-bar';
                        if ((e.metaKey || e.ctrlKey) && e.code === 'KeyD') cmd = 'toggle-bookmark';
                        if ((e.metaKey || e.ctrlKey) && e.code === 'KeyT') cmd = 'new-tab';
                        if ((e.metaKey || e.ctrlKey) && e.code === 'KeyW') cmd = 'close-tab';
                        if ((e.metaKey || e.ctrlKey) && e.key >= '1' && e.key <= '9') cmd = 'tab-' + e.key;
                        if (cmd) {
                            e.preventDefault();
                            var a = document.createElement('a');
                            a.href = 'titan-cmd://' + cmd;
                            a.click();
                        }
                    });

                    // Console message forwarding
                    (function() {
                        if (window.__titanConsoleHooked) return;
                        window.__titanConsoleHooked = true;

                        function fwd(level, args) {
                            try {
                                var msg = Array.prototype.map.call(args, function(a) {
                                    if (typeof a === 'string') return a;
                                    try { return JSON.stringify(a); } catch(_) { return String(a); }
                                }).join(' ');
                                var a = document.createElement('a');
                                a.href = 'titan-cmd://console-msg/' + level + '/' + encodeURIComponent(msg);
                                a.click();
                            } catch(_) {}
                        }

                        var origLog = console.log;
                        var origWarn = console.warn;
                        var origError = console.error;
                        var origInfo = console.info;
                        var origDebug = console.debug;

                        console.log = function() { origLog.apply(console, arguments); fwd('info', arguments); };
                        console.info = function() { origInfo.apply(console, arguments); fwd('info', arguments); };
                        console.warn = function() { origWarn.apply(console, arguments); fwd('warn', arguments); };
                        console.error = function() { origError.apply(console, arguments); fwd('error', arguments); };
                        console.debug = function() { origDebug.apply(console, arguments); fwd('debug', arguments); };

                        window.addEventListener('error', function(e) {
                            fwd('error', [e.message + ' at ' + (e.filename || '') + ':' + (e.lineno || '')]);
                        });

                        window.addEventListener('unhandledrejection', function(e) {
                            fwd('error', ['Unhandled rejection: ' + (e.reason || '')]);
                        });
                    })();

                    // ── Network capture ──
                    //
                    // Wraps fetch / XMLHttpRequest / WebSocket at page
                    // load so the Network tab can see what the page is
                    // doing. Wrappers report metadata (URL, method,
                    // status, timing, headers) back to chrome via
                    // titan-cmd://net-event/<encoded-json>. Response
                    // bodies are NOT captured — cloning every response
                    // would double memory for every request and we
                    // don't have a UX for large body previews yet.
                    //
                    // This always runs regardless of whether the user
                    // has the Network tab open — the overhead is a
                    // ~microsecond per wrapped call and the chrome
                    // side enforces the recording toggle + ring buffer.
                    (function() {
                        if (window.__titanNetHooked) return;
                        window.__titanNetHooked = true;

                        // Report a completed event to chrome. We build
                        // an anchor and click it rather than calling
                        // fetch(), so we don't recursively capture our
                        // own reports.
                        function report(event) {
                            try {
                                var json = JSON.stringify(event);
                                // Truncate really huge bodies before
                                // encoding so titan-cmd URLs don't
                                // blow up (the Rust side also caps).
                                if (event.request_body && event.request_body.length > 8192) {
                                    event.request_body = event.request_body.slice(0, 8192) + '...[truncated]';
                                    json = JSON.stringify(event);
                                }
                                var a = document.createElement('a');
                                a.href = 'titan-cmd://net-event/' + encodeURIComponent(json);
                                a.click();
                            } catch (_) {}
                        }

                        // Guess a resource type from a URL + Content-Type.
                        // Mirrors the Rust-side classifier.
                        function classifyType(url, contentType) {
                            var ct = (contentType || '').toLowerCase();
                            if (ct.indexOf('text/html') === 0) return 'document';
                            if (ct.indexOf('text/css') === 0) return 'stylesheet';
                            if (ct.indexOf('javascript') !== -1) return 'script';
                            if (ct.indexOf('image/') === 0) return 'image';
                            if (ct.indexOf('font/') === 0 || ct.indexOf('font') !== -1) return 'font';
                            if (ct.indexOf('application/json') === 0) return 'fetch';
                            // Fall back to URL extension
                            try {
                                var u = new URL(url, location.href);
                                var p = u.pathname.toLowerCase();
                                if (/\.(html?|htm)$/.test(p)) return 'document';
                                if (/\.css$/.test(p)) return 'stylesheet';
                                if (/\.(js|mjs)$/.test(p)) return 'script';
                                if (/\.(png|jpe?g|gif|svg|webp|ico)$/.test(p)) return 'image';
                                if (/\.(woff2?|ttf|otf)$/.test(p)) return 'font';
                            } catch (_) {}
                            return 'fetch';
                        }

                        // Extract an absolute URL from a fetch() input
                        // (string or Request object).
                        function inputToUrl(input) {
                            try {
                                if (typeof input === 'string') return new URL(input, location.href).href;
                                if (input && input.url) return input.url;
                            } catch (_) {}
                            return String(input);
                        }

                        // ── fetch wrapper ──
                        var origFetch = window.fetch;
                        if (typeof origFetch === 'function') {
                            window.fetch = function(input, init) {
                                var started = performance.now();
                                var url = inputToUrl(input);
                                var method = ((init && init.method) ||
                                    (input && input.method) || 'GET').toUpperCase();

                                // Capture outgoing headers if present.
                                var reqHeaders = [];
                                try {
                                    if (init && init.headers) {
                                        if (init.headers instanceof Headers) {
                                            init.headers.forEach(function(v, k) { reqHeaders.push([k, v]); });
                                        } else if (Array.isArray(init.headers)) {
                                            reqHeaders = init.headers.slice();
                                        } else if (typeof init.headers === 'object') {
                                            for (var k in init.headers) reqHeaders.push([k, init.headers[k]]);
                                        }
                                    }
                                } catch (_) {}

                                // Capture request body (string only for
                                // copy-as-cURL reconstruction).
                                var reqBody = null;
                                try {
                                    if (init && typeof init.body === 'string') reqBody = init.body;
                                } catch (_) {}

                                return origFetch.apply(this, arguments).then(function(resp) {
                                    var duration = Math.round(performance.now() - started);
                                    var respHeaders = [];
                                    try {
                                        resp.headers.forEach(function(v, k) { respHeaders.push([k, v]); });
                                    } catch (_) {}
                                    var ct = '';
                                    try { ct = resp.headers.get('content-type') || ''; } catch (_) {}
                                    report({
                                        method: method,
                                        url: url,
                                        status: resp.status,
                                        resource_type: classifyType(url, ct),
                                        duration_ms: duration,
                                        request_headers: reqHeaders,
                                        response_headers: respHeaders,
                                        request_body: reqBody,
                                    });
                                    return resp;
                                }, function(err) {
                                    var duration = Math.round(performance.now() - started);
                                    report({
                                        method: method,
                                        url: url,
                                        status: 0,
                                        resource_type: 'fetch',
                                        duration_ms: duration,
                                        request_headers: reqHeaders,
                                        request_body: reqBody,
                                        error: String(err && err.message || err),
                                    });
                                    throw err;
                                });
                            };
                        }

                        // ── XHR wrapper ──
                        var origOpen = window.XMLHttpRequest.prototype.open;
                        var origSend = window.XMLHttpRequest.prototype.send;
                        var origSetHeader = window.XMLHttpRequest.prototype.setRequestHeader;

                        window.XMLHttpRequest.prototype.open = function(method, url) {
                            this.__titanNet = {
                                method: (method || 'GET').toUpperCase(),
                                url: (function() {
                                    try { return new URL(url, location.href).href; }
                                    catch (_) { return String(url); }
                                })(),
                                started: 0,
                                req_headers: [],
                                body: null,
                            };
                            return origOpen.apply(this, arguments);
                        };

                        window.XMLHttpRequest.prototype.setRequestHeader = function(k, v) {
                            if (this.__titanNet) this.__titanNet.req_headers.push([k, v]);
                            return origSetHeader.apply(this, arguments);
                        };

                        window.XMLHttpRequest.prototype.send = function(body) {
                            var self = this;
                            var meta = self.__titanNet;
                            if (meta) {
                                meta.started = performance.now();
                                if (typeof body === 'string') meta.body = body;
                                self.addEventListener('loadend', function() {
                                    var respHeaders = [];
                                    try {
                                        var raw = self.getAllResponseHeaders() || '';
                                        raw.trim().split(/\r?\n/).forEach(function(line) {
                                            var idx = line.indexOf(':');
                                            if (idx > 0) respHeaders.push([line.slice(0, idx).trim(), line.slice(idx + 1).trim()]);
                                        });
                                    } catch (_) {}
                                    var ct = '';
                                    try { ct = self.getResponseHeader('content-type') || ''; } catch (_) {}
                                    report({
                                        method: meta.method,
                                        url: meta.url,
                                        status: self.status || 0,
                                        resource_type: classifyType(meta.url, ct),
                                        duration_ms: Math.round(performance.now() - meta.started),
                                        request_headers: meta.req_headers,
                                        response_headers: respHeaders,
                                        request_body: meta.body,
                                        error: (self.status === 0 ? 'network error or blocked' : null),
                                    });
                                });
                            }
                            return origSend.apply(this, arguments);
                        };

                        // ── WebSocket wrapper ──
                        var OrigWebSocket = window.WebSocket;
                        if (typeof OrigWebSocket === 'function') {
                            function TitanWebSocket(url, protocols) {
                                var started = performance.now();
                                var absUrl;
                                try { absUrl = new URL(url, location.href).href; }
                                catch (_) { absUrl = String(url); }

                                var ws = protocols !== undefined
                                    ? new OrigWebSocket(url, protocols)
                                    : new OrigWebSocket(url);

                                ws.addEventListener('open', function() {
                                    report({
                                        method: 'WS',
                                        url: absUrl,
                                        status: 101,
                                        resource_type: 'websocket',
                                        duration_ms: Math.round(performance.now() - started),
                                    });
                                });
                                ws.addEventListener('error', function() {
                                    report({
                                        method: 'WS',
                                        url: absUrl,
                                        status: 0,
                                        resource_type: 'websocket',
                                        duration_ms: Math.round(performance.now() - started),
                                        error: 'WebSocket error',
                                    });
                                });
                                return ws;
                            }
                            // Preserve prototype so instanceof checks keep working
                            TitanWebSocket.prototype = OrigWebSocket.prototype;
                            TitanWebSocket.CONNECTING = OrigWebSocket.CONNECTING;
                            TitanWebSocket.OPEN = OrigWebSocket.OPEN;
                            TitanWebSocket.CLOSING = OrigWebSocket.CLOSING;
                            TitanWebSocket.CLOSED = OrigWebSocket.CLOSED;
                            window.WebSocket = TitanWebSocket;
                        }
                    })();
                "#);
            }
        }),
        tauri::LogicalPosition::new(0.0, top),
        tauri::LogicalSize::new(width, height),
    )?;

    Ok(webview)
}

// ── Main ──

fn main() {
    use tracing_subscriber::prelude::*;
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "titan=info".parse().unwrap());
    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(log_forward::ChromeLogLayer)
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

    let _ = std::fs::create_dir_all(&data_dir);
    let (bookmark_store, bookmark_load_outcome) = BookmarkStore::load(data_dir.clone());
    let settings = load_settings(&data_dir);
    info!(
        "loaded {} bookmark(s), settings from {}",
        bookmark_store.list().len(),
        data_dir.display()
    );

    let first_tab = Tab {
        id: 0,
        label: "tab-0".to_string(),
        display_url: String::new(),
        title: "New Tab".to_string(),
    };

    let signer = Signer::new();
    // Auto-unlock if an identity exists in the keychain. The OS keychain
    // already gates access at the user level (login keychain on macOS,
    // Secret Service on Linux, Credential Manager on Windows), so a
    // separate in-app unlock step adds friction without adding security.
    // Manual lock is still available via the signer panel.
    if signer.has_identity() && !signer.is_unlocked() {
        match signer.unlock() {
            Ok(pubkey) => info!("signer auto-unlocked, pubkey={pubkey}"),
            Err(e) => warn!("signer auto-unlock failed: {e}"),
        }
    }
    info!(
        "signer: has_identity={}, unlocked={}",
        signer.has_identity(),
        signer.is_unlocked()
    );

    let permissions = Permissions::load(data_dir.clone());

    let state = Arc::new(AppState {
        resolver: OnceCell::new(),
        cache_dir,
        data_dir,
        bookmarks: bookmark_store,
        settings: std::sync::Mutex::new(settings),
        tabs: std::sync::Mutex::new(vec![first_tab]),
        active_tab: std::sync::Mutex::new(0),
        next_tab_id: std::sync::Mutex::new(1),
        signer,
        permissions,
        prompt_queue: PromptQueue::new(),
        audit_log: AuditLog::new(),
        devtools: devtools::DevtoolsState::new(),
    });

    // Bookmark sync: spawn a one-shot startup task that handles three
    // cases:
    //   1. Legacy v0.1.4 file → publish a fresh kind 10003 (one-time
    //      migration), then mark in sync.
    //   2. New-format file with `pending_publish: true` → an earlier
    //      session edited bookmarks while offline; flush them now.
    //   3. New-format file in sync → fetch the latest kind 10003 from
    //      relays and merge in case another device updated it.
    //
    // Runs in the background so it doesn't block window creation. The
    // bookmarks bar paints from the local cache immediately; the Nostr
    // sync just refreshes it.
    {
        let state = state.clone();
        let outcome_summary = match &bookmark_load_outcome {
            LoadOutcome::Empty => "empty",
            LoadOutcome::NewFormat { .. } => "new",
            LoadOutcome::Legacy { .. } => "legacy",
        };
        info!("bookmark sync: load outcome = {outcome_summary}");
        tauri::async_runtime::spawn(async move {
            sync_bookmarks_on_startup(state, bookmark_load_outcome).await;
        });
    }

    let protocol_state = state.clone();
    let state_for_nostr = state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state)
        .register_asynchronous_uri_scheme_protocol("nsite-content", move |_ctx, request, responder| {
            let state = protocol_state.clone();

            tauri::async_runtime::spawn(async move {
                // Capture request metadata upfront for devtools recording.
                // Every return branch below records a completed event so
                // the Network tab sees the full picture regardless of
                // which code path served the response.
                let started = std::time::Instant::now();
                let uri = request.uri();
                let devtools_url = uri.to_string();
                let devtools_method = request.method().as_str().to_string();
                let host = uri.host().unwrap_or("internal");
                let path = uri.path();
                let path = if path.is_empty() { "/" } else { path };

                debug!("protocol: {host}{path}");

                // Closure that builds + records a NetworkEvent for this
                // protocol handler response. Captures by value so it can
                // be called from any branch below without reborrowing.
                let record = |status: u16, resource_type: &str| {
                    let duration_ms = started.elapsed().as_millis() as u64;
                    state.devtools.record_event(devtools::NetworkEvent {
                        id: 0,
                        timestamp_ms: 0,
                        tab_label: String::new(),
                        method: devtools_method.clone(),
                        url: devtools_url.clone(),
                        status,
                        resource_type: resource_type.to_string(),
                        duration_ms: Some(duration_ms),
                        request_headers: vec![],
                        response_headers: vec![
                            ("x-titan-scheme".to_string(), "nsite-content".to_string()),
                        ],
                        request_body: None,
                        source: "rust".to_string(),
                        error: None,
                    });
                };

                // Internal pages
                if host == "internal" {
                    let (body, ct) = match path {
                        "/welcome" | "/" => (welcome_page(), "text/html"),
                        "/loading" => (loading_page(), "text/html"),
                        "/bookmarks" => {
                            let bookmarks = state.bookmarks.list();
                            (bookmarks_page(&bookmarks), "text/html")
                        }
                        p if p.starts_with("/error") => {
                            let msg_raw = uri.query()
                                .and_then(|q| q.strip_prefix("msg="))
                                .unwrap_or("Unknown error");
                            let msg = url_decode(msg_raw);
                            (error_page(&msg), "text/html")
                        }
                        _ => (welcome_page(), "text/html"),
                    };
                    record(200, "internal");
                    responder.respond(
                        apply_nsite_content_headers(
                            tauri::http::Response::builder()
                                .status(200)
                                .header("content-type", ct),
                        )
                        .body(body)
                        .unwrap(),
                    );
                    return;
                }

                // Parse site identity from host
                let (pubkey, site_name) = match parse_content_host(host) {
                    Some(p) => p,
                    None => {
                        record(404, "nsite-content");
                        responder.respond(
                            apply_nsite_content_headers(
                                tauri::http::Response::builder()
                                    .status(404)
                                    .header("content-type", "text/html"),
                            )
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
                        record(500, "nsite-content");
                        responder.respond(
                            apply_nsite_content_headers(
                                tauri::http::Response::builder()
                                    .status(500)
                                    .header("content-type", "text/html"),
                            )
                            .body(error_page(&e))
                            .unwrap(),
                        );
                        return;
                    }
                };

                match resolver.resolve(&pubkey, path, site_name.as_deref()).await {
                    Ok(content) => {
                        let content_type = guess_content_type(path);
                        // Classify by content type for the devtools
                        // Type column: document/script/stylesheet/image.
                        let resource_type = if content_type.starts_with("text/html") {
                            "document"
                        } else if content_type.starts_with("text/css") {
                            "stylesheet"
                        } else if content_type.contains("javascript") {
                            "script"
                        } else if content_type.starts_with("image/") {
                            "image"
                        } else if content_type.starts_with("font/")
                            || content_type.contains("font")
                        {
                            "font"
                        } else {
                            "other"
                        };
                        record(200, resource_type);
                        responder.respond(
                            apply_nsite_content_headers(
                                tauri::http::Response::builder()
                                    .status(200)
                                    .header("content-type", &content_type)
                                    .header("access-control-allow-origin", "*"),
                            )
                            .body(content.data)
                            .unwrap(),
                        );
                    }
                    Err(e) => {
                        warn!("failed to resolve {host}{path}: {e}");
                        record(404, "nsite-content");
                        responder.respond(
                            apply_nsite_content_headers(
                                tauri::http::Response::builder()
                                    .status(404)
                                    .header("content-type", "text/html"),
                            )
                            .body(error_page(&format!("{e}")))
                            .unwrap(),
                        );
                    }
                }
            });
        })
        .register_asynchronous_uri_scheme_protocol("titan-nostr", {
            let state = state_for_nostr.clone();
            move |ctx, request, responder| {
                let state = state.clone();
                let app = ctx.app_handle().clone();
                let webview_label = ctx.webview_label().to_string();
                tauri::async_runtime::spawn(async move {
                    // Devtools capture: record every NIP-07 bridge call
                    // so the Network tab shows what the page is asking
                    // the signer for. Matches the nsite-content handler
                    // pattern.
                    let started = std::time::Instant::now();
                    let devtools_url = request.uri().to_string();
                    let devtools_method = request.method().as_str().to_string();
                    let body_bytes = request.body().to_vec();
                    // Truncate the request body preview before recording
                    // so dev-console memory doesn't balloon on large
                    // signEvent payloads. 8 KiB is plenty to see what
                    // the page is signing.
                    let body_preview = if body_bytes.len() <= 8192 {
                        String::from_utf8_lossy(&body_bytes).to_string()
                    } else {
                        format!(
                            "{}...[truncated]",
                            String::from_utf8_lossy(&body_bytes[..8192])
                        )
                    };

                    let respond_json = |value: serde_json::Value, status: u16| {
                        let body = serde_json::to_vec(&value).unwrap_or_default();
                        tauri::http::Response::builder()
                            .status(status)
                            .header("content-type", "application/json")
                            .header("access-control-allow-origin", "*")
                            .header("access-control-allow-headers", "content-type")
                            .header("access-control-allow-methods", "POST, OPTIONS")
                            .body(body)
                            .unwrap()
                    };

                    let record_devtools = |status: u16, error: Option<String>| {
                        let duration_ms = started.elapsed().as_millis() as u64;
                        state.devtools.record_event(devtools::NetworkEvent {
                            id: 0,
                            timestamp_ms: 0,
                            tab_label: webview_label.clone(),
                            method: devtools_method.clone(),
                            url: devtools_url.clone(),
                            status,
                            resource_type: "titan-nostr".to_string(),
                            duration_ms: Some(duration_ms),
                            request_headers: vec![(
                                "content-type".to_string(),
                                "application/json".to_string(),
                            )],
                            response_headers: vec![(
                                "x-titan-scheme".to_string(),
                                "titan-nostr".to_string(),
                            )],
                            request_body: Some(body_preview.clone()),
                            source: "rust".to_string(),
                            error,
                        });
                    };

                    // OPTIONS preflight
                    if request.method() == "OPTIONS" {
                        record_devtools(204, None);
                        responder.respond(respond_json(serde_json::json!({}), 204));
                        return;
                    }

                    // Parse the request JSON
                    let req: nip07::NostrRequest = match serde_json::from_slice(&body_bytes) {
                        Ok(r) => r,
                        Err(e) => {
                            record_devtools(400, Some(format!("invalid request: {e}")));
                            let err = nip07::NostrResponse {
                                id: String::new(),
                                result: None,
                                error: Some(format!("invalid request: {e}")),
                            };
                            let v = serde_json::to_value(&err).unwrap();
                            responder.respond(respond_json(v, 400));
                            return;
                        }
                    };

                    // Look up the site origin from the tab that made this
                    // request. We trust our own state over anything the
                    // content page could send, which prevents a site from
                    // spoofing another site's permissions.
                    let site = tab_site_for_label(&state, &webview_label)
                        .unwrap_or_else(|| "unknown".to_string());

                    // Snapshot the user's configured relays so getRelays()
                    // can return them. We clone here to release the settings
                    // lock before awaiting anything in dispatch.
                    let relay_urls = state.settings.lock().unwrap().relays.clone();

                    let dispatch_ctx = nip07::DispatchContext {
                        signer: &state.signer,
                        permissions: &state.permissions,
                        queue: &state.prompt_queue,
                        audit_log: &state.audit_log,
                        app: &app,
                        site,
                        relay_urls,
                    };
                    let response = nip07::dispatch(dispatch_ctx, req).await;
                    // Record based on whether dispatch produced an error.
                    // Still 200 at the HTTP layer — the error (if any)
                    // is in the JSON body — but devtools colors it red.
                    let err = response.error.clone();
                    record_devtools(if err.is_some() { 400 } else { 200 }, err);
                    let v = serde_json::to_value(&response).unwrap();
                    responder.respond(respond_json(v, 200));
                });
            }
        })
        .invoke_handler(tauri::generate_handler![
            navigate, go_back, go_forward, refresh, resize_content, console_eval,
            open_console, focus_address_bar, toggle_bookmark_cmd,
            add_bookmark, remove_bookmark, rename_bookmark, list_bookmarks, is_bookmarked,
            get_settings, update_settings, update_side_panel_width,
            create_tab, close_tab, switch_tab, get_tabs,
            get_site_info,
            signer_status, signer_create, signer_import, signer_unlock,
            signer_lock, signer_delete, signer_reveal_nsec,
            signer_pending_prompts, signer_resolve_prompt,
            signer_list_permissions, signer_revoke_permission, signer_revoke_site,
            signer_audit_log, signer_clear_audit_log,
            devtools_network_snapshot, devtools_network_clear,
            devtools_set_network_recording, devtools_get_network_recording,
            devtools_read_storage, devtools_delete_storage_key, devtools_clear_storage,
            hide_content_webview, show_content_webview,
            check_for_update, install_update,
        ])
        .setup(|app| {
            // Wire up the chrome log forwarder — from now on, tracing events
            // get emitted to the dev console panel via Tauri events.
            let handle = app.handle().clone();
            log_forward::set_app_handle(handle.clone());
            log_forward::flush_pending(&handle);

            let window = app.get_window("main").unwrap();
            let scale = window.scale_factor().unwrap_or(1.0);
            let phys_size = window.inner_size().unwrap();
            let logical_width = phys_size.width as f64 / scale;
            let logical_height = phys_size.height as f64 / scale;

            let content_top = 82.0; // tab strip (32) + toolbar (48) + loading bar (1) + 1px

            info!("window setup: phys={}x{}, scale={}, logical={}x{}, content_top={}",
                phys_size.width, phys_size.height, scale, logical_width, logical_height, content_top);

            // Create the first tab webview
            create_tab_webview(
                &window,
                &app.handle(),
                "tab-0",
                "nsite-content://internal/welcome",
                content_top,
                logical_width,
                logical_height - content_top,
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
    #[cfg(target_os = "windows")]
    fn platform_navigate_url_rewrites_on_windows() {
        assert_eq!(
            platform_navigate_url("nsite-content://internal/welcome"),
            "http://nsite-content.internal/welcome"
        );
        assert_eq!(
            platform_navigate_url("nsite-content://ab.titan/path"),
            "http://nsite-content.ab.titan/path"
        );
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn platform_navigate_url_is_identity_off_windows() {
        assert_eq!(
            platform_navigate_url("nsite-content://internal/welcome"),
            "nsite-content://internal/welcome"
        );
        assert_eq!(
            platform_navigate_url("nsite-content://ab.titan/path"),
            "nsite-content://ab.titan/path"
        );
    }

    #[test]
    fn content_url_to_display_strips_windows_prefix() {
        // Simulate the Windows-rewritten URL form
        let url = tauri::Url::parse("http://nsite-content.0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20.titan/").unwrap();
        let display = content_url_to_display(&url);
        assert_eq!(display, Some("titan".to_string()));
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

    // ── decode_npub edge cases ──

    #[test]
    fn decode_npub_valid() {
        let bytes = decode_npub("npub10qdp2fc9ta6vraczxrcs8prqnv69fru2k6s2dj48gqjcylulmtjsg9arpj")
            .unwrap();
        assert_eq!(
            hex::encode(bytes),
            "781a1527055f74c1f70230f10384609b34548f8ab6a0a6caa74025827f9fdae5"
        );
    }

    #[test]
    fn decode_npub_rejects_invalid_bech32() {
        assert!(decode_npub("npub1notavalidbech32string").is_err());
    }

    #[test]
    fn decode_npub_rejects_wrong_prefix() {
        // nsec prefix should fail — we only accept npub
        assert!(decode_npub("nsec1hmq6xuqnplk5lw0h3700cujmx5gymqn5wrn42u6432r6ntzumezqc3marw").is_err());
    }

    // ── base36 pubkey decoding ──

    #[test]
    fn base36_single_digit_roundtrip() {
        // "0" -> [0]
        let bytes = bigint_from_base36("0").unwrap();
        assert_eq!(bytes, vec![0]);
    }

    #[test]
    fn base36_uppercase_lowercase_equivalent() {
        let a = bigint_from_base36("abc123").unwrap();
        let b = bigint_from_base36("ABC123").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn base36_rejects_invalid_char() {
        assert!(bigint_from_base36("bad!char").is_err());
        assert!(bigint_from_base36("hello world").is_err());
    }

    #[test]
    fn base36_pubkey_rejects_oversized() {
        // A 51-char base36 string that decodes to more than 32 bytes
        // should fail in decode_base36_pubkey. Use all 'z' chars which
        // maximizes the value.
        let oversized = "z".repeat(60);
        assert!(decode_base36_pubkey(&oversized).is_err());
    }

    // ── html_escape ──

    #[test]
    fn html_escape_basic() {
        assert_eq!(html_escape(""), "");
        assert_eq!(html_escape("hello"), "hello");
        assert_eq!(html_escape("a & b"), "a &amp; b");
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a<b>c&d"), "a&lt;b&gt;c&amp;d");
    }

    #[test]
    fn html_escape_preserves_unicode_and_quotes() {
        // We only escape &, <, > — quotes pass through unchanged.
        assert_eq!(html_escape("hello \"world\""), "hello \"world\"");
        assert_eq!(html_escape("café 🎉"), "café 🎉");
    }

    // ── url_decode ──

    #[test]
    fn url_decode_plain_text() {
        assert_eq!(url_decode("hello"), "hello");
        assert_eq!(url_decode(""), "");
    }

    #[test]
    fn url_decode_percent_encoded() {
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("a%2Bb"), "a+b");
        assert_eq!(url_decode("%3Cscript%3E"), "<script>");
    }

    #[test]
    fn url_decode_plus_becomes_space() {
        assert_eq!(url_decode("hello+world"), "hello world");
    }

    #[test]
    fn url_decode_invalid_percent_passes_through() {
        // A malformed percent sequence should not crash; it passes through.
        assert_eq!(url_decode("100%"), "100%");
        assert_eq!(url_decode("a%zz"), "a%zz");
    }

    #[test]
    fn url_decode_trailing_percent_with_one_char() {
        // "%a" at the end — one char present but not enough for hex pair.
        // Should emit literally, not fabricate a zero byte.
        assert_eq!(url_decode("a%f"), "a%f");
    }

    #[test]
    fn url_decode_does_not_emit_null_byte() {
        // Regression: a trailing % must never produce a \0 character.
        let result = url_decode("hello%");
        assert!(!result.contains('\0'), "result was: {:?}", result);
        assert_eq!(result, "hello%");
    }

    #[test]
    fn url_decode_mixed() {
        assert_eq!(
            url_decode("name%3Dtitan+foo%26bar"),
            "name=titan foo&bar"
        );
    }

    // ── content_url_to_display all host forms ──

    #[test]
    fn content_url_to_display_internal_returns_none() {
        let url = tauri::Url::parse("nsite-content://internal/welcome").unwrap();
        assert_eq!(content_url_to_display(&url), None);
    }

    #[test]
    fn content_url_to_display_hex_returns_npub() {
        let url = tauri::Url::parse(
            "nsite-content://781a1527055f74c1f70230f10384609b34548f8ab6a0a6caa74025827f9fdae5/",
        )
        .unwrap();
        let display = content_url_to_display(&url).unwrap();
        assert!(display.starts_with("npub1"));
    }

    #[test]
    fn content_url_to_display_hex_with_path() {
        let url = tauri::Url::parse(
            "nsite-content://781a1527055f74c1f70230f10384609b34548f8ab6a0a6caa74025827f9fdae5/blog/post.html",
        )
        .unwrap();
        let display = content_url_to_display(&url).unwrap();
        assert!(display.starts_with("npub1"));
        assert!(display.ends_with("/blog/post.html"));
    }

    #[test]
    fn content_url_to_display_hex_with_name() {
        let url = tauri::Url::parse(
            "nsite-content://781a1527055f74c1f70230f10384609b34548f8ab6a0a6caa74025827f9fdae5.titan/",
        )
        .unwrap();
        assert_eq!(content_url_to_display(&url), Some("titan".to_string()));
    }

    #[test]
    fn content_url_to_display_hex_with_name_and_path() {
        let url = tauri::Url::parse(
            "nsite-content://781a1527055f74c1f70230f10384609b34548f8ab6a0a6caa74025827f9fdae5.titan/guide",
        )
        .unwrap();
        assert_eq!(
            content_url_to_display(&url),
            Some("titan/guide".to_string())
        );
    }

    // ── parse_content_host edge cases ──

    #[test]
    fn parse_content_host_short_hex_rejected() {
        // 63 hex chars — just under the 64-char minimum
        assert!(parse_content_host(&"ab".repeat(31)).is_none());
    }

    #[test]
    fn parse_content_host_non_hex_rejected() {
        let not_hex = "z".repeat(64);
        assert!(parse_content_host(&not_hex).is_none());
    }

    #[test]
    fn parse_content_host_hex_then_non_dot_separator() {
        // 64 hex chars followed by a non-dot character (no extra chars
        // until byte 65) — this should decode as plain hex only
        let hex = "ab".repeat(32);
        // Input is exactly 64 chars so it's the plain hex case
        let (pk, name) = parse_content_host(&hex).unwrap();
        assert_eq!(pk, [0xab; 32]);
        assert!(name.is_none());
    }

    // ── guess_content_type edge cases ──

    #[test]
    fn guess_content_type_uppercase_extension() {
        // Lowercase normalized — PNG should still map to image/png
        assert_eq!(guess_content_type("/PHOTO.PNG"), "image/png");
    }

    #[test]
    fn guess_content_type_directory_index() {
        assert_eq!(guess_content_type("/blog/"), "text/html");
    }

    #[test]
    fn guess_content_type_no_extension() {
        assert_eq!(guess_content_type("/README"), "text/html");
    }

    #[test]
    fn guess_content_type_fonts_and_wasm() {
        assert_eq!(guess_content_type("/font.woff2"), "font/woff2");
        assert_eq!(guess_content_type("/font.woff"), "font/woff");
        assert_eq!(guess_content_type("/font.ttf"), "font/ttf");
        assert_eq!(guess_content_type("/module.wasm"), "application/wasm");
    }

    #[test]
    fn guess_content_type_images() {
        assert_eq!(guess_content_type("/icon.svg"), "image/svg+xml");
        assert_eq!(guess_content_type("/photo.webp"), "image/webp");
        assert_eq!(guess_content_type("/icon.ico"), "image/x-icon");
        assert_eq!(guess_content_type("/photo.jpg"), "image/jpeg");
        assert_eq!(guess_content_type("/photo.jpeg"), "image/jpeg");
        assert_eq!(guess_content_type("/photo.gif"), "image/gif");
    }

    // ── Settings side panel width clamp tests ──
    //
    // The side panel width goes through two clamp layers:
    //   1. `load_settings` clamps on file load (so a hand-edited
    //      settings.json can't render the panel unusable)
    //   2. `update_side_panel_width` clamps on JS-driven saves
    //
    // These tests target the load path since it's the only one
    // reachable without a full Tauri State fixture.

    use std::sync::atomic::{AtomicU64, Ordering};
    static SETTINGS_TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn settings_test_dir() -> PathBuf {
        let n = SETTINGS_TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "titan-settings-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            n
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_settings_json(dir: &PathBuf, json: &str) {
        std::fs::write(dir.join("settings.json"), json).unwrap();
    }

    #[test]
    fn settings_missing_file_returns_default() {
        let dir = settings_test_dir();
        let s = load_settings(&dir);
        assert_eq!(s.side_panel_width, DEFAULT_SIDE_PANEL_WIDTH);
        assert_eq!(s.homepage, "titan");
    }

    #[test]
    fn settings_missing_field_uses_serde_default() {
        // A settings.json from a pre-v0.1.7 install won't have a
        // side_panel_width field. serde(default = "...") must kick in.
        let dir = settings_test_dir();
        write_settings_json(
            &dir,
            r#"{
                "relays": [],
                "discovery_relays": [],
                "blossom_servers": [],
                "indexer_pubkey": "abc",
                "homepage": "titan"
            }"#,
        );
        let s = load_settings(&dir);
        assert_eq!(s.side_panel_width, DEFAULT_SIDE_PANEL_WIDTH);
    }

    #[test]
    fn settings_width_zero_is_clamped_to_default() {
        // A hand-edited 0 would render the panel invisible. Clamp up
        // to the default.
        let dir = settings_test_dir();
        write_settings_json(
            &dir,
            r#"{
                "relays": [], "discovery_relays": [], "blossom_servers": [],
                "indexer_pubkey": "abc", "homepage": "titan",
                "side_panel_width": 0
            }"#,
        );
        let s = load_settings(&dir);
        assert_eq!(s.side_panel_width, DEFAULT_SIDE_PANEL_WIDTH);
    }

    #[test]
    fn settings_width_below_min_is_clamped() {
        let dir = settings_test_dir();
        let below = MIN_SIDE_PANEL_WIDTH - 1;
        write_settings_json(
            &dir,
            &format!(
                r#"{{
                    "relays": [], "discovery_relays": [], "blossom_servers": [],
                    "indexer_pubkey": "a", "homepage": "titan",
                    "side_panel_width": {below}
                }}"#
            ),
        );
        let s = load_settings(&dir);
        assert_eq!(s.side_panel_width, DEFAULT_SIDE_PANEL_WIDTH);
    }

    #[test]
    fn settings_width_above_max_is_clamped() {
        // A runaway 999999 would shove the content webview off-screen.
        let dir = settings_test_dir();
        write_settings_json(
            &dir,
            r#"{
                "relays": [], "discovery_relays": [], "blossom_servers": [],
                "indexer_pubkey": "a", "homepage": "titan",
                "side_panel_width": 999999
            }"#,
        );
        let s = load_settings(&dir);
        assert_eq!(s.side_panel_width, DEFAULT_SIDE_PANEL_WIDTH);
    }

    #[test]
    fn settings_width_at_min_is_accepted() {
        // Boundary: exactly MIN_SIDE_PANEL_WIDTH must pass through.
        let dir = settings_test_dir();
        write_settings_json(
            &dir,
            &format!(
                r#"{{
                    "relays": [], "discovery_relays": [], "blossom_servers": [],
                    "indexer_pubkey": "a", "homepage": "titan",
                    "side_panel_width": {}
                }}"#,
                MIN_SIDE_PANEL_WIDTH
            ),
        );
        let s = load_settings(&dir);
        assert_eq!(s.side_panel_width, MIN_SIDE_PANEL_WIDTH);
    }

    #[test]
    fn settings_width_at_max_is_accepted() {
        let dir = settings_test_dir();
        write_settings_json(
            &dir,
            &format!(
                r#"{{
                    "relays": [], "discovery_relays": [], "blossom_servers": [],
                    "indexer_pubkey": "a", "homepage": "titan",
                    "side_panel_width": {}
                }}"#,
                MAX_SIDE_PANEL_WIDTH
            ),
        );
        let s = load_settings(&dir);
        assert_eq!(s.side_panel_width, MAX_SIDE_PANEL_WIDTH);
    }

    #[test]
    fn settings_width_in_range_is_accepted() {
        // A normal user-dragged width between the bounds should
        // survive the load pass unchanged.
        let dir = settings_test_dir();
        write_settings_json(
            &dir,
            r#"{
                "relays": [], "discovery_relays": [], "blossom_servers": [],
                "indexer_pubkey": "a", "homepage": "titan",
                "side_panel_width": 720
            }"#,
        );
        let s = load_settings(&dir);
        assert_eq!(s.side_panel_width, 720);
    }

    #[test]
    fn settings_malformed_json_falls_back_to_default() {
        // A corrupted settings.json shouldn't crash the app on boot
        // — it should just fall back to defaults and warn.
        let dir = settings_test_dir();
        write_settings_json(&dir, "{ this is not valid json");
        let s = load_settings(&dir);
        assert_eq!(s.side_panel_width, DEFAULT_SIDE_PANEL_WIDTH);
        assert_eq!(s.homepage, "titan");
    }

    #[test]
    fn settings_save_and_reload_roundtrip() {
        // Write settings with a custom width, reload, and verify the
        // value is preserved.
        let dir = settings_test_dir();
        let mut settings = Settings::default();
        settings.side_panel_width = 512;
        settings.homepage = "custom".to_string();
        save_settings(&dir, &settings);

        let loaded = load_settings(&dir);
        assert_eq!(loaded.side_panel_width, 512);
        assert_eq!(loaded.homepage, "custom");
    }
}
