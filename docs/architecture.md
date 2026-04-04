# Titan Architecture

## Overview

Titan is a native desktop browser for the `nsite://` protocol. It resolves human-readable names registered on Bitcoin to websites hosted on Nostr relays and Blossom servers. No DNS, no certificates, no traditional hosting.

```
┌─────────────────────────────────────────────────────────┐
│                     Titan Browser                       │
│                   (Rust + Tauri 2)                       │
│                                                         │
│  Address Bar ──→ Name Resolution ──→ Manifest Fetch     │
│                        │                    │           │
│                  ┌─────┴─────┐        ┌─────┴─────┐     │
│                  │  Nostr    │        │  Nostr    │     │
│                  │  Index    │        │  Manifest │     │
│                  │ (k35129)  │        │(k15128/   │     │
│                  │           │        │  k35128)  │     │
│                  └───────────┘        └─────┬─────┘     │
│                                             │           │
│                                       ┌─────┴─────┐     │
│                                       │  Blossom  │     │
│                                       │  Servers  │     │
│                                       └─────┬─────┘     │
│                                             │           │
│                                       ┌─────┴─────┐     │
│                                       │  Webview  │     │
│                                       │  Render   │     │
│                                       └───────────┘     │
└─────────────────────────────────────────────────────────┘
```

The browser has no local database. All name resolution happens via Nostr relays.

## Components

### 1. Titan Browser (`crates/titan-app`)

Native desktop app built with Rust and Tauri 2. Uses the system webview for rendering.

**Responsibilities:**
- Parse `nsite://` URLs (names, npubs, hex pubkeys)
- Resolve names to pubkeys via Nostr index (kind 35129)
- Fetch site manifests from Nostr relays
- Fetch content blobs from Blossom servers
- Render HTML/CSS/JS in the webview
- Intercept `nsite://` link clicks within rendered pages

**Key files:**
- `src/main.rs` — Tauri app, `navigate` command, `nsite-content://` protocol handler
- `ui/index.html` — Browser chrome (address bar, nav buttons)
- `ui/app.js` — Navigation logic, history, error handling, default homepage (`nsite://titan`)
- `ui/style.css` — Titan moon theme (black + smokey amber)

**No local state.** No SQLite, no Bitcoin Core dependency. The browser is a pure Nostr + Blossom client.

### 2. Resolver (`crates/titan-resolver`)

Handles all Nostr relay queries and Blossom blob fetching.

**Modules:**
- `relay.rs` — Relay pool, race-then-linger search, manifest fetching, NSIT name lookup (kind 35129)
- `manifest.rs` — NIP-5A manifest parsing (v2 kind 15128/35128 + v1 kind 34128 fallback)
- `blossom.rs` — SHA256-verified blob downloading from Blossom servers
- `cache.rs` — Disk cache with TTLs (blobs forever, manifests 5min, relay lists 1hr)
- `lib.rs` — Top-level `Resolver` with `lookup_name()` and `resolve()` APIs

### 3. Bitcoin Module (`crates/titan-bitcoin`)

OP_RETURN codec, block scanner, and transaction builder. Used by the nsit-indexer service (not the browser).

**Modules:**
- `codec.rs` — NSIT wire format encode/decode
- `store.rs` — SQLite name index (used by the indexer service)
- `indexer.rs` — Block scanner with reorg detection
- `rpc.rs` — Bitcoin Core JSON-RPC client
- `tx.rs` — Transaction builder for name registration/transfer

### 4. Types (`crates/titan-types`)

Shared types: `TitanName`, `TitanOp`, `OpAction`, `NsiteUrl`, error types.

### 5. NSIT Indexer Service (`westernbtc-monorepo/services/nsit-indexer`)

Kubernetes service that watches Bitcoin blocks and publishes the name index as Nostr events. This is the bridge between Bitcoin and Nostr — browsers never touch Bitcoin directly.

**Architecture:**
```
Bitcoin Core (k8s) ──→ nsit-indexer ──→ Nostr Relays ──→ Titan Browser
     8332/TCP              │                                    │
                     ┌─────┴─────┐                        ┌────┴────┐
                     │ Kind 35129│  Name records           │ Query   │
                     │ Kind 15129│  Index stats            │ k35129  │
                     └───────────┘                        └─────────┘
```

