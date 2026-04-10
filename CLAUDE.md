# Titan — nsite:// browser

> Read this at the start of every session.

## What is Titan

A native desktop browser that resolves `nsite://` URLs using Nostr relays and renders static websites stored on Blossom servers. Implements NIP-5A (nsite v2) and introduces a Bitcoin-native name registration protocol.

## Tech Stack

- **Language**: Rust (edition 2024)
- **Desktop framework**: Tauri 2 (system webview, cross-platform)
- **Nostr**: nostr-sdk for relay connections + race-then-linger search
- **Name index**: Nostr events (kind 35129/15129) — no local database, no Bitcoin Core
- **Theme**: Titan moon of Saturn — dark/black with amber accents

## Repo Structure

Cargo workspace with 4 crates:

```
crates/
  titan-types/      Core types: TitanName, TitanOp, NsiteUrl, errors
  titan-bitcoin/    OP_RETURN codec, block scanner, tx builder (used by nsit-indexer service)
  titan-resolver/   Nostr relay queries, Blossom blob fetching, disk cache, name lookup
  titan-app/        Tauri desktop shell, signer, permissions, nsite:// + titan-nostr:// protocols
```

Related (in westernbtc-monorepo):
```
apps/titan-nsite/        nsite://titan — search, register, transfer, browse names (static nsite v2)
services/nsit-indexer/   k8s service: watches Bitcoin blocks, publishes name index as Nostr events
```

## Bitcoin Name Protocol

OP_RETURN wire format (80 bytes max):

```
Offset  Size  Field       Description
0       4     magic       "NSIT" (0x4E534954)
4       1     version     0x01
5       1     action      0x00=register, 0x01=transfer
6       1     name_len    1-41
7       N     name        [a-z0-9-], no leading/trailing/consecutive hyphens
7+N     32    pubkey      x-only Schnorr pubkey (same as Nostr pubkey)
```

- First-in-chain wins for registration
- Same-block conflicts: lower transaction index wins
- UTXO-based ownership: first non-OP_RETURN output = ownership UTXO
- Transfer: must spend the ownership UTXO; new ownership = first non-OP_RETURN output of transfer tx
- Full ownership transfer built in (send UTXO to new address)
- If ownership UTXO spent without NSIT OP_RETURN → name permanently frozen
- Names: lowercase only, DNS-like charset, max 41 chars
- Mainnet only

## URL Spec

```
nsite://<host>[/<path>]

host = <bitcoin-name> | npub1<bech32>
path = file path within manifest (default: /)
```

No extensions, no subdomains, no TLDs. One name = one site.

Reserved hosts: `settings`, `history`, `bookmarks` (browser internals).
`nsite://titan` is a real registered name serving the name manager UI through the nsite stack itself.

## Resolution Flow

Name resolution uses a three-tier strategy with race-then-linger search:

**1. Sync parse** (instant): npub bech32 decode, hex pubkey, base36
**2. Nostr index** (fast, ~1s): kind 35129 from indexer service, race-then-linger from relays
**3. Not found**: name is unregistered

**Bitcoin name** (`nsite://westernbtc`):
```
westernbtc → Nostr index (kind 35129, d=westernbtc) → pubkey
  → Relays: kind 10002 (relay list)
  → Relays: kind 35128 (manifest, d=westernbtc)
  → Blossom: SHA256 hash → blob → render
```

**Direct npub** (`nsite://npub1...`):
```
npub → pubkey (bech32 decode)
  → Relays: kind 10002 (relay list)
  → Relays: kind 15128 (root manifest)
  → Blossom: SHA256 hash → blob → render
```

## Nostr Name Index (published by nsit-indexer service)

- **Kind 35129** (addressable, d=name): current name state — pubkey, owner UTXO, txid, block height
- **Kind 1129** (regular): name history log — one event per action, never replaced, full chain of custody
- **Kind 15129** (replaceable): index stats — block height, hash, total names
- **Indexer pubkey**: `bec1a370130fed4fb9f78f9efc725b35104d827470e75573558a87a9ac5cde44`
- Race-then-linger query: first relay response + 200ms linger window, newest event wins

## Race-Then-Linger Search

All Nostr queries use the same custom search pattern:
1. Subscribe to filter, stream events
2. On first event received, start 200ms linger timer
3. Collect any additional events that arrive within the linger window
4. Return the newest event by `created_at`

This gives fast perceived latency while still picking up fresher data from slower relays.

## Hardcoded Fallbacks

Relays: wss://relay.westernbtc.com, wss://relay.primal.net, wss://relay.damus.io
Blossom: https://blossom.westernbtc.com

## Caching Strategy

- Name index: Nostr events (kind 35129), queried from relays on demand
- Manifests (kind 15128/35128): 5 min TTL
- Relay lists (kind 10002): 1 hour TTL
- Blobs: forever (content-addressed, immutable)
- Blossom server lists (kind 10063): 1 hour TTL

## nsite Versions

- **v1** (nsite-cli): Kind 34128, one event per file, d=path, x=sha256
- **v2** (NIP-5A): Kind 15128 (root) / 35128 (named), single manifest with path tags
- Titan resolver supports both (v2 preferred, v1 fallback)
- Titan publishes v2 only

## Build Phases

