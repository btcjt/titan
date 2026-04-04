# Titan

**A native `nsite://` browser for the Nostr web.**

Named after Titan, the largest moon of Saturn — shrouded in a dense amber atmosphere, hiding an entire world beneath.

Titan is a desktop browser that resolves `nsite://` URLs using Nostr relay infrastructure and renders static websites stored on Blossom servers. It implements [NIP-5A](https://github.com/nostr-protocol/nips/blob/master/5A.md) (nsite v2) and introduces a Bitcoin-native name registration protocol for permanent, decentralized domain ownership.

## How It Works

```
nsite://westernbtc
  |
  v
Bitcoin OP_RETURN index (name -> pubkey)
  |
  v
Nostr relays (kind 10002 relay list -> kind 35128 site manifest)
  |
  v
Blossom servers (SHA256 hash -> file blob)
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

Two names registered for testing (`titan`, `westernbtc`). Everything else is unclaimed. Fair launch.

> *"It might make sense just to get some in case it catches on."* — Satoshi Nakamoto

## The Name Protocol

Titan introduces a Bitcoin-native name system using OP_RETURN transactions. Names are permanent, first-in-chain-wins, and transferable.

```
OP_RETURN payload (80 bytes max):
  NSIT  01  00  0a  westernbtc   <32-byte pubkey>
  ^^^^  ^^  ^^  ^^  ^^^^^^^^^^  ^^^^^^^^^^^^^^^^
  magic ver act len name        x-only Schnorr key
```

- **Register**: First valid `NSIT` OP_RETURN for a name claims it forever
- **Transfer**: Spend from the registration address with a new pubkey
- **Names**: `a-z`, `0-9`, hyphens. 1-41 characters. Lowercase only.

See [docs/whitepaper.md](docs/whitepaper.md) for the full protocol specification.

## Architecture

```
titan/
  crates/
    titan-types/      Core types — names, URLs, errors
    titan-bitcoin/    OP_RETURN codec, block scanner, SQLite index
    titan-resolver/   Nostr relay queries, Blossom fetching, cache
    titan-app/        Tauri desktop shell + nsite:// protocol handler
```

- **Language**: Rust
- **Desktop**: Tauri 2 (system webview)
- **Name index**: Nostr events (kind 35129/15129) — no Bitcoin Core required for browsing
- **Nostr**: nostr-sdk with race-then-linger search
- **Bitcoin**: Core RPC for block scanning (optional, for indexer service and direct registration)
## Status

Phases 1–7 complete. `nsite://titan` is live — registered on Bitcoin mainnet, published as nsite v2, loads as the browser's default homepage. Name lookups via Nostr relays. No Bitcoin Core required. See [docs/roadmap.md](docs/roadmap.md).

## Building

```bash
# Prerequisites: Rust toolchain, system webview libraries
cargo build
cargo test

# Run the browser
cargo tauri dev
```

## License

MIT
