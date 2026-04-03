# Titan Name Protocol (TNP)

Technical specification for the Bitcoin OP_RETURN name registration protocol.

## Wire Format

```
Offset  Size  Field       Value/Range          Description
------  ----  -----       -----------          -----------
0       4     magic       0x5449544E           ASCII "TITN"
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

**Ownership**: The "owner" of a name is the entity that controls the address used as the first input of the registration transaction. This address is stored by the indexer for transfer verification.

### Transfer (0x01)

Updates the pubkey associated with a name.

**Requirements**:
1. The name must already be registered
2. The first input of the transfer transaction must spend from the current owner address
3. The pubkey field contains the new Nostr pubkey to associate with the name

**After transfer**: The owner address updates to the first input address of the transfer transaction.

## Indexer Behavior

### Block Processing

For each block, iterate transactions in order. For each transaction, check all outputs for OP_RETURN scripts. Attempt to decode using the TITN magic prefix.

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
OP_RETURN: 5449544e 01 00 0a 776573746572 6e627463 <32-byte pubkey>
           TITN     v1 reg 10 w e s t e r n b t c
```

Total: 49 bytes (39 overhead + 10 name bytes)

### Transferring "westernbtc" to a new pubkey

Same format with action byte `0x01`. The transaction's first input must be from the current owner.

## Security Considerations

- **Finality**: Wait for 6 confirmations before considering a registration final
- **Dust attacks**: Registration requires a standard Bitcoin transaction (~$0.10), making mass-squatting expensive
- **Frontrunning**: A miner could theoretically see a registration in the mempool and register the name themselves. Mitigated by the low value of individual names and the reputational cost.
