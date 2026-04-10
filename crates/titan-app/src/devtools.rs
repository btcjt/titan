//! In-memory devtools state: network log ring buffer + recording toggle.
//!
//! The dev console panel's Network tab reads from this state via Tauri
//! commands. Events arrive from two sources:
//!
//! 1. **Rust-side protocol handlers** (`nsite-content://` and
//!    `titan-nostr://`) call `record_event` directly with the method,
//!    URL, status, and timing for every request they serve.
//!
//! 2. **Injected JS wrappers** in the content webview wrap `fetch`,
//!    `XMLHttpRequest`, and `WebSocket` at init time. When a request
//!    fires they send the event back to chrome via a `titan-cmd://`
//!    URL (the same escape hatch the console REPL uses), and the
//!    navigation handler parses the payload and calls `record_event`.
//!
//! The log is a simple ring buffer capped at `MAX_NETWORK_EVENTS` so
//! long-running sessions don't grow unbounded. Recording can be
//! toggled off from the UI; when off, new events are dropped on the
//! floor (the buffer is not cleared — the user has to Clear manually).
//!
//! Not persisted to disk. Devtools state resets every time Titan
//! restarts, same as browser devtools in Chrome / Firefox.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// Maximum number of events held in the ring buffer. When the buffer
/// is full, the oldest event is dropped to make room for a new one.
///
/// 500 is generous for interactive use: even a heavy nsite page load
/// (HTML + 20 chunks + images + fonts) produces ~40 events, so this
/// holds 10+ full page loads worth of history.
pub const MAX_NETWORK_EVENTS: usize = 500;

/// A single captured network request. Fields match what the dev
/// console Network tab displays: method, URL, status, timing, and an
/// optional request body for copy-as-cURL reconstruction.
///
/// The `id` is a monotonic counter assigned on insertion so the UI
/// can stable-sort and identify rows across snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEvent {
    /// Monotonic id assigned at record time. Used by the UI for
    /// stable selection across snapshots.
    pub id: u64,
    /// Unix milliseconds at insertion time. The UI shows a relative
    /// time ("2s ago") and uses this for sorting.
    pub timestamp_ms: u64,
    /// Which content tab's webview originated the request. Empty for
    /// Rust-side captures that don't have a tab context.
    pub tab_label: String,
    /// "GET", "POST", "WS" (for WebSocket open), "XHR", etc. The UI
    /// color-codes by method.
    pub method: String,
    /// Full URL including scheme and query string.
    pub url: String,
    /// HTTP-ish status code, or 0 if pending / unavailable.
    pub status: u16,
    /// Resource kind as best-guessed from the URL / MIME type:
    /// "document", "script", "stylesheet", "image", "fetch", "xhr",
    /// "websocket", "nsite-content", "titan-nostr", "other".
    pub resource_type: String,
    /// Elapsed time in milliseconds from request start to completion.
    /// None while the request is still in flight.
    pub duration_ms: Option<u64>,
    /// Selected request headers (user-agent, content-type, etc.).
    /// Small subset to keep memory reasonable.
    pub request_headers: Vec<(String, String)>,
    /// Selected response headers.
    pub response_headers: Vec<(String, String)>,
    /// Request body, truncated to 8 KiB. Used only for copy-as-cURL
    /// reconstruction of POST/PUT requests.
    pub request_body: Option<String>,
    /// Source: "rust" for Rust-side protocol handlers, "js" for
    /// injected content-page wrappers. Lets the UI show provenance.
    pub source: String,
    /// Populated if the request failed before completing (network
    /// error, CORS denial, etc.). Mutually exclusive with a non-zero
    /// status in the happy path.
    pub error: Option<String>,
}

/// Shared devtools state. Held inside the AppState arc.
pub struct DevtoolsState {
    /// Ring buffer of captured events, newest last.
    events: Mutex<VecDeque<NetworkEvent>>,
    /// Whether new events should be accepted. Toggled by the UI
    /// "Record" checkbox. Off by default would surprise users who
    /// open the Network tab expecting to see recent activity, so we
    /// start in the on state.
    recording: AtomicBool,
    /// Monotonic id counter for new events.
    next_id: AtomicU64,
}

