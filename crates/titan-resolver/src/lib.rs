//! Nostr relay interaction and Blossom blob fetching for Titan.

// TODO: Phase 4
// pub mod relay;
// pub mod manifest;
// pub mod blossom;
// pub mod cache;

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
