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
- [x] My names: list names by npub (/my-names, queries kind 35129 by #p tag)
- [x] Name details + history: /name?q= page with UTXO, mempool.space link, timeline

## Phase 8: Distribution
- [x] GitHub Actions CI — tests on every push/PR
- [x] GitHub Actions Release — builds on tag push, creates GitHub release with artifacts
- [x] macOS .dmg (aarch64 + x86_64)
- [x] Linux .AppImage + .deb
- [x] Windows .msi + .exe (NSIS)
- [x] Tauri bundle config (macOS min version, Linux deps, Windows install mode)
- [x] zapstore.dev publish script (kind 32267 app listing via nak)

## Phase 9: Tabs, Console, Interactive Registration
- [x] Tab support — multi-webview, per-tab URL/history, Cmd+T/W/1-9
- [x] Tab strip in overlay titlebar with favicon placeholders
- [x] Dev console forwarding (console.log/warn/error from content webviews)
- [x] Bookmarks with side panel, inline rename, star toggle
- [x] Settings panel (relays, discovery relays, Blossom servers, indexer pubkey, homepage)
- [x] Kind 1129 name history events (non-replaceable chain of custody)
- [x] Interactive bitcoin-cli command builder for registration (/register)
- [x] Interactive bitcoin-cli command builder for transfers (/transfer)
- [x] Wallet name support (-rpcwallet) for multi-wallet Bitcoin Core setups
- [x] UTXO protection guide (lockunspent, setlabel, dedicated wallet)
- [x] Registered `bitcoin` on mainnet (block 943978)
- [x] nsit-indexer: internal k8s relay service, removed network policy

## Phase 10: Built-in Signer (NIP-07)

Titan ships with a built-in signer that injects `window.nostr` into every content webview. This makes Titan a drop-in browser for any existing nsite that uses Nostr, with no external signer required.

### v1 scope

**Key management**
- [ ] Generate new nsec in-app with backup confirmation
- [ ] Import existing nsec (nsec1... or hex)
- [ ] Single identity (multi-identity deferred)
- [ ] Delete / replace identity
- [ ] Reveal nsec with password + warning

**Storage & security**
- [ ] OS keychain integration (macOS Keychain, Linux Secret Service, Windows Credential Manager)
- [ ] Encrypted file fallback (master password) when keychain unavailable
- [ ] Lock on app startup with OS biometric/password
- [ ] Manual lock button in signer panel
- [ ] Auto-lock after N minutes of inactivity (configurable)
- [ ] Never log nsec, never transmit outside Rust process

**NIP-07 API (injected as `window.nostr`)**
- [ ] `getPublicKey()` — returns active identity's pubkey hex
- [ ] `signEvent(event)` — signs and returns the event
- [ ] `getRelays()` — returns configured relays with read/write markers
- [ ] `nip44.encrypt(pubkey, plaintext)` / `nip44.decrypt(pubkey, ciphertext)`
- [ ] `nip04.encrypt` / `nip04.decrypt` (legacy, with deprecation banner)
- [ ] Signature verification before return (self-check)

**Permission model**
- [ ] Per-site, per-method approval storage
- [ ] Scopes: "Allow once," "Allow for session," "Allow always," "Deny"
- [ ] "Don't ask again" checkbox that persists the chosen scope
- [ ] Sites identified by nsite name or npub

**Approval prompt UI**
- [ ] Focus-stealing modal with site identity (name/npub + avatar)
- [ ] Method name + kind number + human-readable kind name
- [ ] Event content preview (truncated + expandable)
- [ ] Tags preview (key-value layout)
- [ ] `created_at` sanity check (warn if >1 day off)
- [ ] Warning banners for sensitive kinds (0, 3, 5, 10000, 10002)
- [ ] Approve / Deny buttons
- [ ] Scope selector
- [ ] Copy raw event button
- [ ] Keyboard shortcuts (Enter=approve, Esc=deny)
- [ ] Auto-deny timeout after 60s

**Signer management panel** (new side panel)
- [ ] View active identity + pubkey
- [ ] Switch identity (if multi-identity in v2)
- [ ] List all sites with stored permissions
- [ ] Revoke individual permissions or all for a site
- [ ] Lock signer button
- [ ] Settings entry point

**History & audit log**
- [ ] Last 100 signing events logged (timestamp, site, method, kind, approved/denied, scope applied)
- [ ] Viewable in signer panel
- [ ] Clear history button

**Integration**
- [ ] Bridge mechanism: content webview → `titan-cmd://nostr-request/...` → chrome → signer → `eval()` response callback
- [ ] Works without any external signer installed
- [ ] Prompt queue (stack multiple requests cleanly)

## Future

### Signer — deferred to v2+
- Multiple named identities (personal, work, alt)
- Per-tab / per-site identity override
- Identity switcher in toolbar (avatar menu)
- Incognito mode (ephemeral throwaway key per tab)
- NIP-06 BIP-39 mnemonic import/export
- Encrypted backup export/import
- Advanced approval scopes (N times, duration-based)
- Per-kind rules ("always allow kind 1 on this site, never kind 0")
- Rate limiting (max N signatures/minute)
- Batch approval UI for bulk signing
- Trust levels (untrusted/standard/trusted/full)
- NIP-19 entity expansion in event previews
- Markdown preview for kind 1 content
- Event kind database with risk levels
- Signer console (real-time request/response debug view)
- Dry-run mode
- Wipe on N failed unlock attempts
- Print backup (paper + QR)

### Signer — extension territory
- NIP-46 bunker URL (`bunker://...`) remote signer support
- Amber / nostrconnect mobile pairing
- Hardware wallet support (Trezor, Ledger via NIP-07-over-USB)
- Delegated signing (NIP-26)
- Multi-sig / FROST threshold signatures
- Import from Alby / nos2x

### Extension system (after signer is solid)
- Side panel extensions (nsites with `["t", "titan-extension"]` tag)
- `window.titanExt` message bus for content page ↔ extension communication
- Extensions as alternate `window.nostr` providers (NIP-07 proxies to external signers)
- Event-driven daemon extensions (passive Nostr listeners in Web Workers)
- Extension manager panel (install/uninstall, permissions, updates)

### Other future work
- Loading diagnostics: show why a site is slow (no kind 10002 relay list, no kind 10063 Blossom list, missing server tags in manifest, relay timeouts)
- History panel with search
- Name marketplace
- Mobile (iOS/Android via Tauri)
- Relay connection keepalive / reconnect after idle (fixes stale manifest errors after long idle)
