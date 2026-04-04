# Titan Roadmap

## Phase 1: Types + OP_RETURN Codec
- [x] Cargo workspace structure
- [x] `titan-types`: TitanName, TitanOp, NsiteUrl, error types
- [x] `titan-bitcoin/codec`: encode/decode OP_RETURN payloads
- [x] Unit tests for codec (round-trip, boundary, rejection)

## Phase 2: Bitcoin RPC + SQLite Store
- [x] Bitcoin Core JSON-RPC client (getblockchaininfo, getblockhash, getblock)
- [x] Wallet RPCs (getnewaddress, createrawtransaction, fundrawtransaction, signrawtransactionwithwallet, sendrawtransaction)
- [x] SQLite schema (names table, sync_state table)
- [x] Store operations (get_name, insert_name, transfer_name, sync state, rollback)

## Phase 3: Block Scanner / Indexer
- [x] Block scanner with reorg detection
- [x] Registration processing (first-in-chain wins)
- [x] Transfer processing (owner verification)
- [x] Background chain tip polling (30s interval)
- [x] Configurable start height (BITCOIN_START_HEIGHT env var)

## Phase 4: Nostr Resolver
- [x] Relay connection pool with fallbacks
- [x] Race-then-linger search (first result + 200ms linger window)
- [x] Kind 10002 relay list discovery
- [x] Kind 35128 manifest fetching (Bitcoin name, d-tag = name)
- [x] Kind 15128 manifest fetching (direct npub, root site)
- [x] Kind 10063 Blossom server list
- [x] Blossom HTTP blob fetching with SHA256 verification
- [x] Disk cache (manifests 5min TTL, relay/blossom lists 1hr, blobs forever)
- [x] nsite v1 fallback (kind 34128 per-file events assembled into manifest)

## Phase 5: Tauri Browser Shell
- [x] `nsite-content://` custom protocol handler for sub-resource resolution
- [x] Minimal UI: address bar, back/forward/refresh, Titan moon theme
- [x] Webview content rendering via custom protocol
- [x] npub, hex pubkey, and base36 resolution
- [x] Bitcoin name resolution: Nostr index (primary) → SQLite (fallback)
- [x] Background indexer startup (graceful if Bitcoin Core unavailable)
- [x] NSIT transaction builder (register/transfer names via Bitcoin Core wallet RPCs)
- [x] OnceCell resolver + RwLock nav context for concurrent sub-resource loading
- [x] Link interception (nsite:// clicks within rendered pages via postMessage)
- [x] Error categorization (name not found, relay down, hash mismatch, etc.)
- [x] Graceful shutdown (relay disconnect on window close)
- [x] Keyboard shortcut: Cmd/Ctrl+L to focus address bar

## Phase 6: Integration + Polish
- [x] End-to-end: registered `titan` on Bitcoin mainnet (txid: 322ab8...)
- [x] End-to-end: published titan nsite v2, loaded in Titan browser
- [x] `nsite://titan` resolves via Bitcoin name → Nostr manifest → Blossom blobs → render

## Phase 7: Name Manager + Nostr Index
- [x] `titan` registered on Bitcoin mainnet (block 943619)
- [x] `nsite://titan` published as nsite v2 (kind 35128, d=titan + kind 15128 root)
- [x] nsite v2 publisher script with Blossom auth (BUD-01 kind 24242)
- [x] Client-side NSIT OP_RETURN generator on nsite://titan (paste into any wallet)
- [x] Nostr name index: kind 35129 (name records) + kind 15129 (stats)
- [x] nsit-indexer service (TypeScript, k8s deployment, watches Bitcoin blocks → publishes Nostr events)
- [x] Full k8s setup: Dockerfile, deployment, network policy, secrets, CI/CD build job, deploy.sh
- [x] Bitcoin-node network policy updated for nsit-indexer access
- [x] Built-in browser name manager (Tauri commands: lookup_name, get_index_stats, register_name)
- [x] Name manager UI accessible via toolbar button (◈)
- [x] Three-tier name resolution: sync parse → Nostr index (race-then-linger) → not found
- [x] nsite://titan NDK integration (lookupName, fetchIndexStats via race-then-linger)
- [ ] Transfer name: build transfer transaction for a name you own
- [ ] My names: list names owned by a given address or pubkey
- [ ] Name history: registration and transfer timeline for a name

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
- Nostr-published name index as bootstrap (skip block scanning for new users)
- Mobile (iOS/Android via Tauri)
