# Titan Name Protocol (TNP)

Technical specification for the Bitcoin OP_RETURN name registration protocol.

## Wire Format

```
Offset  Size  Field       Value/Range          Description
------  ----  -----       -----------          -----------
0       4     magic       0x4E534954           ASCII "NSIT"
4       1     version     0x01                 Protocol version
5       1     action      0x00 | 0x01          Register or Transfer
6       1     name_len    0x01-0x29 (1-41)     Name length in bytes
7       N     name        [a-z0-9-]{1,41}      The name
7+N     32    pubkey      <32 bytes>           x-only Schnorr pubkey
```

Total: 39 + N bytes. Maximum 80 bytes (Bitcoin OP_RETURN limit).

## Name Validation

| Rule | Constraint |
|------|-----------|
| Character set | `[a-z0-9-]` |
| Case | Lowercase only (uppercase normalized) |
| Min length | 1 |
| Max length | 41 |
| Leading hyphen | Not allowed |
| Trailing hyphen | Not allowed |
| Consecutive hyphens | Not allowed |

## Actions

### Register (0x00)

Creates a name→pubkey mapping. If the name already exists on-chain, the transaction is silently ignored.

**Ownership UTXO**: The first non-OP_RETURN output of the registration transaction becomes the ownership UTXO. Whoever can spend this UTXO controls the name.

**Name = site identity**: The registered name is also the site identifier on Nostr. When registering `westernbtc`, the publisher creates a kind 35128 (addressable) manifest event with `d=westernbtc`. One name, one site. Multiple sites require multiple name registrations (which can point to the same pubkey).

### Transfer (0x01)

Updates the Nostr pubkey and/or transfers ownership of a name.

**Requirements**:
1. The name must already be registered
2. The transaction must **spend the current ownership UTXO** as one of its inputs
3. The pubkey field contains the new Nostr pubkey to associate with the name

**After transfer**: The first non-OP_RETURN output of the transfer transaction becomes the new ownership UTXO. This enables full ownership transfer — send the output to a new address to hand off control.

## Indexer Behavior

### Block Processing

For each block, iterate transactions in order. For each transaction, check all outputs for OP_RETURN scripts. Attempt to decode using the NSIT magic prefix.

### Conflict Resolution

If the same name appears multiple times in the same block:
- The transaction with the lower index wins (first-seen within the block)
- Subsequent registrations for the same name are ignored

### Chain Reorganizations

On detecting a reorg (previous block hash mismatch):
1. Roll back the affected block(s)
2. Delete any name registrations from those blocks
3. Re-process the new chain tip

For MVP, handle depth-1 reorgs. Deeper reorgs require scanning back to the fork point.

## Examples

### Registering "westernbtc"

```
OP_RETURN: 4e534954 01 00 0a 776573746572 6e627463 <32-byte pubkey>
           NSIT     v1 reg 10 w e s t e r n b t c
```

Total: 49 bytes (39 overhead + 10 name bytes)

### Transferring "westernbtc" to a new pubkey

Same format with action byte `0x01`. The transaction's first input must be from the current owner.

## Nostr Name Index

An indexer service watches Bitcoin blocks for NSIT OP_RETURNs and publishes the name index as Nostr events. This allows any client to query name records without running a Bitcoin full node.

### Name Record — Kind 35129 (addressable, d=name)

```json
{
  "kind": 35129,
  "pubkey": "<indexer-service-pubkey>",
  "tags": [
    ["d", "westernbtc"],
    ["p", "<registered-nostr-pubkey-hex>"],
    ["owner_txid", "abc123..."],
    ["owner_vout", "0"],
    ["txid", "abc123..."],
    ["block", "943619"],
    ["action", "register"],
    ["reg_txid", "abc123..."],
    ["reg_block", "943619"]
  ],
  "content": ""
}
```

Addressable by d-tag = name. Transfers replace the previous record automatically. Includes `reg_txid`/`reg_block` (original registration, carried forward) and optionally `prev_pubkey`/`prev_txid` on transfers.

