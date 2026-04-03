# Titan — nsite:// browser

> Read this at the start of every session.

## What is Titan

A native desktop browser that resolves `nsite://` URLs using Nostr relays and renders static websites stored on Blossom servers. Implements NIP-5A (nsite v2) and introduces a Bitcoin-native name registration protocol.

## Tech Stack

- **Language**: Rust (edition 2024)
- **Desktop framework**: Tauri 2 (system webview, cross-platform)
- **Database**: SQLite via rusqlite (bundled)
- **Nostr**: nostr-sdk for relay connections
- **Bitcoin**: Core RPC for scanning OP_RETURN transactions
- **Theme**: Titan moon of Saturn — dark/black with amber accents

## Repo Structure

Cargo workspace with 4 crates:

```
crates/
  titan-types/      Core types: TitanName, TitanOp, NsiteUrl, errors
  titan-bitcoin/    OP_RETURN codec, block scanner, SQLite name index
  titan-resolver/   Nostr relay queries, Blossom blob fetching, disk cache
  titan-app/        Tauri desktop shell + nsite:// protocol handler
```

## Bitcoin Name Protocol (TNP)

OP_RETURN wire format (80 bytes max):

```
Offset  Size  Field       Description
0       4     magic       "TITN" (0x5449544E)
4       1     version     0x01
5       1     action      0x00=register, 0x01=transfer
6       1     name_len    1-41
7       N     name        [a-z0-9-], no leading/trailing/consecutive hyphens
7+N     32    pubkey      x-only Schnorr pubkey (same as Nostr pubkey)
```

- First-in-chain wins for registration
- Transfer: first input must spend from current owner address
- Names: lowercase only, DNS-like charset, max 41 chars
- Mainnet only

## Resolution Flow

```
nsite://westernbtc
  → SQLite: name → pubkey
  → Relays: kind 10002 (relay list) → kind 15128/35128 (manifest)
  → Blossom: SHA256 hash → blob
  → Webview: render
```

## Hardcoded Fallbacks

Relays: wss://relay.westernbtc.com, wss://relay.primal.net, wss://relay.damus.io
Blossom: https://blossom.westernbtc.com, https://nostr.build

## Caching Strategy

- Name index: SQLite, always fresh (updated per block)
- Manifests (kind 15128/35128): 5 min TTL
- Relay lists (kind 10002): 1 hour TTL
- Blobs: forever (content-addressed, immutable)
- Blossom server lists (kind 10063): 1 hour TTL

## Build Phases

1. ~~Types + OP_RETURN codec~~ (DONE — 13 tests passing)
2. Bitcoin RPC client + SQLite store (NEXT)
3. Block scanner / indexer
4. Nostr resolver (relays + Blossom)
5. Tauri browser shell + protocol handler
6. Integration + error states
7. Distribution (dmg/AppImage/msi, GitHub Actions)

## Key Decisions Made

- Tauri over Electron/CEF (small binary, system webview)
- nostr-sdk over raw nostr crate (includes relay pool)
- Bitcoin Core RPC over Electrum (user already runs a node)
- Embedded indexer in app (no separate server for MVP)
- x-only 32-byte pubkeys (matches Nostr, saves 1 byte vs compressed)
- "TITN" 4-byte magic prefix
- SQLite with bundled feature (no system dependency)
- `nsite://` scheme (protocol name, not browser name — other browsers could implement)

## Docs

- `docs/whitepaper.md` — full protocol design and security model
- `docs/name-protocol.md` — TNP wire format spec
- `docs/roadmap.md` — phased build plan with checkboxes

## GitHub

Public repo: github.com/btcjt/titan (HTTPS remote, gh CLI auth as btcjt)

## Privacy

Use "Josh" only — no full name anywhere in the codebase.
