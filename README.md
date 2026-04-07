# Titan

**A native `nsite://` browser for the Nostr web.**

Named after Titan, the largest moon of Saturn — shrouded in a dense amber atmosphere, hiding an entire world beneath.

Titan is a desktop browser that resolves `nsite://` URLs using Nostr relay infrastructure and renders static websites stored on Blossom servers. It implements [NIP-5A](https://github.com/nostr-protocol/nips/blob/master/5A.md) (nsite v2) and introduces a Bitcoin-native name registration protocol for permanent, decentralized domain ownership.

## How It Works

```
nsite://titan
  |
  v
Nostr name index (kind 35129 → pubkey)
  |
  v
Nostr relays (kind 10002 relay list → kind 35128 site manifest)
  |
  v
Blossom servers (SHA256 hash → file blob)
  |
  v
Rendered in native webview
```

Type a name, get a website. No DNS. No ICANN. No certificates. No hosting providers.

## Why

Nostr gives everyone a cryptographic identity — a public key. But public keys look like this:

```
npub1qe3e2054qkxsyt0yzem0xxv5gdpgmstahaqma3ja6pv2n9auqelqh2q4jf
```

Nobody can remember that. Nobody can type it. Nobody is putting that on a business card.

The traditional web solved this problem with domain names — but domain names are controlled by ICANN, can be seized by governments, expire if you miss a payment, and cost money every year. Current nsite gateways just move the npub into a subdomain (`npub1qe3e....gateway.com`), which doesn't solve readability and reintroduces DNS as a single point of failure.

Titan replaces all of that. Register a name once on Bitcoin for ~$0.10. It's yours forever. No renewals. No registrar. No one can take it from you.

```
nsite://westernbtc
```

That's it. Simple enough to say out loud, short enough for a billboard, permanent as the blockchain.

**Names are first-come, first-served, and permanent.** There are only 36 possible single-character names. Only ~1,300 two-character names. Words like `bitcoin`, `wallet`, `news`, `shop` — each can only be claimed once, ever. There is no expiration, no appeals process, no second chance.

Two names registered for testing (`titan` and `bitcoin`). Everything else is unclaimed. Fair launch.

## The Name Protocol

Titan introduces a Bitcoin-native name system using OP_RETURN transactions. Names are permanent, first-in-chain-wins, and transferable.

```
OP_RETURN payload (80 bytes max):
  NSIT  01  00  0a  westernbtc   <32-byte pubkey>
  ^^^^  ^^  ^^  ^^  ^^^^^^^^^^  ^^^^^^^^^^^^^^^^
  magic ver act len name        x-only Schnorr key
```

- **Register**: First valid `NSIT` OP_RETURN for a name claims it forever
- **Transfer**: Spend the ownership UTXO with a new pubkey in the OP_RETURN
- **Ownership**: UTXO-based — whoever can spend the ownership output controls the name
- **Names**: `a-z`, `0-9`, hyphens. 1-41 characters. Lowercase only.

See [docs/whitepaper.md](docs/whitepaper.md) for the full protocol specification and [docs/architecture.md](docs/architecture.md) for the system architecture.

## Architecture

```
titan/
  crates/
    titan-types/      Core types — names, URLs, errors
    titan-bitcoin/    OP_RETURN codec, block scanner, UTXO indexer
    titan-resolver/   Nostr relay queries, Blossom fetching, cache
    titan-app/        Tauri desktop shell — two-webview architecture
```

**Browser** (multi-webview, tabbed):

- Chrome webview: tab strip (in titlebar), address bar, back/forward/refresh, side panels (bookmarks, console, settings)
- Per-tab content webviews: nsite content via `nsite-content://` protocol with URL-encoded site identity
- `on_navigation`: intercepts `nsite://` links, keyboard shortcuts via `titan-cmd://`
- `on_page_load`: syncs content URL back to address bar, injects console forwarding
- Native webview history per tab for back/forward

**Stack**:

- **Language**: Rust
- **Desktop**: Tauri 2 (system webview, multi-webview, overlay titlebar)
- **Name index**: Nostr events (kind 35129/1129/15129) via race-then-linger search
- **Site manifests**: NIP-5A v2 (kind 15128/35128) with v1 fallback
- **Content storage**: Blossom servers (SHA256-verified blobs)
- **Name ownership**: Bitcoin UTXO-based (OP_RETURN registration + transfer)

**Related** (in [westernbtc-monorepo](https://github.com/btcjt/westernbtc-monorepo)):

- `apps/titan-nsite/` — `nsite://titan` homepage (search, register, transfer, browse, guide)
- `services/nsit-indexer/` — k8s service: watches Bitcoin blocks → publishes name index as Nostr events

## Status

Phases 1–7 complete. `nsite://titan` is live — registered on Bitcoin mainnet, published as nsite v2, loads as the browser's default homepage. Name lookups via Nostr relays with race-then-linger search. No Bitcoin Core required. See [docs/roadmap.md](docs/roadmap.md).

## Building

```bash
# Prerequisites: Rust toolchain, system webview libraries
cargo build
cargo test

# Run the browser
cargo tauri dev
```

Nix users: `direnv allow` first

## Docs

- [Architecture](docs/architecture.md) — system design, resolution flow, event kinds
- [Whitepaper](docs/whitepaper.md) — protocol design and security model
- [Name Protocol](docs/name-protocol.md) — wire format, UTXO ownership, Nostr index
- [Roadmap](docs/roadmap.md) — build phases and status

## License

MIT