1. ~~Types + OP_RETURN codec~~ (DONE)
2. ~~Bitcoin RPC client + SQLite store~~ (DONE)
3. ~~Block scanner / indexer~~ (DONE)
4. ~~Nostr resolver (relays + Blossom)~~ (DONE)
5. ~~Tauri browser shell + protocol handler~~ (DONE)
6. ~~Integration + error states~~ (DONE — 57 tests, nsite://titan live)
7. ~~nsite://titan + Nostr Index~~ (DONE — search/register/transfer/browse, nsit-indexer deployed)
8. ~~Distribution~~ (DONE — GitHub Actions CI/CD, dmg/AppImage/msi, zapstore)
9. ~~Tabs, Console, Interactive Registration~~ (DONE — multi-tab, console forwarding, bitcoin-cli builder)
10. Built-in Signer (IN PROGRESS — key management, window.nostr bridge, permissions + approval prompts, audit log, getRelays plumbing, auto-updater, CSP hardening done; auto-lock, encrypted file fallback remaining)
11. ~~Developer Tools~~ (DONE — v0.1.7: three-tab dev console with Logs/Network/Application, ring-buffered network log with copy-as-cURL, fetch/XHR/WebSocket wrappers, `nsite-content://` + `titan-nostr://` request capture, localStorage/sessionStorage/cookies viewer via webview.eval readback, drag-to-resize side panel with persisted width, level + source filters on Rust log stream)

## Key Decisions Made

- Tauri over Electron/CEF (small binary, system webview)
- nostr-sdk over raw nostr crate (includes relay pool)
- Nostr-published name index (no local database, no Bitcoin Core for browsers)
- Browser is a pure Nostr + Blossom client — no local state beyond blob cache
- Race-then-linger search for all Nostr queries (fast + fresh)
- "NSIT" 4-byte magic prefix (protocol name, not browser name)
- `nsite://` scheme (protocol name, not browser name — other browsers could implement)
- Name = site: registered Bitcoin name IS the d-tag for kind 35128
- npub = root site: kind 15128, one per pubkey, no d-tag needed
- No file extensions in URLs — the scheme is the protocol signal
- Sub-resources served via `nsite-content://` custom Tauri protocol
- nsite://titan is a real dogfooded nsite with search, register, transfer, browse pages
- Browser default homepage is nsite://titan
- Removed SQLite and Bitcoin Core from browser — pure Nostr client
- Removed built-in name manager — all name operations happen on nsite://titan
- Kind 35129/1129/15129 for the Nostr name index (published by nsit-indexer service)
- v1 nsite fallback (kind 34128) for compatibility with existing sites
- Built-in signer uses OS keychain (keyring v3), no external signer needed
- Per-site permission model with AllowOnce/Session/Always/DenyAlways scopes
- Prompt queue capped at 16/site + 128 global to block memory-exhaustion DoS
- `nsite-content://` responses ship strict CSP (`connect-src 'self' titan-nostr:`) to prevent exfil
- Site origin read server-side from tab state, never from request body (spoofing defense)
- Auto-updater via tauri-plugin-updater + minisign signature verification
- Windows WebView2 workaround: `platform_navigate_url` rewrites `nsite-content://` → `http://nsite-content.` on every navigate call (wry only rewrites at creation time)
- Titan Bookmarks minted as its own kind (10129, replaceable, `xx129` convention) instead of reusing NIP-51 kind 10003 — clean semantics for `nsite://` URLs, no cross-client tag collisions (see `docs/titan-bookmarks.md`)
- Dev console network capture uses two paths: Rust protocol handlers (`nsite-content://`, `titan-nostr://`) push into the `devtools::DevtoolsState` ring buffer directly, and content-page JS wraps `fetch`/`XMLHttpRequest`/`WebSocket` at page load, posting events back to chrome via `titan-cmd://net-event/<json>` URLs intercepted by the navigation handler. Ring buffer capped at 500 events to bound memory on long-running sessions.
- Dev console Application tab reads `localStorage`/`sessionStorage`/`document.cookie` from the active content webview via `webview.eval()` — injects a reader script that POSTs the snapshot back via `titan-cmd://devtools-storage/<json>`. Mutations (delete key, clear all) also go through `webview.eval()` for symmetry.
- Side panel width is a CSS custom property (`--panel-width`) updated live during drag, persisted via a dedicated `update_side_panel_width` Tauri command to avoid races with concurrent Settings panel edits. Clamped to [280px, 1400px] both at the Rust layer (on save) and the JS layer (during drag).

## Registered Names

- `titan` — txid: 322ab8800aa8d926161ff398d5d0b6c851c66679830fe05b223a548794e7002f (block 943619)
- `bitcoin` — txid: 673af03fef0aa5af59fd350927898bef8e4a2fc6e4d4ca6508d7cd82b4609155 (block 943978)

## Docs

- `docs/architecture.md` — full system architecture (browser, resolver, indexer, nsite, infra)
- `docs/whitepaper.md` — protocol design and security model
- `docs/name-protocol.md` — wire format spec + Nostr index event kinds
- `docs/titan-bookmarks.md` — Titan Bookmarks protocol (kind 10129, private, encrypted)
- `docs/roadmap.md` — phased build plan with checkboxes
- `docs/blog-announcement.md` — launch announcement draft

## GitHub

Public repo: github.com/btcjt/titan (HTTPS remote, gh CLI auth as btcjt)

## Privacy

Use "Josh" only — no full name anywhere in the codebase.
