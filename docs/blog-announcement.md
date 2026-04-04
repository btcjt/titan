# Titan: Solving the Last Problem of the Decentralized Web

**Two names have been registered for testing: `titan` and `westernbtc`. Everything else is unclaimed. No insider allocation, no reserved list. Fair launch.**

---

Nostr solved identity. Blossom solved storage. NIP-5A solved hosting. But there's been one glaring problem left: **nobody can remember a public key.**

```
npub1qe3e2054qkxsyt0yzem0xxv5gdpgmstahaqma3ja6pv2n9auqelqh2q4jf
```

Try to type that from memory. Try to read it to someone over the phone.

That's been the bottleneck for the entire Nostr web. You can build a site, host it on Blossom, serve it through relays — but the only way to reach it is through a 63-character string of gibberish, or through a gateway that quietly reintroduces the same DNS dependency you were trying to escape.

## The Problem with DNS

The traditional web solved this in 1985 with the Domain Name System. Instead of remembering `142.250.80.14`, you type `google.com`. It works. But DNS is controlled by ICANN, managed by registrars, and runs on a system where governments can seize your domain, corporations can outbid you on renewals, and your name expires the moment you miss a payment. You don't own your domain — you rent it.

Current nsite solutions haven't fixed this. Gateways serve sites at addresses like `npub1qe3e...gateway.com` — still unreadable, still dependent on the gateway's DNS entry. NIP-05 verification maps usernames to pubkeys, but requires hosting a `.well-known` file on a DNS-resolvable domain. Every workaround loops back to the same centralized infrastructure.

## Enter Titan

Titan is a native desktop browser that resolves `nsite://` URLs. When you type:

```
nsite://titan
```

The browser looks up "titan" in a name index, gets the associated Nostr public key, fetches the site manifest from relays, downloads the content from Blossom servers, and renders it — all without touching DNS, a certificate authority, or a traditional hosting provider.

The name "titan" is registered directly on the Bitcoin blockchain using an OP_RETURN transaction. It costs about $0.10. One confirmation. Permanent.

## How It Works

Every name registration is a single Bitcoin transaction with an OP_RETURN output:

```
NSIT | version | action | name | nostr pubkey
```

The NSIT prefix identifies it as a name operation. The action is either "register" or "transfer." The name is the human-readable label. The pubkey is the Nostr identity it points to.

**First-in-chain wins.** The first valid registration for a name claims it. Duplicates are ignored. The blockchain is the arbiter.

**Transferable.** Names are controlled by a Bitcoin UTXO. Whoever can spend that output controls the name — update the pubkey, or hand off ownership to someone else. No intermediary needed.

**Permanent.** No renewal. No expiration. No dispute process. A registered name exists as long as Bitcoin exists.

## How Names Are Different

Names are a finite resource. There are 36 single-character names, about 1,300 two-character combinations. Unlike DNS, they don't expire. Once registered, a name doesn't come back.

Names can be transferred, sold, or updated — whoever controls the ownership UTXO controls the name. But the total supply is fixed at the moment of registration. This is a property of the protocol — it's how permanent, first-come-first-served systems work.

## The Security Model

Every step from name to rendered page is cryptographically verified:

1. **Name → pubkey**: Secured by Bitcoin proof-of-work.
2. **Pubkey → manifest**: Nostr events are signed. Relays can't forge them.
3. **Manifest → content**: Every file is addressed by its SHA256 hash. Blossom servers can't tamper with it.

No certificates. No MITM. The entire chain is verified end-to-end.

## What Titan Is

Titan is a native desktop app built in Rust that understands the `nsite://` protocol. It connects directly to Nostr relays and Blossom servers. It doesn't need Bitcoin Core — name lookups happen through Nostr events published by an indexer service that watches the blockchain.

You can also register names without the browser. Visit `nsite://titan` and it generates the OP_RETURN hex for you. Paste it into Sparrow, Electrum, or any wallet that supports OP_RETURN outputs. The address you send from becomes the owner.

For now, it's minimal: an address bar and a webview. You type a name, you see a site. What matters is the protocol underneath — `nsite://` is an open scheme. Other browsers can implement it. Other indexers can publish the same name events. Titan is the reference client, not a walled garden.

## The Stack

- **Names**: Bitcoin OP_RETURN (NSIT prefix, x-only Schnorr pubkeys)
- **Name index**: Nostr events (kind 35129/15129 — no Bitcoin Core needed)
- **Sites**: NIP-5A v2 (kind 15128/35128 manifests, path→SHA256 mappings)
- **Storage**: Blossom servers (content-addressed blob hosting)
- **Discovery**: Nostr relays
- **Client**: Rust + Tauri (system webview, cross-platform)
- **Registration**: Client-side OP_RETURN generator

Everything is open source: [github.com/btcjt/titan](https://github.com/btcjt/titan)

---

_Titan is named after Titan, the largest moon of Saturn — shrouded in a dense amber atmosphere, hiding an entire world beneath._
