# Titan Roadmap

## Phase 1: Types + OP_RETURN Codec
- [x] Cargo workspace structure
- [x] `titan-types`: TitanName, TitanOp, NsiteUrl, error types
- [x] `titan-bitcoin/codec`: encode/decode OP_RETURN payloads
- [x] Unit tests for codec (round-trip, boundary, rejection)

## Phase 2: Bitcoin RPC + SQLite Store
- [x] Bitcoin Core JSON-RPC client (getblockchaininfo, getblockhash, getblock)
- [x] Wallet RPCs (getnewaddress, createrawtransaction, fundrawtransaction, signrawtransactionwithwallet, sendrawtransaction)
- [x] SQLite schema (names table, sync_state table) — used by nsit-indexer service
- [x] Store operations (get_name, insert_name, transfer_name, sync state, rollback)

## Phase 3: Block Scanner / Indexer
- [x] Block scanner with reorg detection
- [x] Registration processing (first-in-chain wins)
- [x] Transfer processing (owner verification)
- [x] Background chain tip polling (30s interval)
- [x] Configurable start height

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
- [x] Nostr-based name lookup (kind 35129 from indexer service)

## Phase 5: Tauri Browser Shell
- [x] `nsite-content://` custom protocol handler for sub-resource resolution
- [x] Minimal UI: address bar, back/forward/refresh, Titan moon theme + logo
- [x] Webview content rendering via custom protocol
- [x] npub, hex pubkey, and base36 resolution
- [x] Name resolution purely via Nostr (no local SQLite, no Bitcoin Core)
- [x] OnceCell resolver + RwLock nav context for concurrent sub-resource loading
- [x] Link interception (nsite:// clicks within rendered pages via postMessage)
- [x] Error categorization (name not found, relay down, hash mismatch, etc.)
- [x] Graceful shutdown (relay disconnect on window close)
- [x] Keyboard shortcut: Cmd/Ctrl+L to focus address bar
- [x] Default homepage: nsite://titan

## Phase 6: Integration + Polish
- [x] Registered `titan` on Bitcoin mainnet (block 943619)
- [x] Published nsite://titan as nsite v2, loaded in Titan browser
- [x] End-to-end: nsite://titan resolves via Nostr name index → manifest → Blossom → render

## Phase 7: nsite://titan + Nostr Index
- [x] nsite://titan published as nsite v2 (kind 35128 + kind 15128)
- [x] nsite v2 publisher script with Blossom auth (BUD-01 kind 24242)
- [x] nsit-indexer service deployed on k8s (watches Bitcoin → publishes Nostr events)
- [x] Indexer resumes from relay state on restart (fetches own kind 15129/35129)
- [x] Nostr name index: kind 35129 (name records) + kind 15129 (stats)
- [x] nsite://titan: name lookup via NDK with race-then-linger
- [x] nsite://titan: index stats from kind 15129
- [x] nsite://titan: recent activity feed
- [x] nsite://titan: client-side OP_RETURN generator for registration (/register)
- [x] nsite://titan: transfer OP_RETURN generator (/transfer)
- [x] nsite://titan: name browser with filter (/names)
- [x] nsite://titan: redesigned UX — dedicated pages for search, register, transfer, browse
- [x] nsite://titan: nav bar (Search, Browse, Register)
- [x] Removed SQLite and Bitcoin Core dependency from browser
- [x] Removed built-in name manager (replaced by nsite://titan)
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
- Mobile (iOS/Android via Tauri)
