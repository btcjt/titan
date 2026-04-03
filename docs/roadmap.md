# Titan Roadmap

## Phase 1: Types + OP_RETURN Codec
- [x] Cargo workspace structure
- [x] `titan-types`: TitanName, TitanOp, NsiteUrl, error types
- [x] `titan-bitcoin/codec`: encode/decode OP_RETURN payloads
- [x] Unit tests for codec (round-trip, boundary, rejection)

## Phase 2: Bitcoin RPC + SQLite Store
- [ ] Bitcoin Core JSON-RPC client (getblockchaininfo, getblockhash, getblock)
- [ ] SQLite schema (names table, sync_state table)
- [ ] Store operations (get_name, insert_name, transfer_name, sync state)

## Phase 3: Block Scanner / Indexer
- [ ] Background block scanner
- [ ] Registration processing (first-in-chain wins)
- [ ] Transfer processing (owner verification)
- [ ] Chain tip polling (~30s interval)
- [ ] Basic reorg handling (depth-1)

## Phase 4: Nostr Resolver
- [ ] Relay connection pool with fallbacks
- [ ] Kind 10002 relay list discovery
- [ ] Kind 15128/35128 manifest fetching
- [ ] Kind 10063 Blossom server list
- [ ] Blossom HTTP blob fetching with SHA256 verification
- [ ] Disk cache (manifests with TTL, blobs forever)

## Phase 5: Tauri Browser Shell
- [ ] `nsite://` custom protocol handler
- [ ] Minimal UI: address bar, back/forward/refresh
- [ ] Webview content rendering
- [ ] Sub-resource resolution through manifest
- [ ] Background indexer startup

## Phase 6: Integration + Polish
- [ ] End-to-end test (register name, publish site, load in Titan)
- [ ] Error states (name not found, relay down, blob unavailable)
- [ ] Loading indicator
- [ ] Content-type inference
- [ ] Graceful shutdown

## Phase 7: Distribution
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