impl DevtoolsState {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(VecDeque::with_capacity(MAX_NETWORK_EVENTS)),
            recording: AtomicBool::new(true),
            next_id: AtomicU64::new(1),
        }
    }

    /// Record a completed (or failed) network event. Drops the
    /// oldest event if the buffer is full. No-op when recording is
    /// disabled.
    pub fn record_event(&self, mut event: NetworkEvent) {
        if !self.recording.load(Ordering::Relaxed) {
            return;
        }
        // Stamp the id + timestamp here so callers don't have to
        // fight with monotonic counters.
        event.id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if event.timestamp_ms == 0 {
            event.timestamp_ms = now_ms();
        }

        let mut guard = self.events.lock().unwrap();
        if guard.len() >= MAX_NETWORK_EVENTS {
            guard.pop_front();
        }
        guard.push_back(event);
    }

    /// Snapshot of all current events (newest last). Used by the UI
    /// to repaint the table. Clones the whole buffer so the caller
    /// doesn't hold the lock.
    pub fn snapshot(&self) -> Vec<NetworkEvent> {
        self.events.lock().unwrap().iter().cloned().collect()
    }

    /// Clear the buffer. Called from the UI "Clear" button. Does not
    /// affect the recording toggle.
    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }

    /// Toggle whether new events are accepted.
    pub fn set_recording(&self, on: bool) {
        self.recording.store(on, Ordering::Relaxed);
    }

    /// Whether recording is currently enabled.
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }
}

