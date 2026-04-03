# Titan: A Bitcoin-Native Name System for the Nostr Web

**Version 0.1 — Draft**

## Abstract

Titan is a native desktop browser for Nostr-hosted static websites (nsites). It introduces a Bitcoin-native name registration protocol that permanently maps human-readable names to Nostr public keys using OP_RETURN transactions. Combined with the NIP-5A nsite protocol, Titan provides a fully decentralized web browsing experience with no reliance on DNS, certificate authorities, or traditional hosting infrastructure.

## 1. Problem

The current web depends on centralized chokepoints:

- **DNS** — ICANN controls the root, registrars can seize domains, governments can order takedowns
- **Certificate Authorities** — a handful of organizations decide who gets HTTPS
- **Hosting providers** — a company can deplatform any site at any time
- **Browsers** — four corporations control how the web renders

NIP-5A (nsite v2) solves the hosting and serving layer by storing website content on Blossom blob servers and indexing it through Nostr relay events. But it leaves a critical gap: **name resolution**.

### 1.1 The npub Problem

Every Nostr identity is a public key. In its human-readable form, an npub looks like this:

```
npub1qe3e2054qkxsyt0yzem0xxv5gdpgmstahaqma3ja6pv2n9auqelqh2q4jf
```

This is 63 characters of seemingly random letters and numbers. It is:

- **Impossible to remember** — no human can memorize or type this reliably
- **Error-prone** — a single wrong character points to a different identity (or no identity at all)
- **Unshareable** — you can't tell someone your address over the phone, print it on a business card, or say it in conversation
- **Hostile to adoption** — asking a non-technical user to navigate to `npub1qe3e...` is a non-starter

The traditional web solved this decades ago with domain names. `google.com` is memorable. `142.250.80.14` is not. Nostr has the same problem — the underlying identifiers work perfectly for machines but are unusable by humans.

### 1.2 Current Workarounds Fall Short

Existing nsite gateways use subdomain-based addressing: `<npub>.gateway.com`. This has two problems:

1. **It doesn't actually solve readability** — the npub is still in the URL, just moved to a subdomain
2. **It reintroduces DNS dependency** — the gateway domain is controlled by a registrar, hosted by a company, and resolvable only through ICANN's system. The entire decentralization benefit is negated by the fact that one DNS seizure takes down the gateway for everyone.

NIP-05 verification (`user@domain.com`) maps names to pubkeys but requires a web server at that domain — again, DNS-dependent.

### 1.3 What's Needed

The missing piece is a **decentralized, human-readable name system** that maps simple names to Nostr pubkeys without depending on any centralized authority. It should be:

- As easy to type as a domain name
- Permanent — no annual renewals, no expiration, no seizure
- Decentralized — no single entity controls the registry
- Verifiable — anyone can independently confirm that a name maps to a pubkey

Titan solves this by anchoring name registrations in Bitcoin's blockchain — the most secure, immutable, and censorship-resistant ledger in existence.

### 1.4 The Name Land Rush

Because names are permanent and first-come-first-served, the launch of the Titan Name Protocol creates a one-time land rush. Unlike DNS where desirable names can be reclaimed when registrations lapse, a Titan name claimed today is claimed forever. There are no second chances.

Consider: there are only 36 possible single-character names (`a`-`z`, `0`-`9`). Only ~1,300 two-character combinations. Common words like `bitcoin`, `wallet`, `news`, `shop`, `music` — each can only be claimed once. The first person to broadcast a valid registration transaction owns that name for as long as Bitcoin exists.

This isn't a flaw — it's an intentional feature. Scarce, permanent names have value precisely because they cannot be inflated or revoked. The transfer mechanism allows names to change hands, creating a market. But there is no undo, no appeals process, no governance committee. The blockchain is the only authority.

Early participants who understand this will move quickly.

## 2. The Name Protocol

### 2.1 Design Goals

- **Permanent**: Once registered, a name cannot be revoked by any authority
- **First-come, first-served**: The first valid registration on the Bitcoin blockchain wins
- **Transferable**: Name ownership can be transferred by the current owner
- **Minimal**: The protocol fits within Bitcoin's 80-byte OP_RETURN limit
- **Self-sovereign**: No registration authority, no annual fees, no approval process

### 2.2 Wire Format

Every Titan name operation is encoded in a single OP_RETURN output:

```
Offset  Size  Field       Description
0       4     magic       "NSIT" (0x4E534954)
4       1     version     0x01
5       1     action      0x00 = register, 0x01 = transfer
6       1     name_len    Length of name (1-41 bytes)
7       N     name        ASCII name [a-z0-9-]
7+N     32    pubkey      32-byte x-only Schnorr public key
```

**Total**: 39 + N bytes, where N is the name length. Maximum 80 bytes.

### 2.3 Name Rules

- Characters: lowercase ASCII letters (`a-z`), digits (`0-9`), and hyphens (`-`)
- No leading or trailing hyphens
- No consecutive hyphens (`--`)
- Minimum length: 1 character
- Maximum length: 41 characters
- Case-insensitive (automatically lowercased)

### 2.4 Registration

To register a name, create a Bitcoin transaction with an OP_RETURN output containing the encoded payload with action `0x00` (register). The `pubkey` field specifies the Nostr public key that the name resolves to.

If the name has already been registered in an earlier block (or earlier in the same block by transaction index), the registration is ignored. **First-in-chain wins.**

### 2.5 Transfer

