# Titan Bookmarks Protocol

Titan Bookmarks is a Nostr event kind published by the Titan browser
to sync a user's bookmark list across devices. It is orthogonal to the
NSIT name protocol — they just happen to share the same
publisher (the built-in Titan signer) and the same `xx129` kind
convention.

## Goals

- **Sync across devices.** Any Titan install holding the user's nsec
  pulls the same bookmark list on startup.
- **Private by default.** Relays see opaque ciphertext. Only the
  holder of the nsec can read the URLs or titles.
- **Offline-first.** The browser always writes a local mirror so the
  bookmarks bar paints instantly at startup, before any relay
  round-trip. Edits made while offline queue for the next publish.
- **Forward compatible.** Unknown row types in the encrypted payload
  are skipped by older readers, so new row kinds (folders, separators,
  annotations) can be introduced without bumping the event kind.

## Non-goals

- Interop with NIP-51 kind 10003 (generic bookmarks) or NIP-B0 kind
  39701 (web bookmarks). Neither fits Titan's `nsite://` URL shape.
  See "Why not NIP-51" below.
- Public/shared bookmark lists. v1 is single-user, self-encrypted.
  Future row types or a separate kind could add "shared with another
  pubkey" semantics.

## Event shape

Titan Bookmarks uses **kind 10129**, a **replaceable** event (one per
author, no `d`-tag). New publishes overwrite the previous list.

```json
{
  "kind": 10129,
  "pubkey": "bec1a370130fed4fb9f78f9efc725b35104d827470e75573558a87a9ac5cde44",
  "created_at": 1775829991,
  "tags": [],
  "content": "<NIP-44 v2 ciphertext encrypted to self>",
  "id": "<32-byte hex>",
  "sig": "<64-byte hex>"
}
```

### Tags array

**Always empty.** Every bookmark entry lives inside the encrypted
`.content`. Relays cannot filter on URL, title, or any bookmark
metadata — they only see author, kind, and created_at.

### Content

The plaintext that gets NIP-44 v2 encrypted to the user's own pubkey is
a JSON array of row arrays:

```json
[
  ["bookmark", "nsite://titan", "Titan", 1775829991],
  ["bookmark", "nsite://westernbtc", "Western BTC", 1775829992]
]
```

Row structure:

| Index | Type | Meaning |
|-------|------|---------|
| 0 | string | Row type discriminator. See "Row types" below. |
| 1 | string | Full URL including scheme (e.g. `nsite://titan`). |
| 2 | string | User-assigned title. |
| 3 | integer | Unix seconds when the bookmark was added. |

Decoders MUST skip any row whose first element is not a type they
recognize. Decoders SHOULD be lenient about type coercion on the
timestamp field (accepting both integer and numeric-string forms)
to protect against future encoder changes.

### Row types

Only `"bookmark"` is defined in v1. Reserved for future use:

- `"folder"` — a named collection; children would be referenced by
  bookmark ID or indexed by position
- `"separator"` — a visual divider in the bookmarks bar
- `"group"` — a recursive grouping mechanism for nested folders
- `"note"` — annotation attached to a preceding bookmark

All of these are future work and are NOT defined by this document.
Titan will ignore them on read until a future version of this spec
formalizes them.

## Encryption