impl Default for DevtoolsState {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Parse a JSON payload sent by the content-page JS wrappers via
/// `titan-cmd://net-event/<encoded>`. Tolerant of missing fields —
/// the injected JS uses a tight schema but we don't want a single
/// bad event to poison the whole log.
pub fn parse_js_event(json: &str, tab_label: &str) -> Option<NetworkEvent> {
    #[derive(Deserialize)]
    struct JsEvent {
        method: Option<String>,
        url: Option<String>,
        status: Option<u16>,
        resource_type: Option<String>,
        duration_ms: Option<u64>,
        request_headers: Option<Vec<(String, String)>>,
        response_headers: Option<Vec<(String, String)>>,
        request_body: Option<String>,
        error: Option<String>,
    }

    let parsed: JsEvent = serde_json::from_str(json).ok()?;
    let url = parsed.url?;
    let method = parsed.method.unwrap_or_else(|| "GET".to_string());
    // Cap request_body at 8 KiB — the JS wrapper already truncates
    // but we enforce here defensively.
    let body = parsed.request_body.map(|s| {
        if s.len() > 8192 {
            format!("{}...[truncated]", &s[..8192])
        } else {
            s
        }
    });
    Some(NetworkEvent {
        id: 0,
        timestamp_ms: 0,
        tab_label: tab_label.to_string(),
        method,
        url,
        status: parsed.status.unwrap_or(0),
        resource_type: parsed.resource_type.unwrap_or_else(|| "other".to_string()),
        duration_ms: parsed.duration_ms,
        request_headers: parsed.request_headers.unwrap_or_default(),
        response_headers: parsed.response_headers.unwrap_or_default(),
        request_body: body,
        source: "js".to_string(),
        error: parsed.error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only shorthand for building a NetworkEvent with the
    /// defaults we use throughout the tests (no headers, no body,
    /// source = "rust"). Production callers build the full struct
    /// inline at each protocol-handler response site, which is why
    /// there's no production helper.
    fn rust_event(
        method: &str,
        url: impl Into<String>,
        status: u16,
        resource_type: &str,
        duration_ms: Option<u64>,
    ) -> NetworkEvent {
        NetworkEvent {
            id: 0,
            timestamp_ms: 0,
            tab_label: String::new(),
            method: method.to_string(),
            url: url.into(),
            status,
            resource_type: resource_type.to_string(),
            duration_ms,
            request_headers: vec![],
            response_headers: vec![],
            request_body: None,
            source: "rust".to_string(),
            error: None,
        }
    }

    #[test]
    fn ring_buffer_caps_at_max() {
        let state = DevtoolsState::new();
        // Insert 2x capacity so we can verify the oldest events were
        // dropped and the newest ones remain.
        for i in 0..(MAX_NETWORK_EVENTS * 2) {
            state.record_event(rust_event(
                "GET",
                format!("https://example.com/{i}"),
                200,
                "fetch",
                Some(50),
            ));
        }
        let snap = state.snapshot();
        assert_eq!(snap.len(), MAX_NETWORK_EVENTS);
        // The oldest surviving event should be the one at index
        // MAX_NETWORK_EVENTS (half of what we inserted).
        assert!(snap[0].url.ends_with(&format!("/{}", MAX_NETWORK_EVENTS)));
        // The newest should be the last we inserted.
        assert!(snap[snap.len() - 1]
            .url
            .ends_with(&format!("/{}", MAX_NETWORK_EVENTS * 2 - 1)));
    }

    #[test]
    fn id_is_monotonic() {
        let state = DevtoolsState::new();
        state.record_event(rust_event("GET", "https://a/", 200, "fetch", None));
        state.record_event(rust_event("GET", "https://b/", 200, "fetch", None));
        state.record_event(rust_event("GET", "https://c/", 200, "fetch", None));
        let snap = state.snapshot();
        assert!(snap[0].id < snap[1].id);
        assert!(snap[1].id < snap[2].id);
    }

    #[test]
    fn recording_off_drops_events() {
        let state = DevtoolsState::new();
        state.record_event(rust_event("GET", "https://a/", 200, "fetch", None));
        state.set_recording(false);
        state.record_event(rust_event("GET", "https://b/", 200, "fetch", None));
        state.set_recording(true);
        state.record_event(rust_event("GET", "https://c/", 200, "fetch", None));

        let snap = state.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].url, "https://a/");
        assert_eq!(snap[1].url, "https://c/");
    }

    #[test]
    fn clear_empties_buffer_but_keeps_recording() {
        let state = DevtoolsState::new();
        state.record_event(rust_event("GET", "https://a/", 200, "fetch", None));
        state.clear();
        assert_eq!(state.snapshot().len(), 0);
        assert!(state.is_recording());
        // New events still land after clear
        state.record_event(rust_event("GET", "https://b/", 200, "fetch", None));
        assert_eq!(state.snapshot().len(), 1);
    }

    #[test]
    fn parse_js_event_minimal_fields() {
        let json = r#"{"url":"https://example.com/api","method":"POST","status":201}"#;
        let ev = parse_js_event(json, "tab-0").expect("parses");
        assert_eq!(ev.url, "https://example.com/api");
        assert_eq!(ev.method, "POST");
        assert_eq!(ev.status, 201);
        assert_eq!(ev.tab_label, "tab-0");
        assert_eq!(ev.source, "js");
    }

    #[test]
    fn parse_js_event_with_full_fields() {
        let json = r#"{
            "method": "POST",
            "url": "https://example.com/api/v1",
            "status": 200,
            "resource_type": "fetch",
            "duration_ms": 42,
            "request_headers": [["content-type", "application/json"]],
            "response_headers": [["server", "nginx"]],
            "request_body": "{\"hello\":\"world\"}"
        }"#;
        let ev = parse_js_event(json, "tab-7").expect("parses");
        assert_eq!(ev.method, "POST");
        assert_eq!(ev.duration_ms, Some(42));
        assert_eq!(ev.request_headers.len(), 1);
        assert_eq!(ev.response_headers.len(), 1);
        assert_eq!(ev.request_body.as_deref(), Some(r#"{"hello":"world"}"#));
    }

    #[test]
    fn parse_js_event_missing_url_fails() {
        let json = r#"{"method":"GET"}"#;
        assert!(parse_js_event(json, "tab-0").is_none());
    }

    #[test]
    fn parse_js_event_malformed_json_fails() {
        assert!(parse_js_event("not json", "tab-0").is_none());
        assert!(parse_js_event("", "tab-0").is_none());
    }

    #[test]
    fn parse_js_event_truncates_huge_request_body() {
        // Build a JSON with a 20 KB request body — the parser should
        // truncate to 8 KB + "...[truncated]".
        let big = "x".repeat(20_000);
        let json = format!(r#"{{"url":"https://x","request_body":"{big}"}}"#);
        let ev = parse_js_event(&json, "tab-0").expect("parses");
        let body = ev.request_body.expect("has body");
        assert!(body.len() < 20_000);
        assert!(body.ends_with("...[truncated]"));
    }

    #[test]
    fn rust_event_defaults_are_sensible() {
        let ev = rust_event("GET", "https://example.com", 200, "fetch", Some(100));
        assert_eq!(ev.source, "rust");
        assert_eq!(ev.duration_ms, Some(100));
        assert_eq!(ev.id, 0); // filled by record_event
        assert!(ev.request_headers.is_empty());
        assert!(ev.response_headers.is_empty());
        assert!(ev.request_body.is_none());
        assert!(ev.error.is_none());
    }

    #[test]
    fn snapshot_preserves_insertion_order() {
        let state = DevtoolsState::new();
        let urls = [
            "https://a/",
            "https://b/",
            "https://c/",
            "https://d/",
        ];
        for u in urls.iter() {
            state.record_event(rust_event("GET", u.to_string(), 200, "fetch", None));
        }
        let snap = state.snapshot();
        for (i, ev) in snap.iter().enumerate() {
            assert_eq!(ev.url, urls[i]);
        }
    }

    // ── Adversarial tests ──
    //
    // These tests intentionally try to break the parser or the ring
    // buffer with malformed / hostile inputs. The goal is not coverage
    // padding but catching real failure modes: integer overflow, JSON
    // injection, concurrent access, exotic Unicode, etc.
    //
    // If any of these tests start failing after a refactor, treat it
    // as a bug in the new code — not a reason to delete the test.

    #[test]
    fn parse_js_event_with_javascript_url_is_not_blocked() {
        // Devtools faithfully records what the page tried to do, even
        // if the URL is dangerous. The Network tab is an observer,
        // not a security boundary — the browser's CSP is the real
        // defense. Regression guard: if someone adds URL validation
        // here, they'll break the Network tab's ability to show
        // suspicious traffic.
        let json = r#"{"url":"javascript:alert(1)","method":"GET"}"#;
        let ev = parse_js_event(json, "tab-0").expect("parses");
        assert_eq!(ev.url, "javascript:alert(1)");
    }

    #[test]
    fn parse_js_event_status_over_u16_max_fails_or_clamps() {
        // serde_json parsing u16 from a number > 65535 will fail.
        // We want the whole event to be rejected, not silently
        // wrapping to a misleading status.
        let json = r#"{"url":"https://x/","status":99999}"#;
        let result = parse_js_event(json, "tab-0");
        // Either None (rejected) or 0 (default). Both are fine; the
        // key is we don't wrap to 34463 or similar.
        match result {
            None => {} // rejected wholesale — acceptable
            Some(ev) => assert_eq!(ev.status, 0, "must not wrap to a bogus value"),
        }
    }

    #[test]
    fn parse_js_event_negative_duration_ms_rejected_or_defaulted() {
        // duration_ms is u64 — a negative value can't deserialize.
        let json = r#"{"url":"https://x/","duration_ms":-500}"#;
        let result = parse_js_event(json, "tab-0");
        match result {
            None => {} // rejected
            Some(ev) => assert_eq!(ev.duration_ms, None),
        }
    }

    #[test]
    fn parse_js_event_with_null_bytes_in_strings_is_preserved() {
        // Rust Strings can hold NUL. JSON spec says \u0000 is valid.
        // We should round-trip it rather than truncating.
        let json = r#"{"url":"https://x/\u0000/y","method":"GET"}"#;
        let ev = parse_js_event(json, "tab-0").expect("parses");
        assert!(ev.url.contains('\0'), "null byte should survive");
    }

    #[test]
    fn parse_js_event_with_enormous_header_count_is_bounded() {
        // 10k headers shouldn't crash the parser. The parser doesn't
        // currently cap header count but the resulting event is
        // eventually stored in the ring buffer. Document current
        // behavior: it parses successfully. If we ever add a header
        // cap, this test needs to be updated accordingly.
        let mut headers = String::from("[");
        for i in 0..10_000 {
            if i > 0 {
                headers.push(',');
            }
            headers.push_str(&format!(r#"["h{i}","v{i}"]"#));
        }
        headers.push(']');
        let json = format!(
            r#"{{"url":"https://x/","request_headers":{headers}}}"#
        );
        let ev = parse_js_event(&json, "tab-0").expect("parses 10k headers");
        assert_eq!(ev.request_headers.len(), 10_000);
    }

    #[test]
    fn parse_js_event_with_header_not_pair_is_ignored() {
        // The request_headers shape is Vec<(String, String)>. A
        // single-element row like ["lonely"] should make serde fail,
        // so the whole event is rejected.
        let json = r#"{"url":"https://x/","request_headers":[["lonely"]]}"#;
        let result = parse_js_event(json, "tab-0");
        assert!(result.is_none(), "malformed header row must reject event");
    }

    #[test]
    fn parse_js_event_url_with_percent_encoded_injection() {
        // A URL with a percent-encoded titan-cmd scheme. Devtools
        // records it as-is; execution safety is the navigation
        // handler's job, not the parser's.
        let json = r#"{"url":"https://evil.com/%2Fback?cmd=titan-cmd%3A%2F%2Fconsole","method":"GET"}"#;
        let ev = parse_js_event(json, "tab-0").expect("parses");
        assert!(ev.url.contains("%2F"));
    }

    #[test]
    fn parse_js_event_extremely_long_url_is_preserved_intact() {
        // No max URL length in the parser. 64 KB URL should still
        // parse without silently truncating.
        let long = "a".repeat(65_536);
        let json = format!(r#"{{"url":"https://x/{long}"}}"#);
        let ev = parse_js_event(&json, "tab-0").expect("parses long url");
        assert!(ev.url.len() > 65_000);
    }

    #[test]
    fn parse_js_event_unicode_strings_roundtrip() {
        // Ensure multibyte characters in URL, method, body, and
        // header values all survive the parse.
        let json = r#"{
            "url": "https://例え.com/日本語/路径",
            "method": "POST",
            "request_body": "{\"msg\":\"こんにちは\"}",
            "request_headers": [["x-note", "Спасибо"]]
        }"#;
        let ev = parse_js_event(json, "tab-🦀").expect("parses unicode");
        assert!(ev.url.contains("例え"));
        assert!(ev.url.contains("日本語"));
        assert!(ev.request_body.as_deref().unwrap().contains("こんにちは"));
        assert_eq!(ev.request_headers[0].1, "Спасибо");
        assert_eq!(ev.tab_label, "tab-🦀");
    }

    #[test]
    fn record_event_accepts_empty_url_without_panicking() {
        // A Rust-side capture with an empty URL string. Not great,
        // but it should never panic. Devtools will show a blank URL
        // row which is a visual bug, not a crash.
        let state = DevtoolsState::new();
        state.record_event(rust_event("GET", "", 200, "other", None));
        assert_eq!(state.snapshot().len(), 1);
    }

    #[test]
    fn concurrent_record_and_snapshot_does_not_deadlock() {
        // Spawn many threads that hammer record_event while another
        // thread takes snapshots. The mutex should serialize cleanly.
        // Deadlock would manifest as a test timeout.
        use std::sync::Arc;
        use std::thread;

        let state = Arc::new(DevtoolsState::new());
        let mut handles = vec![];

        for t in 0..8 {
            let s = state.clone();
            handles.push(thread::spawn(move || {
                for i in 0..200 {
                    s.record_event(rust_event(
                        "GET",
                        format!("https://thread-{t}/{i}"),
                        200,
                        "fetch",
                        None,
                    ));
                }
            }));
        }
        // One reader thread taking snapshots in parallel
        let reader_state = state.clone();
        let reader = thread::spawn(move || {
            for _ in 0..50 {
                let snap = reader_state.snapshot();
                // Snapshot length is monotonically increasing up to the cap
                assert!(snap.len() <= MAX_NETWORK_EVENTS);
            }
        });

        for h in handles {
            h.join().unwrap();
        }
        reader.join().unwrap();

        // Final state: 8 threads * 200 events = 1600 records recorded,
        // but the ring buffer caps at MAX_NETWORK_EVENTS (500). So the
        // final snapshot must be exactly at the cap.
        assert_eq!(state.snapshot().len(), MAX_NETWORK_EVENTS);
    }

    #[test]
    fn recording_toggle_race_does_not_deadlock() {
        // A reader flipping the recording toggle concurrently with
        // writers shouldn't cause lockups. Recording state uses an
        // atomic, not the mutex, so this should be lock-free.
        use std::sync::Arc;
        use std::thread;

        let state = Arc::new(DevtoolsState::new());
        let toggler_state = state.clone();
        let toggler = thread::spawn(move || {
            for i in 0..1000 {
                toggler_state.set_recording(i % 2 == 0);
            }
        });
        let writer_state = state.clone();
        let writer = thread::spawn(move || {
            for i in 0..1000 {
                writer_state.record_event(rust_event(
                    "GET",
                    format!("https://x/{i}"),
                    200,
                    "fetch",
                    None,
                ));
            }
        });
        toggler.join().unwrap();
        writer.join().unwrap();
        // Test passes if we got here without a deadlock or panic.
        // The number of recorded events depends on timing; we don't
        // assert an exact count.
    }

    #[test]
    fn huge_request_body_truncation_is_lossless_below_cap() {
        // Bodies at exactly the 8 KiB boundary should NOT get the
        // truncation marker. One byte over gets it.
        let at_cap = "x".repeat(8192);
        let json_at = format!(r#"{{"url":"https://x/","request_body":"{at_cap}"}}"#);
        let ev_at = parse_js_event(&json_at, "tab-0").expect("parses");
        let body_at = ev_at.request_body.unwrap();
        assert_eq!(body_at.len(), 8192, "8192 bytes should not be truncated");
        assert!(!body_at.contains("[truncated]"));

        let over_cap = "x".repeat(8193);
        let json_over = format!(r#"{{"url":"https://x/","request_body":"{over_cap}"}}"#);
        let ev_over = parse_js_event(&json_over, "tab-0").expect("parses");
        let body_over = ev_over.request_body.unwrap();
        assert!(body_over.contains("[truncated]"));
        // The truncated prefix is exactly 8192 chars from the original
        assert!(body_over.starts_with(&"x".repeat(8192)));
    }

    #[test]
    fn ring_buffer_exactly_at_cap_does_not_evict() {
        // Inserting exactly MAX_NETWORK_EVENTS should not evict any.
        let state = DevtoolsState::new();
        for i in 0..MAX_NETWORK_EVENTS {
            state.record_event(rust_event(
                "GET",
                format!("https://x/{i}"),
                200,
                "fetch",
                None,
            ));
        }
        let snap = state.snapshot();
        assert_eq!(snap.len(), MAX_NETWORK_EVENTS);
        assert!(snap[0].url.ends_with("/0"));
        assert!(snap[snap.len() - 1]
            .url
            .ends_with(&format!("/{}", MAX_NETWORK_EVENTS - 1)));
    }

    #[test]
    fn id_counter_survives_clear() {
        // Clearing the buffer should NOT reset the monotonic id.
        // If it did, we'd get duplicate ids across clear boundaries,
        // which would break the UI's "stable row selection" logic.
        let state = DevtoolsState::new();
        state.record_event(rust_event("GET", "https://a/", 200, "fetch", None));
        let first_id = state.snapshot()[0].id;
        state.clear();
        state.record_event(rust_event("GET", "https://b/", 200, "fetch", None));
        let second_id = state.snapshot()[0].id;
        assert!(
            second_id > first_id,
            "id counter must not reset on clear (got {first_id} then {second_id})"
        );
    }

    #[test]
    fn timestamp_is_stamped_on_insert_when_zero() {
        // record_event fills in timestamp_ms if the caller passed 0.
        let state = DevtoolsState::new();
        state.record_event(rust_event("GET", "https://a/", 200, "fetch", None));
        let snap = state.snapshot();
        assert!(snap[0].timestamp_ms > 0);
    }

    #[test]
    fn caller_supplied_timestamp_is_preserved() {
        // A non-zero timestamp from the caller (e.g. a test fixture
        // replaying captured events) should pass through unchanged.
        let state = DevtoolsState::new();
        let mut ev = rust_event("GET", "https://a/", 200, "fetch", None);
        ev.timestamp_ms = 1_000_000_000_000;
        state.record_event(ev);
        assert_eq!(state.snapshot()[0].timestamp_ms, 1_000_000_000_000);
    }
}
