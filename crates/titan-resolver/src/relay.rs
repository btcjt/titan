//! Nostr relay interaction — fetches events by kind and pubkey.
//!
//! Uses a "race + linger" search strategy: returns as soon as the first event
//! arrives, then waits an additional 200ms for other relays to respond before
//! finalizing results. This gives fast perceived latency while still collecting
//! the freshest replaceable events from slower relays.

use nostr_sdk::prelude::*;
use std::time::Duration;
use tokio_stream::StreamExt;
use tracing::{debug, warn};

/// Maximum time to wait for the first event from any relay.
const FIRST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// After the first event arrives, how long to wait for additional relays.
const LINGER_DURATION: Duration = Duration::from_millis(200);

/// Manages a pool of Nostr relay connections.
pub struct RelayPool {
    client: Client,
}

impl RelayPool {
    /// Create a new relay pool and connect to the given relay URLs.
    pub async fn connect(relay_urls: &[&str]) -> Result<Self, RelayError> {
        let client = Client::default();

        for url in relay_urls {
            if let Err(e) = client.add_relay(*url).await {
                warn!("failed to add relay {url}: {e}");
            }
        }

        client.connect().await;

        Ok(Self { client })
    }

    /// Race-then-linger event fetch.
    ///
    /// Streams events matching the filter. Returns as soon as the first event
    /// arrives, then collects any additional events that come in within 200ms.
    /// For replaceable events (kinds 10002, 10063, 15128, 35128) the newest
    /// event wins.
    async fn race_fetch(
        &self,
        filter: Filter,
    ) -> Result<Vec<Event>, RelayError> {
        let mut stream = self
            .client
            .stream_events(vec![filter], Some(FIRST_RESPONSE_TIMEOUT))
            .await
            .map_err(|e| RelayError::Fetch(e.to_string()))?;

        let mut events: Vec<Event> = Vec::new();

        // Wait for the first event (up to FIRST_RESPONSE_TIMEOUT)
        match stream.next().await {
            Some(event) => events.push(event),
            None => return Ok(events), // no results from any relay
        }

        // First event received — now linger for additional responses
        let linger_deadline = tokio::time::Instant::now() + LINGER_DURATION;
        loop {
            let remaining = linger_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, stream.next()).await {
                Ok(Some(event)) => events.push(event),
                _ => break,
            }
        }

        debug!("race_fetch: collected {} event(s)", events.len());
        Ok(events)
    }

    /// From a set of events, return the one with the highest `created_at`
    /// (newest). For replaceable events, this is the "winner".
    fn newest_event(events: Vec<Event>) -> Option<Event> {
        events.into_iter().max_by_key(|e| e.created_at)
    }

    /// Fetch the relay list (kind 10002) for a pubkey.
    /// Returns a list of relay URLs the pubkey has advertised.
    pub async fn fetch_relay_list(
        &self,
        pubkey: &PublicKey,
    ) -> Result<Vec<String>, RelayError> {
        let filter = Filter::new()
            .author(*pubkey)
            .kind(Kind::RelayList);

        let events = self.race_fetch(filter).await?;
        let event = match Self::newest_event(events) {
            Some(e) => e,
            None => return Ok(vec![]),
        };

        // Kind 10002 tags: ["r", "wss://relay.example.com"] or ["r", "wss://...", "read"]
        let urls: Vec<String> = event
            .tags
            .iter()
            .filter_map(|tag| {
                let values: Vec<&str> = tag.as_slice().iter().map(|s| s.as_str()).collect();
                if values.len() >= 2 && values[0] == "r" {
                    Some(values[1].to_string())
                } else {
                    None
                }
            })
            .collect();

        debug!("found {} relay(s) for {pubkey}", urls.len());
        Ok(urls)
    }

    /// Fetch the Blossom server list (kind 10063) for a pubkey.
    pub async fn fetch_blossom_servers(
        &self,
        pubkey: &PublicKey,
    ) -> Result<Vec<String>, RelayError> {
        let filter = Filter::new()
            .author(*pubkey)
            .kind(Kind::Custom(10063));

        let events = self.race_fetch(filter).await?;
        let event = match Self::newest_event(events) {
            Some(e) => e,
            None => return Ok(vec![]),
        };

        // Kind 10063 tags: ["server", "https://blossom.example.com"]
        let urls: Vec<String> = event
            .tags
            .iter()
            .filter_map(|tag| {
                let values: Vec<&str> = tag.as_slice().iter().map(|s| s.as_str()).collect();
                if values.len() >= 2 && values[0] == "server" {
                    Some(values[1].to_string())
                } else {
                    None
                }
            })
            .collect();

        debug!("found {} blossom server(s) for {pubkey}", urls.len());
        Ok(urls)
    }

    /// Fetch the nsite manifest event (kind 15128 or 35128) for a pubkey.
    /// Tries both kinds and returns the newest event found.
    pub async fn fetch_manifest(
        &self,
        pubkey: &PublicKey,
    ) -> Result<Option<Event>, RelayError> {
        // Query both kinds simultaneously — race_fetch will collect from all relays
        for kind_num in [35128u16, 15128] {
            let filter = Filter::new()
                .author(*pubkey)
                .kind(Kind::Custom(kind_num));

            let events = self.race_fetch(filter).await?;
            if let Some(event) = Self::newest_event(events) {
                debug!("found manifest (kind {kind_num}) for {pubkey}");
                return Ok(Some(event));
            }
        }

        Ok(None)
    }

    /// Add additional relay URLs to the pool (e.g. from a pubkey's relay list).
    pub async fn add_relays(&self, urls: &[String]) {
        for url in urls {
            if let Err(e) = self.client.add_relay(url.as_str()).await {
                debug!("failed to add relay {url}: {e}");
            }
        }
        self.client.connect().await;
    }

    /// Shut down all relay connections.
    pub async fn shutdown(self) -> Result<(), RelayError> {
        self.client
            .shutdown()
            .await
            .map_err(|e| RelayError::Fetch(e.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("relay fetch error: {0}")]
    Fetch(String),
}