The plaintext JSON array is encrypted with
[NIP-44](https://github.com/nostr-protocol/nips/blob/master/44.md)
version 2, using the user's own secret key as both the sender and the
recipient (i.e. self-encryption via the ECDH shared secret between
`pubkey` and `pubkey`).

This is the same convention used by nostrudel and Amethyst for
NIP-51 private items — the plaintext is formally "a message from you
to yourself."

## URL normalization

On the wire (inside the encrypted payload), URLs always include the
`nsite://` scheme:

- `nsite://titan` ✓
- `nsite://westernbtc/some/path` ✓
- `titan` ✗ (decoders should accept this for defensive decoding, but
  encoders MUST write the full scheme)

Titan's internal in-memory state stores URLs scheme-stripped
(`"titan"` instead of `"nsite://titan"`) to match how the address bar
and navigate handler work. The `bookmarks.rs` encoder normalizes at
serialization time; the decoder strips on read. See
`normalize_url()` / `denormalize_url()` in the implementation.

## Publish triggers

Titan publishes a fresh kind 10129 event in these cases:

1. **Add** — user bookmarks a new URL.
2. **Remove** — user deletes a bookmark.
3. **Rename** — user changes a bookmark's title.
4. **Migration** — first launch after upgrading from a pre-10129 version
   with a legacy `bookmarks.json` on disk.
5. **Pending flush** — on startup, if the local mirror has
   `pending_publish: true` (set when an edit happened while offline or
   the signer was locked).

Every publish is best-effort. If the relay push fails, the edit still
takes effect locally and the store is marked `pending_publish: true`
for the next online attempt.

## Fetch triggers

Titan fetches the kind 10129 event on startup in these cases:

1. **Empty local state** — no `bookmarks.json` on disk. Pull the
   remote list as the initial seed.
2. **In sync at last shutdown** — merge any updates from another
   device.

If a pending flush is queued (case 5 above), Titan publishes instead
of fetching — the local state is the newer source of truth.

## Local cache

Titan mirrors the plaintext bookmark list to
`<data_dir>/bookmarks.json` in a versioned wrapper format:

```json
{
  "version": 1,
  "bookmarks": [
    {"url": "titan", "title": "Titan", "created_at": 1775829991}
  ],
  "pending_publish": false
}
```

- `version: 1` — schema version of the wrapper. Older binaries that
  only know version 1 will error out parsing future versions and fall
  back to an empty list (the next publish from a newer binary
  overwrites).
- `bookmarks` — the current list, stored scheme-stripped to match
  in-memory state.
- `pending_publish` — whether the in-memory list has unpublished
  changes. Set to `true` on every mutation; cleared on successful
  publish.

## Why not NIP-51 kind 10003

[NIP-51](https://github.com/nostr-protocol/nips/blob/master/51.md) kind
10003 is defined as a list of `"e"` tags (kind:1 notes) and `"a"`
tags (kind:30023 long-form articles). URL references are not part of
the spec. Titan's bookmarks are URL references to a novel protocol
(`nsite://`), which doesn't fit the "event reference" shape.

Titan could shoehorn URL rows into kind 10003 via an `"r"` tag inside
the encrypted private items — `"r"` is loosely defined across NIPs
24/38/52/71/75/84 as "a web URL the event is referring to." Two
problems with this:

1. **Semantic drift.** A future generic NIP-51 reader that understands
   `"e"`/`"a"` would encounter Titan's `"r"` rows and either render
   them confusingly or skip them. Titan in turn would have to skip
   other clients' `"e"`/`"a"` entries that showed up in the same kind.
2. **No filter precision.** Titan cannot cleanly ask relays for "my
   nsite bookmarks" — it would always pull every client's kind 10003
   for the user and then filter client-side.

A dedicated kind gives us:
- A clear wire contract (no ambiguity, no cross-client bleed)
- One-shot relay filters
- Room to extend row types without fighting the spec

The cost is that clients which don't know about kind 10129 will
ignore Titan bookmarks entirely. That's acceptable — Titan bookmarks
are for Titan users.

## Why not NIP-B0 kind 39701

[NIP-B0](https://github.com/nostr-protocol/nips/blob/master/B0.md)
defines kind 39701 for "web bookmarks" with the URL as a `d`-tag with
the scheme stripped. Two problems:

1. **Scheme stripping breaks `nsite://`.** NIP-B0 assumes http/https
   and strips the scheme before computing the `d`-tag. That makes
   `nsite://titan` and `https://titan` collide into the same event,
   which is incorrect.
2. **No privacy story.** NIP-B0 is public-only; it has no encrypted
   private variant. Titan defaults to private.

## Kind number selection

Kind 10129 was chosen for three reasons:

1. **Range.** The 10000–19999 range is for replaceable events (one per
   author, no `d`-tag), which matches the "one bookmark list per user"
   model.
2. **Titan convention.** Titan's existing NSIT kinds end in `129`:
   35129 (name state), 15129 (index stats), 1129 (history log). Kind
   10129 extends this convention — `kind % 1000 == 129` marks an
   event as "Titan-defined."
3. **Unused.** A `nak req -k 10129` against relay.damus.io,
   relay.primal.net, relay.westernbtc.com, nos.lol, and
   purplerelay.com returned zero events at the time this kind was
   minted. No collision risk with existing clients.

## Reference implementation

See `crates/titan-app/src/bookmarks.rs` for the Titan implementation.
Tests covering the wire format, row-type tolerance, URL normalization,
and end-to-end encrypt/decrypt round trips live in the same file.
