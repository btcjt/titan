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
Nostr relays (kind 10002 relay list -> kind 15128 site manifest)
  |
  v
Blossom servers (SHA256 hash -> file blob)
  |
  v
Rendered in native webview
```

Type a name, get a website. No DNS. No ICANN. No certificates. No hosting providers.

## The Name Protocol

Titan introduces a Bitcoin-native name system using OP_RETURN transactions. Names are permanent, first-in-chain-wins, and transferable.

```
OP_RETURN payload (80 bytes max):
  TITN  01  00  0a  westernbtc  <32-byte pubkey>
  ^^^^  ^^  ^^  ^^  ^^^^^^^^^^  ^^^^^^^^^^^^^^^^
  magic ver act len name        x-only Schnorr key
```

- **Register**: First valid `TITN` OP_RETURN for a name claims it forever
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
- **Database**: SQLite (bundled)
- **Nostr**: nostr-sdk
- **Bitcoin**: Core RPC for block scanning

## Status

Phase 1 — types and OP_RETURN codec. See [docs/roadmap.md](docs/roadmap.md).

## Building

```bash
# Prerequisites: Rust toolchain, system webview libraries
cargo build
cargo test
```

## License

MIT
