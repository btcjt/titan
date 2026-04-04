# Titan: Solving the Last Problem of the Decentralized Web

**I'm giving everybody a 15-minute head start. Starting from the timestamp on this post. I've registered two names for testing: titan and westernbtc. Every other name is untouched. After that, it's open season — for me and everyone else.**

Read on to understand what that means.

---

Nostr solved identity. Blossom solved storage. NIP-5A solved hosting. But there's been one glaring problem left: **nobody can remember a public key.**

```
npub1qe3e2054qkxsyt0yzem0xxv5gdpgmstahaqma3ja6pv2n9auqelqh2q4jf
```

Go ahead. Try to type that from memory. Try to read it to someone over the phone. Maybe you could put a QR code on a poster.

For most of these, you can't. And that's been the bottleneck for the entire Nostr web. You can build a site, host it on Blossom, serve it through relays — but the only way to reach it is through a 63-character string of gibberish, or through a gateway that quietly reintroduces the same DNS dependency you were trying to escape.

## The Domain Name Problem

The traditional web solved this in 1985 with the Domain Name System. Instead of remembering `142.250.80.14`, you type `google.com`. It works. But DNS is controlled by ICANN, managed by registrars, and runs on a system where governments can seize your domain, corporations can outbid you on renewals, and your name expires the moment you miss a payment. You don't own your domain — you rent it.

Current nsite solutions haven't actually fixed this. Gateways serve sites at addresses like `npub1qe3e...gateway.com` — which is still unreadable and still depends on the gateway's DNS entry existing. NIP-05 verification maps usernames to pubkeys, but it requires hosting a `.well-known` file on a DNS-resolvable domain. Every workaround loops back to the same centralized infrastructure.

The Nostr web needed its own name system. One that's truly decentralized. One where you actually own your name.

## Enter Titan

Titan is a native desktop browser that resolves `nsite://` URLs. When you type:

```
nsite://titan
```

The Titan browser looks up "titan" in a name index, gets the associated Nostr public key, fetches the site manifest from relays, downloads the content from Blossom servers, and renders it — all without ever touching DNS, a certificate authority, or a traditional hosting provider.

The name "titan" is registered directly on the Bitcoin blockchain using an OP_RETURN transaction. It costs about $0.10. It takes one confirmation. And it lasts forever.

## How the Name Protocol Works

Every name registration is a single Bitcoin transaction containing an OP_RETURN output with a simple binary payload:

```
NSIT | version | action | name | nostr pubkey
```

That's it. The protocol prefix identifies it as a Titan name operation. The action is either "register" (claim a new name) or "transfer" (hand it to someone else). The name is the human-readable label. The pubkey is the Nostr identity it points to.

**First-in-chain wins.** If you're the first person to broadcast a valid registration for "bitcoin", it's yours. If someone else tries to register it in a later block — or even later in the same block — it's ignored. The blockchain is the only arbiter.

**Names are transferable.** The owner of a name (whoever controls the Bitcoin address that funded the registration) can transfer it to a new Nostr pubkey by creating a new transaction. This creates a chain of ownership rooted in the original registration. No escrow, no intermediary — just a Bitcoin transaction.

**Names are permanent.** There is no renewal. There is no expiration. There is no dispute process. Once a name is registered, it exists as long as Bitcoin exists.

## The Security Chain

Every step from name to rendered page is cryptographically verified:

1. **Name → pubkey**: Secured by Bitcoin proof-of-work. Reversing a registration means a 51% attack.
2. **Pubkey → manifest**: Nostr events are signed by the site owner's keypair. Relays can't forge them.
3. **Manifest → content**: Every file is addressed by its SHA256 hash. Blossom servers can't tamper with it.

No certificates needed. No MITM possible. The entire chain is trustless.

## The Land Rush

Here's the thing about permanent, first-come-first-served names: **there will never be more of them.**

There are 36 possible single-character names. About 1,300 two-character combinations. Common words — `bitcoin`, `wallet`, `news`, `shop`, `music`, `pay`, `mail`, `search` — can each be claimed exactly once.

This isn't like DNS where you can wait for a domain to expire and grab it. There is no expiration. The person who registers `btc` today will own it in 2030, 2050, and 2100 — unless they voluntarily transfer it.

The transfer mechanism means names can be bought and sold. A marketplace will emerge naturally. But the supply is fixed at registration time. Every name that gets claimed is one fewer name available to everyone else, forever.

Two names have been registered for testing: `titan` and `westernbtc`. That's it. No insider allocation. No reserved list. Everything else is unclaimed.

That's the 15 minutes. I'm giving you a head start on myself. After that, I'm claiming names too — because I'd be an idiot not to.

As Satoshi put it:

> _"It might make sense just to get some in case it catches on."_

## What Titan Is (and Isn't)

Titan is a browser — specifically, a native desktop app built in Rust that understands the `nsite://` protocol. It's not a website. You download it and run it. It connects directly to Nostr relays and Blossom servers. It doesn't need Bitcoin Core to work — name lookups happen through Nostr events published by an indexer service that watches the blockchain.

You can also register names without Titan. Visit `nsite://titan` in the browser and it generates the OP_RETURN hex for you. Paste it into Sparrow, Electrum, or any wallet that supports custom OP_RETURN outputs. The address you send from becomes the owner.

For MVP, it's minimal: an address bar and a webview. No tabs, no extensions, no bookmarks. You type a name, you see a site. That's the product.

What matters isn't the chrome around the browser. What matters is the protocol underneath. `nsite://` is an open scheme. Other browsers can implement it. Other indexers can scan the same OP_RETURNs and publish the same name events. Titan is the first client, not the only one.

## The Stack

For the technically curious:

- **Names**: Bitcoin OP_RETURN (NSIT prefix, x-only Schnorr pubkeys)
- **Name index**: Nostr events (kind 35129/15129 — no Bitcoin Core needed for lookups)
- **Sites**: NIP-5A v2 (kind 15128/35128 manifests, path→SHA256 mappings)
- **Storage**: Blossom servers (content-addressed blob hosting)
- **Discovery**: Nostr relays (race-then-linger search for fast results)
- **Client**: Rust + Tauri (system webview, cross-platform)
- **Registration**: Client-side OP_RETURN generator (paste into any Bitcoin wallet)

Everything is open source: [github.com/btcjt/titan](https://github.com/btcjt/titan)

## What's Next

The browser works. The protocol is live. You can register names now.

Your 15 minutes started at the top of this post. Clock's ticking.

---

_Titan is named after Titan, the largest moon of Saturn — shrouded in a dense amber atmosphere, hiding an entire world beneath._