To transfer a name, create a Bitcoin transaction where:

1. The first input spends from an address controlled by the current owner (the address that funded the registration or most recent transfer)
2. The OP_RETURN output contains the encoded payload with action `0x01` (transfer)
3. The `pubkey` field specifies the new Nostr public key

This creates a chain of ownership rooted in the original registration transaction. No separate key management is needed — whoever controls the Bitcoin UTXO controls the name.

### 2.6 Indexing

An indexer scans the Bitcoin blockchain for OP_RETURN outputs matching the `NSIT` magic prefix. For each valid payload:

- **Register**: If the name is unclaimed, record the mapping (name → pubkey, owner address, txid, block height)
- **Transfer**: If the name exists and the transaction's first input is from the current owner address, update the pubkey and owner address

The index is deterministic — any node scanning the same blockchain will arrive at the same name→pubkey mappings.

## 3. Resolution Flow

When a user navigates to `nsite://westernbtc`:

1. **Name lookup**: Query the local Bitcoin name index for "westernbtc" → returns a 32-byte Nostr pubkey
2. **Relay discovery**: Query fallback relays for the pubkey's kind 10002 (NIP-65 relay list) event
3. **Manifest fetch**: Query the pubkey's relays for kind 15128 (root site manifest) or kind 35128 (named site manifest) per NIP-5A
4. **Path resolution**: Match the requested path against the manifest's `path` tags to get a SHA256 blob hash
5. **Blob fetch**: Retrieve the blob from Blossom servers (listed in the manifest's `server` tags, the pubkey's kind 10063 event, or fallback servers)
6. **Render**: Display the content in the native webview

Sub-resources (CSS, JS, images) referenced by the HTML resolve through the same pipeline using the cached manifest.

## 4. Caching Strategy

Content-addressed storage enables aggressive caching:

| Layer | Cache Duration | Rationale |
|-------|---------------|-----------|
| Name index | Always fresh | SQLite, updated per Bitcoin block |
| Relay list (kind 10002) | 1 hour | Replaceable event, changes infrequently |
| Site manifest (kind 15128/35128) | 5 minutes | Replaceable event, updated on deploy |
| Blobs | Forever | SHA256-addressed, immutable by definition |
| Blossom server list (kind 10063) | 1 hour | Rarely changes |

Blobs are the bulk of cached data and never need invalidation. A site "update" means a new manifest pointing to new hashes — old blobs remain valid and shared resources (common libraries, unchanged images) hit cache automatically.

## 5. Security Model

- **Name integrity**: Secured by Bitcoin proof-of-work. Reversing a registration requires a 51% attack.
- **Content integrity**: Every blob is verified against its SHA256 hash. Blossom servers cannot serve tampered content.
- **Manifest authenticity**: Nostr events are cryptographically signed by the site owner's keypair. Relays cannot forge manifests.
- **No MITM**: The entire chain from name→pubkey→manifest→content is cryptographically verified. No certificates needed.

### What Titan does NOT protect against

- A relay withholding events (mitigated by querying multiple relays)
- All Blossom servers going offline (mitigated by fallback servers and local cache)
- A Bitcoin chain reorganization removing a recent registration (mitigated by waiting for confirmations)
- The site owner publishing malicious content (same as the current web — Titan authenticates the author, not the content)

## 6. Comparison

| | Traditional Web | nsite v2 (gateway) | Titan |
|---|---|---|---|
| Name system | DNS (ICANN) | DNS subdomain | Bitcoin OP_RETURN |
| Name cost | $10-50/year | Free (uses npub) | One-time tx fee (~$0.10) |
| Name permanence | Expires annually | Tied to gateway | Permanent (Bitcoin) |
| Hosting | Server rental | Blossom (free) | Blossom (free) |
| TLS | Certificate authority | Gateway handles | Not needed (hash-verified) |
| Censorship | Domain seizure, hosting takedown | Gateway can block | No single point of failure |
| Client | Any browser | Any browser via gateway | Titan (native) |

## 7. Future Work

- **Browser extensions**: Plugin system for nsite-native applications
- **Multi-tab browsing**: Tabbed interface with session management
- **Bookmarks and history**: Local storage of frequently visited nsites
- **Name marketplace**: Transfer protocol enables buying/selling names
- **Light client mode**: Query a trusted indexer API instead of running Bitcoin Core
- **Mobile**: Tauri supports iOS and Android (post-desktop MVP)
- **Search**: Nostr-native search indexing of nsite content

## 8. Implementation

Titan is implemented in Rust using the Tauri framework for native desktop rendering. The codebase is organized as a Cargo workspace:

- `titan-types` — Core types (names, URLs, errors)
- `titan-bitcoin` — OP_RETURN codec, block scanner, SQLite name index
- `titan-resolver` — Nostr relay queries, Blossom blob fetching, disk cache
- `titan-app` — Tauri desktop application with `nsite://` protocol handler

Source: [github.com/btcjt/titan](https://github.com/btcjt/titan)

## References

- [NIP-5A: Pubkey Static Websites (nsite v2)](https://github.com/nostr-protocol/nips/blob/master/5A.md)
- [NIP-65: Relay List Metadata](https://github.com/nostr-protocol/nips/blob/master/65.md)
- [BUD-01: Blossom Server Protocol](https://github.com/hzrd149/blossom)
- [BIP-340: Schnorr Signatures](https://github.com/bitcoin/bips/blob/master/bip-0340.mediawiki)
