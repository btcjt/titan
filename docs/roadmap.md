# Titan Roadmap

## Phase 1: Types + OP_RETURN Codec
- [x] Cargo workspace structure
- [x] `titan-types`: TitanName, TitanOp, NsiteUrl, error types
- [x] `titan-bitcoin/codec`: encode/decode OP_RETURN payloads
- [x] Unit tests for codec (round-trip, boundary, rejection)

## Phase 2: Bitcoin RPC + SQLite Store
- [x] Bitcoin Core JSON-RPC client (getblockchaininfo, getblockhash, getblock)
- [x] SQLite schema (names table, sync_state table)
- [x] Store operations (get_name, insert_name, transfer_name, sync state)

## Phase 3: Block Scanner / Indexer
- [x] Block scanner with reorg detection
- [x] Registration processing (first-in-chain wins)
- [x] Transfer processing (owner verification)
- [ ] Chain tip polling (~30s interval)
- [ ] Basic reorg handling (depth-1)

## Phase 4: Nostr Resolver
- [x] Relay connection pool with fallbacks
- [x] Race-then-linger search (first result + 200ms window)
- [x] Kind 10002 relay list discovery
- [x] Kind 35128 manifest fetching (Bitcoin name, d-tag = name)
- [x] Kind 15128 manifest fetching (direct npub, root site)
- [x] Kind 10063 Blossom server list
- [x] Blossom HTTP blob fetching with SHA256 verification
- [x] Disk cache (manifests 5min TTL, relay/blossom lists 1hr, blobs forever)

## Phase 5: Tauri Browser Shell
- [x] `nsite-content://` custom protocol handler for sub-resource resolution
- [x] Minimal UI: address bar, back/forward/refresh, Titan moon theme
- [x] Webview content rendering via custom protocol
- [x] npub and hex pubkey resolution
- [x] Base36 pubkey parsing (nsite.lol compat for testing)
- [x] Wire up Bitcoin name index to address bar (SQLite lookup in parse_host)
- [x] Background indexer startup (polls Bitcoin Core every 30s, graceful if unavailable)
- [x] NSIT transaction builder (register/transfer names via Bitcoin Core wallet RPCs)

## Phase 6: Integration + Polish
- [ ] End-to-end test (register name, publish site, load in Titan)
- [x] Error states (categorized: name not found, relay down, blob unavailable, hash mismatch)
- [x] Intercept nsite:// link clicks within rendered pages (script injection + postMessage)
- [x] Content-type inference improvements (webmanifest, etc.)
- [x] Graceful shutdown (relay disconnect on window close)
- [x] Keyboard shortcut: Cmd/Ctrl+L to focus address bar

## Phase 7: Name Manager (`nsite://titan`)
- [ ] Register `titan` on-chain, publish name manager as kind 35128 (d=titan)
- [ ] Name lookup: search by name, see owner pubkey, registration block, txid
- [ ] Name availability check: is a name taken or open?
- [ ] Index stats: total registered names, current sync height, blocks behind tip
- [ ] Register name: build OP_RETURN transaction (requires connected Bitcoin Core wallet)
- [ ] Transfer name: build transfer transaction for a name you own
- [ ] My names: list names owned by a given address or pubkey
- [ ] Name history: registration and transfer timeline for a name
- [ ] Serves as end-to-end proof of concept (dogfooding the full nsite stack)

## Phase 8: Distribution
- [ ] macOS .dmg
- [ ] Linux .AppImage
- [ ] Windows .msi
- [ ] GitHub Actions CI (build + test + release)

## Future
- Tab support
- Bookmarks + history
- Extension system
- Name marketplace
- Light client mode (remote indexer API)
- Mobile (iOS/Android via Tauri)
