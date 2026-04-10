# FIPS Network — Integration Analysis

**Researched:** 2026-04-10
**FIPS version evaluated:** v0.2.0
**Upstream:** https://github.com/jmcorgan/fips
**Verdict:** Do not integrate now. Watch and wait.

This document is a cached analysis of whether Titan should integrate with
the FIPS network protocol. Do not treat it as permanent truth — re-read
the FIPS repo before acting on any of this, because the project is
explicitly unstable and the conclusions below depend on its v0.2.0 state.

## One-sentence summary

FIPS is a mesh routing protocol (replacing IP/BGP/DNS), not a content
protocol (replacing Nostr+Blossom). It sits *underneath* Titan rather
than *inside* it, and today it buys Titan nothing user-visible.

## What FIPS is

Self-organizing, encrypted mesh network where **Nostr pubkeys (npubs)
are the node identities**. IPv6 addresses are derived as
`fd` + `SHA-256(pubkey)[0..16]`, giving a deterministic one-way mapping
from any npub to an `fd00::/8` ULA address.

### Four-layer stack

1. **Transport drivers** — UDP (primary), TCP (stream-framed), raw
   Ethernet/WiFi AF_PACKET, Tor onion circuits, Bluetooth L2CAP + BLE
   L2CAP CoC, planned serial/radio. **No WebSocket, no QUIC, no WebRTC,
   no libp2p, no Windows support.**
   [fips-transport-layer.md](https://github.com/jmcorgan/fips/blob/master/docs/design/fips-transport-layer.md)
2. **FMP (FIPS Mesh Protocol)** — hop-by-hop authenticated/encrypted
   links via Noise IK, spanning-tree construction with greedy
   coordinate routing, bloom-filter-guided discovery, forwarding.
   [fips-mesh-layer.md](https://github.com/jmcorgan/fips/blob/master/docs/design/fips-mesh-layer.md)
3. **FSP (FIPS Session Protocol)** — end-to-end encrypted datagram
   sessions between any two nodes via Noise XK, ChaCha20-Poly1305 AEAD,
   periodic rekey with hitless cutover.
   [fips-session-layer.md](https://github.com/jmcorgan/fips/blob/master/docs/design/fips-session-layer.md)
4. **IPv6 adapter** — TUN device that tunnels unmodified IPv6 apps over
   the mesh, with a `.fips` DNS resolver on port 5354.
   [fips-ipv6-adapter.md](https://github.com/jmcorgan/fips/blob/master/docs/design/fips-ipv6-adapter.md)

No DHT, no gossip, no content addressing. Routing is topology-driven.
Human-friendly names come from a static `/etc/fips/hosts` file or by
shoving the full npub into a `npub1....fips` DNS name.

### Cryptography

- Noise Protocol Framework throughout
- Noise IK for hop-by-hop link encryption
- Noise XK for end-to-end session keys
- ChaCha20-Poly1305 AEAD
- secp256k1/Schnorr signatures on tree/lookup messages
- Same primitives as nostr-sdk, so there's no conceptual mismatch

### State of the project (as of 2026-04-10)

- v0.2.0, explicitly unstable
- "Small live mesh" of deployed nodes
- Debian/macOS/OpenWrt/AUR packaging exists (no Windows)
- Runs as a systemd/launchd daemon: `fips`, `fipsctl`, `fipstop`
- Rust 1.85+/edition 2024
- **Not published on crates.io** — listed under longer-term roadmap
- Security audit pending (on the roadmap)

## Why it's philosophically interesting

The single most elegant thing about FIPS from Titan's perspective is
that **Nostr npubs are the node identities**. The same primitive Titan
uses for nsite authors and indexer identities is what FIPS uses for
routing. A future world where Titan's indexer publishes a FIPS
reachability hint alongside each kind 35129 name record isn't hard to
imagine, and would close a nice loop.

But the philosophical alignment is a trap. The two systems solve
different problems at different layers.

## Why it doesn't fit today

FIPS is **layer 3** (datagrams between node addresses). Titan is
**layer 7** (fetching HTML+JS blobs from Blossom over HTTPS).

Swapping "UDP to relay.westernbtc.com" for "UDP through a FIPS mesh to
some other relay" gives the *exact same* user experience unless someone
has deployed Nostr relays and Blossom servers as FIPS-addressable
nodes. Today, nobody has. FIPS does not store or serve content — it
only moves datagrams. A Titan-over-FIPS world still needs:

1. A Nostr relay reachable at a `fd00::/8` address
2. A Blossom server reachable at a `fd00::/8` address
3. Titan's resolver routing requests through a `fips0` TUN device

Without all three, the integration is a no-op.

### Concrete unique value (if the ecosystem existed)

1. **Transport diversity / offline mesh.** BLE / WiFi / Tor / serial
   hops mean a Titan on a FIPS mesh could reach another nsite even
   when both nodes are offline from the clearnet. Cool, but needs a
   relay+Blossom deployment story that doesn't exist.
2. **Metadata privacy vs. infrastructure operators.** Noise XK means
   neither relays nor Blossom servers see the content. Titan currently
   trusts them to serve bytes matching a hash — integrity is fine, but
   operators can log what's being fetched.
3. **Censorship resistance at the transport level.** If westernbtc.com
   got blocked, a FIPS mesh keeps paths alive.

None of these have measurable user demand for a browser today.

## Blockers

- **No Windows support** in any transport. Titan ships Windows MSIs;
  any FIPS-dependent feature would be Linux/macOS only.
- **TUN + raw sockets need elevated privileges** (`CAP_NET_ADMIN` on
  Linux). A browser can't grab those. Any integration means the user
  runs a separate `fips` daemon.
- **No published crate.** Embedding means a git dep against a moving
  v0.x API with no stability guarantees.
- **No browser-reachable transport.** No WebSocket, no QUIC, no WebRTC.
  Titan's system webviews can't speak FIPS directly.
- **No content layer.** Even with FIPS, Titan still needs Nostr relays
  and Blossom servers reachable from within the mesh.
- **Name collision with NIST FIPS** (Federal Information Processing
  Standards 140-x crypto). A `fips://` URL scheme would be a UX and
  SEO mess.
- **Security audit pending.** Listed on the roadmap but not done.
- **Ethos mismatch.** CLAUDE.md says Titan is a "pure Nostr + Blossom
  client — no local database, no Bitcoin Core, no IPFS, no libp2p."
  Embedding a mesh router with spanning trees, bloom filters, and a
  TUN device is a ~10x expansion of moving parts.

## Integration scopes (if we ever did it)

| Scope | What it is | Cost | Value |
|-------|------------|------|-------|
| **Smallest** | `fips://npub...` URL scheme as a metadata hint. Titan surfaces a "reachable via FIPS" badge if the user runs a `fips` daemon. | ~1 day | Low — informational only |
| **Medium** | Titan's resolver dials Nostr relays and Blossom servers at `fd00::/8` addresses if the host has `fips0`. No new Titan code — `reqwest` / `tungstenite` route through the existing TUN. | A few days, mostly docs + tests | Medium IF someone deploys FIPS-addressable relays |
| **Largest** | New `crates/titan-fips` embedding an in-process FSP node. | Multi-month, multi-platform pain, stability risk, platform exclusion | Speculative |

**If we ever integrate, start with the smallest scope.** Do not embed a
FIPS node in Titan itself — the elevated-privileges requirement forces
the user to run a system daemon anyway, at which point the medium-scope
"use the daemon" path is strictly better than embedding.

## Recommendation

**Watch and wait.** Re-evaluate when any of these milestones ship
upstream:

1. **"Peer discovery via Nostr relays"** (on the FIPS near-term
   roadmap). This is the interesting cross-pollination point. If FIPS
   starts publishing Nostr events for peer discovery, Titan's indexer
   could co-publish FIPS reachability alongside kind 35129 name
   records without any direct integration.
2. **FIPS publishes a stable crate on crates.io.**
3. **A public Nostr relay and Blossom server are deployed as
   FIPS-addressable nodes** by someone other than jmcorgan. Without
   this, the medium-scope integration has no targets to reach.
4. **Windows support lands**, or Titan drops Windows.
5. **A browser-friendly transport** (WebSocket wrapping, WebRTC bridge)
   gets added to FIPS.

Until then: **no code changes in Titan.**

## Questions worth asking jmcorgan (if we go deeper)

1. Is there a planned WebSocket or other browser-reachable transport?
   Without one, integration is blocked on running a separate daemon.
2. When "peer discovery via Nostr relays" lands, will it define new
   Nostr event kinds or reuse existing ones? Titan's indexer could
   publish FIPS reachability alongside kind 35129 if the formats are
   stable.
3. Would a thin client-only FSP library (no TUN, no `CAP_NET_ADMIN`)
   make sense for embedding in GUI apps? That would unlock the
   "embed without root" scope.
4. Has the name collision with NIST FIPS come up in project
   discussions? A `fips://` URL scheme has a rough road ahead.

## Bottom line

FIPS is a well-designed layer-3 mesh router from someone who clearly
knows what they're doing. Titan is a layer-7 content browser. They are
**orthogonal, not complementary**. The alignment on "npubs as
identities" is philosophically neat but doesn't translate to a
concrete integration value prop in 2026.

Revisit this document after ~3-6 months, specifically when the "peer
discovery via Nostr relays" milestone lands upstream. That is the one
checkpoint that could change the calculus.
