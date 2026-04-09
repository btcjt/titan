//! Custom `tracing` layer that forwards log events to the chrome webview
//! so they appear in the dev console panel alongside content-side console
//! messages. This makes Rust-side errors visible on Windows where stdout is
//! detached from any terminal.

use serde::Serialize;
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Emitter};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

#[derive(Debug, Clone, Serialize)]
pub struct RustLogPayload {
    pub level: String,
    pub target: String,
    pub message: String,
}

/// Global app handle, set once after Tauri has started.
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Set the app handle so the layer can emit events. Called from `setup`.
pub fn set_app_handle(handle: AppHandle) {
    let _ = APP_HANDLE.set(handle);
}

/// Buffer for log events emitted *before* the app handle is set. Replayed
/// once the handle becomes available.
static PENDING: OnceLock<Arc<std::sync::Mutex<Vec<RustLogPayload>>>> = OnceLock::new();

fn pending_buffer() -> &'static Arc<std::sync::Mutex<Vec<RustLogPayload>>> {
    PENDING.get_or_init(|| Arc::new(std::sync::Mutex::new(Vec::new())))
}

/// Flush any events that were logged before the app handle was set.
pub fn flush_pending(handle: &AppHandle) {
    let buffer = pending_buffer();
    if let Ok(mut pending) = buffer.lock() {
        for payload in pending.drain(..) {
            let _ = handle.emit("rust-log", payload);
        }
    }
}

pub struct ChromeLogLayer;

impl<S: Subscriber> Layer<S> for ChromeLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();

        // Filter out internal tracing noise and event-level DEBUG unless
        // the Rust-side filter is explicitly verbose.
        let level = metadata.level();
        if *level == Level::TRACE {
            return;
        }

        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        let payload = RustLogPayload {
            level: match *level {
                Level::ERROR => "error",
                Level::WARN => "warn",
                Level::INFO => "info",
                Level::DEBUG => "debug",
                _ => "info",
            }
            .to_string(),
            target: metadata.target().to_string(),
            message: visitor.message,
        };

        if let Some(handle) = APP_HANDLE.get() {
            let _ = handle.emit("rust-log", payload);
        } else {
            // App handle not set yet — buffer the event for later replay.
            if let Ok(mut pending) = pending_buffer().lock() {
                // Cap the buffer to avoid unbounded growth if something goes
                // very wrong during startup.
                if pending.len() < 500 {
                    pending.push(payload);
                }
            }
        }
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            if !self.message.is_empty() {
                self.message.push(' ');
            }
            self.message.push_str(&format!("{}={}", field.name(), value));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
            // Strip surrounding quotes that Debug adds for strings.
            if self.message.starts_with('"') && self.message.ends_with('"') {
                self.message = self.message[1..self.message.len() - 1].to_string();
            }
        } else {
            if !self.message.is_empty() {
                self.message.push(' ');
            }
            self.message.push_str(&format!("{}={:?}", field.name(), value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tracing_subscriber::prelude::*;

    /// Serialize all log_forward tests — they share the global PENDING
    /// buffer, so they can't run in parallel.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn clear_buffer() {
        if let Ok(mut b) = pending_buffer().lock() {
            b.clear();
        } else {
            // Mutex was poisoned by a previous test failure — reset it.
            pending_buffer().clear_poison();
            pending_buffer().lock().unwrap().clear();
        }
    }

    fn snapshot_buffer() -> Vec<RustLogPayload> {
        match pending_buffer().lock() {
            Ok(b) => b.clone(),
            Err(e) => {
                let mut g = e.into_inner();
                let out = g.clone();
                g.clear();
                out
            }
        }
    }

    #[test]
    fn log_forwarding_handles_non_string_fields() {
        // Non-string fields go through record_debug, which produces Debug
        // output. Verify the layer captures them correctly.
        let _guard = TEST_LOCK.lock().unwrap();
        clear_buffer();

        let subscriber = tracing_subscriber::registry().with(ChromeLogLayer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(count = 42, flag = true, "numeric fields");
        });

        let events = snapshot_buffer();
        assert_eq!(events.len(), 1);
        // count and flag are recorded via record_debug since they're not strings
        assert!(
            events[0].message.contains("count=42"),
            "message was: {}",
            events[0].message
        );
        assert!(
            events[0].message.contains("flag=true"),
            "message was: {}",
            events[0].message
        );
        clear_buffer();
    }

    #[test]
    fn pending_buffer_respects_cap() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_buffer();

        for _ in 0..1000 {
            let mut guard = pending_buffer().lock().unwrap();
            if guard.len() < 500 {
                guard.push(RustLogPayload {
                    level: "info".to_string(),
                    target: "test".to_string(),
                    message: "x".to_string(),
                });
            }
        }
        assert_eq!(pending_buffer().lock().unwrap().len(), 500);
        clear_buffer();
    }

    #[test]
    fn log_forwarding_captures_events_and_maps_levels() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_buffer();

        let subscriber = tracing_subscriber::registry().with(ChromeLogLayer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!("err message");
            tracing::warn!("warn message");
            tracing::info!("info message");
            tracing::debug!("debug message");
            tracing::trace!("trace message"); // should be dropped
        });

        let events = snapshot_buffer();
        assert_eq!(events.len(), 4, "trace should be filtered out");
        assert_eq!(events[0].level, "error");
        assert_eq!(events[0].message, "err message");
        assert_eq!(events[1].level, "warn");
        assert_eq!(events[2].level, "info");
        assert_eq!(events[3].level, "debug");
        clear_buffer();
    }

    #[test]
    fn log_forwarding_captures_fields() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_buffer();

        let subscriber = tracing_subscriber::registry().with(ChromeLogLayer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(field = "value", "with fields");
        });

        let events = snapshot_buffer();
        assert_eq!(events.len(), 1);
        assert!(
            events[0].message.contains("with fields"),
            "message was: {}",
            events[0].message
        );
        assert!(
            events[0].message.contains("field=value"),
            "message was: {}",
            events[0].message
        );
        clear_buffer();
    }
}