**How it works:**
1. Connects to Bitcoin Core via k8s service DNS
2. Connects to Nostr relays
3. On startup: fetches its own last published stats (kind 15129) to resume from last synced height
4. Fetches its own published name records (kind 35129) to rebuild the in-memory registry
5. Scans new blocks for NSIT OP_RETURN outputs
6. Publishes kind 35129 for each registration/transfer (addressable, d=name)
7. Publishes kind 15129 stats after each sync
8. Polls every 30 seconds

**Files:**
- `src/index.ts` — Entry point, config, poll loop
- `src/lib/codec.ts` — NSIT OP_RETURN decoder (TypeScript port of Rust codec)
- `src/lib/rpc.ts` — Bitcoin Core JSON-RPC client
- `src/lib/scanner.ts` — Block scanner, name registry, transfer verification
- `src/lib/publisher.ts` — Nostr event publisher with ensureConnected pattern

### 6. Titan nsite (`westernbtc-monorepo/apps/titan-nsite`)

Static website published as `nsite://titan`. The browser's default homepage.

**What it does:**
- Name lookup via Nostr (queries kind 35129 from the indexer, race-then-linger)
- Index stats via Nostr (queries kind 15129)
- Client-side NSIT OP_RETURN generator for name registration (paste into any wallet)

**Published as:**
- Kind 35128 manifest with `d=titan` (for `nsite://titan`)
- Kind 15128 root manifest (for direct npub access)
- Blobs on `blossom.westernbtc.com`

**Files:**
- `app/page.tsx` — Name lookup UI, OP_RETURN generator, stats
- `lib/nostr.ts` — NDK client with race-then-linger search
- `scripts/publish-v2.mjs` — nsite v2 publisher

## Resolution Flow

### Name Resolution (two tiers)

```
User types: nsite://titan
     │
     ▼
1. Sync parse (instant)
   ├─ npub1...  → bech32 decode → pubkey
   ├─ 64-char hex → pubkey
   └─ base36 (50 chars + name) → pubkey + site name
     │
     │ (not a direct pubkey — must be a name)
     ▼
2. Nostr index lookup (~1 second)
   Query: kind=35129, d="titan", author=indexer_pubkey
   Pattern: race-then-linger (first relay + 200ms)
   Result: pubkey from "p" tag
     │
     │ (if not found)
     ▼
3. Not registered → error
```

### Content Resolution

```
pubkey + site_name + path
     │
     ▼
Relay Discovery (kind 10002)
   → Add the pubkey's preferred relays to the pool
     │
     ▼
Manifest Fetch
   ├─ Name: kind 35128 (d=site_name) → v2 manifest
   ├─ npub: kind 15128 (root) → v2 manifest
   └─ Fallback: kind 34128 (v1 per-file events) → assembled manifest
     │
     ▼
Path Resolution
   manifest.resolve_path("/blog/post.html")
   → SHA256 hash from path tags
     │
     ▼
Blob Fetch (SHA256-verified)
   1. Check disk cache (blobs cached forever)
   2. Try manifest's server tags
   3. Try pubkey's kind 10063 Blossom server list
   4. Try fallback servers (blossom.westernbtc.com)
   5. Verify SHA256 hash matches
     │
     ▼
Render in webview via nsite-content:// protocol
```

### Sub-resource Loading

When the webview renders HTML that references CSS, JS, images:

```
<link href="/assets/style.css">
     │
     ▼
Webview requests: nsite-content://localhost/assets/style.css
     │
     ▼
Tauri protocol handler intercepts
     │
     ▼
Uses current NavContext (pubkey + site_name)
     │
     ▼
Same manifest → blob pipeline (cached manifest, fast)
```

All sub-resource requests run concurrently (OnceCell resolver, RwLock nav context).

## Race-Then-Linger Search

All Nostr queries use this pattern:

```
Subscribe to filter across all relays
     │
     ▼
Wait for first event (up to 10 seconds)
     │
     ▼
First event received → start 200ms linger timer
     │
     ▼
Collect any additional events within 200ms
     │
     ▼
Return newest event by created_at
```

Used for: name lookups, relay lists, manifests, Blossom server lists.

## Nostr Event Kinds