### Name History — Kind 1129 (regular, non-replaceable)

```json
{
  "kind": 1129,
  "pubkey": "<indexer-service-pubkey>",
  "tags": [
    ["d", "westernbtc"],
    ["p", "<nostr-pubkey-hex>"],
    ["owner_txid", "def456..."],
    ["owner_vout", "0"],
    ["txid", "def456..."],
    ["block", "944100"],
    ["action", "transfer"],
    ["prev_pubkey", "<old-nostr-pubkey-hex>"],
    ["prev_txid", "abc123..."]
  ],
  "content": ""
}
```

One event per action (register or transfer). These are never replaced — they accumulate to form the complete chain of custody. Queryable by `#d` tag to get the full history of a name.

### Index Stats — Kind 15129 (replaceable)

```json
{
  "kind": 15129,
  "pubkey": "<indexer-service-pubkey>",
  "tags": [
    ["block", "943621"],
    ["hash", "00000000..."],
    ["names", "2"]
  ],
  "content": ""
}
```

One per indexer pubkey. Updated after each block sync.

### Query Pattern

Clients use a race-then-linger strategy:
1. Subscribe to the filter across all relays
2. On first event received, start a 200ms linger timer
3. Collect any additional events within the window
4. Return the newest event by `created_at`

Name lookup filter: `{kinds: [35129], authors: [indexerPubkey], "#d": ["name"]}`
Name history filter: `{kinds: [1129], authors: [indexerPubkey], "#d": ["name"]}`
Stats filter: `{kinds: [15129], authors: [indexerPubkey]}`

### Verification

The Nostr index is a convenience layer — it is not the source of truth. Any node scanning the same blockchain will arrive at the same name→pubkey mappings. Clients that run Bitcoin Core can verify the index against their own chain. The indexer's events are signed by its Nostr keypair, providing attribution but not consensus.

## Ownership Model

Ownership of a name is tied to a specific **UTXO** (unspent transaction output), not an address. When you register a name, the first non-OP_RETURN output of the registration transaction becomes the **ownership UTXO**. Whoever can spend that UTXO controls the name.

### How transfer works

To transfer a name (change the pubkey or hand off ownership), you must:

1. **Spend the ownership UTXO** as one of the inputs to the transfer transaction
2. Include the NSIT transfer OP_RETURN with the new pubkey
3. The first non-OP_RETURN output of the transfer transaction becomes the **new ownership UTXO**

This means:
- **Full ownership transfer** is built in — send the ownership UTXO to someone else's address in the transfer transaction, and they now control the name
- **Pubkey updates** work the same way — spend the UTXO back to yourself with a new pubkey in the OP_RETURN
- **Sales** are natural — the ownership UTXO can be spent to a buyer's address as part of the transfer
- No private key sharing needed

### What happens if the ownership UTXO is spent without a transfer?

If the ownership UTXO is spent in a regular Bitcoin transaction (no NSIT OP_RETURN), the name becomes **permanently frozen**. It still resolves to the current pubkey — the site works — but nobody can ever transfer it again, because the required UTXO no longer exists.

**This is irreversible.** There is no recovery mechanism.

### Best practices

- **Use coin control.** In Sparrow or similar wallets, freeze the ownership UTXO so you don't accidentally spend it.
- **Label the UTXO** in your wallet so you know it controls a name.
- **Don't mix name UTXOs with spending funds.** Keep them in a separate wallet or at least clearly labeled.
- When transferring, make sure the NSIT OP_RETURN is in the same transaction that spends the ownership UTXO.

## Security Considerations

- **Finality**: Wait for 6 confirmations before considering a registration final
- **Dust attacks**: Registration requires a standard Bitcoin transaction (~$0.10), making mass-squatting expensive
- **Frontrunning**: A miner could theoretically see a registration in the mempool and register the name themselves. Mitigated by the low value of individual names and the reputational cost.
