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

/// A name record from the NSIT Nostr index (kind 35129).
#[derive(Debug, Clone)]
pub struct NsitNameRecord {
    pub name: String,
    pub pubkey_hex: String,
    pub owner_address: String,
    pub txid: String,
    pub block_height: u64,
}

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

    /// Fetch the nsite manifest event for a pubkey.
    ///
    /// The address type determines which manifest kind to query:
    /// - **Bitcoin name** (`site_name = Some("westernbtc")`): kind 35128 with
    ///   `d` tag matching the registered name. The on-chain name IS the site
    ///   identifier — one name, one site.
    /// - **Direct npub** (`site_name = None`): kind 15128 (root, one per pubkey).
    ///
    /// Falls back to v1 (kind 34128, per-file events) if no v2 manifest is found.
    pub async fn fetch_manifest(
        &self,
        pubkey: &PublicKey,
        site_name: Option<&str>,
    ) -> Result<Option<Event>, RelayError> {
        // Try v2 first
        match site_name {
            Some(name) => {
                // Bitcoin name → kind 35128, d-tag = the registered name
                let filter = Filter::new()
                    .author(*pubkey)
                    .kind(Kind::Custom(35128))
                    .custom_tag(SingleLetterTag::from_char('d').unwrap(), [name]);

                let events = self.race_fetch(filter).await?;
                if let Some(event) = Self::newest_event(events) {
                    debug!("found named manifest (kind 35128, d={name}) for {pubkey}");
                    return Ok(Some(event));
                }

                // Fall back to kind 15128 in case the publisher uses root manifests
                let filter = Filter::new()
                    .author(*pubkey)
                    .kind(Kind::Custom(15128));

                let events = self.race_fetch(filter).await?;
                if let Some(event) = Self::newest_event(events) {
                    debug!("falling back to root manifest (kind 15128) for {pubkey}");
                    return Ok(Some(event));
                }
            }
            None => {
                // Direct npub → kind 15128 (root manifest, one per pubkey)
                let filter = Filter::new()
                    .author(*pubkey)
                    .kind(Kind::Custom(15128));

                let events = self.race_fetch(filter).await?;
                if let Some(event) = Self::newest_event(events) {
                    debug!("found root manifest (kind 15128) for {pubkey}");
                    return Ok(Some(event));
                }
            }
        }

        // No v2 manifest found — check for v1 compatibility (kind 34128 per-file events)
        debug!("no v2 manifest found, trying v1 (kind 34128) for {pubkey}");
        Ok(None)
    }

    /// Fetch v1 nsite file events (kind 34128) and assemble them into a
    /// synthetic manifest. Each v1 event has `d` tag = file path, `x` or
    /// `sha256` tag = content hash.
    pub async fn fetch_v1_file_events(
        &self,
        pubkey: &PublicKey,
    ) -> Result<Vec<Event>, RelayError> {
        let filter = Filter::new()
            .author(*pubkey)
            .kind(Kind::Custom(34128));

        // For v1 we need all file events, not just one — use a longer linger
        let mut stream = self
            .client
            .stream_events(vec![filter], Some(FIRST_RESPONSE_TIMEOUT))
            .await
            .map_err(|e| RelayError::Fetch(e.to_string()))?;

        let mut events: Vec<Event> = Vec::new();

        // Collect events for up to 3 seconds (v1 sites can have many events)
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, stream.next()).await {
                Ok(Some(event)) => events.push(event),
                _ => break,
            }
        }

        debug!("fetched {} v1 file event(s) for {pubkey}", events.len());
        Ok(events)
    }

    // ── NSIT Name Index (kind 35129 from indexer service) ──

    /// Look up a name in the NSIT index published by an indexer service.
    ///
    /// Queries kind 35129 (addressable, d=name) signed by the indexer pubkey.
    /// Returns the name record event if found, using race-then-linger.
    pub async fn lookup_nsit_name(
        &self,
        name: &str,
        indexer_pubkey: &PublicKey,
    ) -> Result<Option<NsitNameRecord>, RelayError> {
        let filter = Filter::new()
            .author(*indexer_pubkey)
            .kind(Kind::Custom(35129))
            .custom_tag(SingleLetterTag::from_char('d').unwrap(), [name]);

        let events = self.race_fetch(filter).await?;
        let event = match Self::newest_event(events) {
            Some(e) => e,
            None => return Ok(None),
        };

        // Parse tags into a name record
        let mut pubkey_hex = String::new();
        let mut owner_address = String::new();
        let mut txid = String::new();
        let mut block_height: u64 = 0;

        for tag in event.tags.iter() {
            let values: Vec<&str> = tag.as_slice().iter().map(|s| s.as_str()).collect();
            match values.first().copied() {
                Some("p") if values.len() >= 2 => pubkey_hex = values[1].to_string(),
                Some("owner") if values.len() >= 2 => owner_address = values[1].to_string(),
                Some("txid") if values.len() >= 2 => txid = values[1].to_string(),
                Some("block") if values.len() >= 2 => {
                    block_height = values[1].parse().unwrap_or(0);
                }
                _ => {}
            }
        }

        if pubkey_hex.is_empty() {
            return Ok(None);
        }

        debug!("found NSIT name record for '{name}' at block {block_height}");

        Ok(Some(NsitNameRecord {
            name: name.to_string(),
            pubkey_hex,
            owner_address,
            txid,
            block_height,
        }))
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

    /// Shut down all relay connections (takes ownership).
    pub async fn shutdown(self) -> Result<(), RelayError> {
        self.client
            .shutdown()
            .await
            .map_err(|e| RelayError::Fetch(e.to_string()))
    }

    /// Disconnect all relays (borrow-friendly, for use in OnceCell/Arc contexts).
    pub async fn disconnect(&self) -> Result<(), RelayError> {
        self.client
            .disconnect()
            .await
            .map_err(|e| RelayError::Fetch(e.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("relay fetch error: {0}")]
    Fetch(String),
}