| Kind | Type | Publisher | Purpose |
|------|------|-----------|---------|
| 10002 | Replaceable | Site owner | Relay list (NIP-65) |
| 10063 | Replaceable | Site owner | Blossom server list |
| 15128 | Replaceable | Site owner | Root site manifest (NIP-5A v2) |
| 15129 | Replaceable | Indexer service | NSIT index stats |
| 24242 | Ephemeral | Any uploader | Blossom upload auth (BUD-01) |
| 34128 | Addressable | Site owner | Per-file event (nsite v1, legacy) |
| 35128 | Addressable | Site owner | Named site manifest (NIP-5A v2, d=name) |
| 35129 | Addressable | Indexer service | NSIT name record (d=name) |

## Bitcoin Name Protocol (NSIT)

### Wire Format

```
Offset  Size  Field       Description
0       4     magic       "NSIT" (0x4E534954)
4       1     version     0x01
5       1     action      0x00=register, 0x01=transfer
6       1     name_len    1-41
7       N     name        [a-z0-9-]
7+N     32    pubkey      x-only Schnorr pubkey
```

Total: 39 + N bytes. Maximum 80 bytes (OP_RETURN limit).

### Rules
- First-in-chain wins (same block: lower tx index wins)
- UTXO-based ownership: first non-OP_RETURN output = ownership UTXO
- Transfer requires spending the ownership UTXO; new ownership UTXO = first non-OP_RETURN output of transfer tx
- If ownership UTXO is spent without NSIT OP_RETURN, name is permanently frozen
- Names: lowercase, 1-41 chars, no leading/trailing/consecutive hyphens

### Registration Flow
```
User → nsite://titan (OP_RETURN generator) → hex payload
User → Bitcoin wallet (Sparrow/Electrum/Core) → OP_RETURN tx → broadcast
  → First non-OP_RETURN output becomes ownership UTXO
Bitcoin network → confirmation
nsit-indexer → scans block → publishes kind 35129 event
Titan browser → queries kind 35129 → name resolves
```

### Transfer Flow
```
User → nsite://titan/transfer → generate transfer OP_RETURN hex
User → wallet: spend the ownership UTXO + include OP_RETURN → broadcast
  → First non-OP_RETURN output becomes new ownership UTXO
nsit-indexer → verifies UTXO spend → publishes updated kind 35129
```

## Caching

| Layer | Duration | Rationale |
|-------|----------|-----------|
| Name index | Via Nostr events | Replaceable, always fresh from relays |
| Relay list (kind 10002) | 1 hour | Changes infrequently |
| Manifest (kind 15128/35128) | 5 minutes | Updated on site deploy |
| Blobs | Forever | Content-addressed (SHA256), immutable |
| Blossom server list (kind 10063) | 1 hour | Rarely changes |

## Infrastructure

### k8s Cluster
```
┌─────────────────────────────────────────────┐
│  cicd namespace                              │
│                                              │
│  nsit-indexer ──→ bitcoin-rpc (8332)         │
│       │                                      │
│       └──→ relay.westernbtc.com (443)        │
│       └──→ relay.primal.net (443)            │
│       └──→ relay.damus.io (443)              │
│                                              │
│  westernbtc-blossom-server                   │
│  westernbtc-relay                            │
│  btc-nostrary                                │
│  ...                                         │
└─────────────────────────────────────────────┘

┌─────────────────────────────────────────────┐
│  Bitcoin Node (10.43.128.100)                │
│                                              │
│  bitcoind (RPC 8332, P2P 8333, ZMQ 28332)   │
│  lnd (REST 8080, gRPC 10009)                 │
└─────────────────────────────────────────────┘
```

## nsite Versions

| | v1 (nsite-cli) | v2 (NIP-5A) |
|---|---|---|
| Kind | 34128 | 15128 (root) / 35128 (named) |
| Model | One event per file | Single manifest event |
| Path | d-tag = relative path | `["path", "/abs/path", "sha256"]` tags |
| Hash | x or sha256 tag | Third element of path tag |

Titan supports both. v2 is preferred, v1 is a fallback.

## Registered Names

| Name | Pubkey | Block | Txid |
|------|--------|-------|------|
| titan | bec1a370...5cde44 | 943619 | 322ab880...e7002f |
| westernbtc | (registered for testing) | | |
