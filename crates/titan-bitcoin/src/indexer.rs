//! Block scanner and indexer for the Titan name protocol.
//!
//! Scans Bitcoin blocks for NSIT OP_RETURN outputs, processes registrations
//! and transfers, and stores them in the SQLite name index.

use crate::codec;
use crate::rpc::{BitcoinRpc, Block, RpcError, Transaction};
use crate::store::{NameStore, StoreError};
use titan_types::OpAction;
use tracing::{debug, info, warn};

/// Errors from the indexer.
#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("rpc: {0}")]
    Rpc(#[from] RpcError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("node is in initial block download")]
    InitialBlockDownload,
    #[error("not on mainnet (chain: {0})")]
    WrongChain(String),
}

/// Result of processing a single block.
#[derive(Debug, Default)]
pub struct BlockResult {
    pub height: u64,
    pub hash: String,
    pub registrations: u32,
    pub transfers: u32,
    pub skipped: u32,
}

/// Scans Bitcoin blocks and indexes NSIT name operations.
pub struct Indexer {
    rpc: BitcoinRpc,
    store: NameStore,
}

impl Indexer {
    pub fn new(rpc: BitcoinRpc, store: NameStore) -> Self {
        Self { rpc, store }
    }

    /// Check that the node is on mainnet and fully synced.
    pub async fn preflight(&self) -> Result<u64, IndexerError> {
        let info = self.rpc.get_blockchain_info().await?;
        if info.chain != "main" {
            return Err(IndexerError::WrongChain(info.chain));
        }
        if info.initial_block_download {
            return Err(IndexerError::InitialBlockDownload);
        }
        Ok(info.blocks)
    }

    /// Sync from the last stored height up to the current chain tip.
    /// Returns the number of blocks processed.
    pub async fn sync_to_tip(&mut self) -> Result<u64, IndexerError> {
        let mut total_processed = 0u64;

        'outer: loop {
            let tip = self.rpc.get_blockchain_info().await?.blocks;
            let start = match self.store.get_sync_state()? {
                Some(state) => state.block_height + 1,
                None => {
                    info!("no sync state found, starting from tip {tip}");
                    let hash = self.rpc.get_block_hash(tip).await?;
                    self.store.set_sync_state(tip, &hash)?;
                    return Ok(0);
                }
            };

            if start > tip {
                break;
            }

            for height in start..=tip {
                let hash = self.rpc.get_block_hash(height).await?;
                let block = self.rpc.get_block(&hash).await?;

                // Reorg detection: check that this block's parent matches our stored hash
                if let Some(sync) = self.store.get_sync_state()? {
                    if let Some(ref prev) = block.previous_block_hash {
                        if *prev != sync.block_hash {
                            warn!(
                                "reorg detected at height {height}: expected parent {}, got {prev}",
                                sync.block_hash
                            );
                            self.handle_reorg(height - 1).await?;
                            continue 'outer; // restart scan from rolled-back position
                        }
                    }
                }

                let result = self.process_block(&block)?;
                log_block_result(&result);
                self.store.set_sync_state(height, &hash)?;
                total_processed += 1;
            }

            break;
        }

        Ok(total_processed)
    }

    /// Set the starting block height for a fresh index.
    /// Call this before `sync_to_tip()` on first run.
    pub async fn set_start_height(&mut self, height: u64) -> Result<(), IndexerError> {
        let hash = self.rpc.get_block_hash(height).await?;
        self.store.set_sync_state(height, &hash)?;
        info!("indexer start height set to {height}");
        Ok(())
    }

    /// Process a single block: scan all transactions for NSIT OP_RETURNs.
    fn process_block(&self, block: &Block) -> Result<BlockResult, StoreError> {
        let mut result = BlockResult {
            height: block.height,
            hash: block.hash.clone(),
            ..Default::default()
        };

        for tx in &block.tx {
            self.process_transaction(tx, block.height, &mut result)?;
        }

        Ok(result)
    }

    /// Scan a single transaction for NSIT OP_RETURN outputs.
    fn process_transaction(
        &self,
        tx: &Transaction,
        block_height: u64,
        result: &mut BlockResult,
    ) -> Result<(), StoreError> {
        for output in &tx.vout {
            let data = match output.op_return_data() {
                Some(d) => d,
                None => continue,
            };

            let op = match codec::decode(&data) {
                Some(op) => op,
                None => continue,
            };

            match op.action {
                OpAction::Register => {
                    let owner = tx.first_input_address().unwrap_or("unknown");
                    let inserted = self.store.insert_name(
                        &op.name,
                        &op.pubkey,
                        owner,
                        &tx.txid,
                        block_height,
                    )?;
                    if inserted {
                        debug!("registered name '{}' at height {block_height}", op.name);
                        result.registrations += 1;
                    } else {
                        debug!("duplicate registration for '{}', skipped", op.name);
                        result.skipped += 1;
                    }
                }
                OpAction::Transfer => {
                    let sender = match tx.first_input_address() {
                        Some(addr) => addr,
                        None => {
                            debug!("transfer for '{}' has no input address, skipped", op.name);
                            result.skipped += 1;
                            continue;
                        }
                    };

                    // Verify the sender is the current owner
                    let current_owner = self.store.get_owner(&op.name)?;
                    match current_owner {
                        Some(ref owner) if owner == sender => {
                            self.store.transfer_name(
                                &op.name,
                                &op.pubkey,
                                sender,
                                &tx.txid,
                                block_height,
                            )?;
                            debug!("transferred name '{}' at height {block_height}", op.name);
                            result.transfers += 1;
                        }
                        Some(ref owner) => {
                            debug!(
                                "transfer for '{}' rejected: sender {sender} != owner {owner}",
                                op.name
                            );
                            result.skipped += 1;
                        }
                        None => {
                            debug!(
                                "transfer for '{}' rejected: name not registered",
                                op.name
                            );
                            result.skipped += 1;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Handle a depth-1 reorg by rolling back to the given height and re-syncing.
    async fn handle_reorg(&mut self, rollback_to: u64) -> Result<(), IndexerError> {
        let deleted = self.store.rollback_to(rollback_to)?;
        info!("reorg: rolled back to height {rollback_to}, removed {deleted} name(s)");

        let hash = self.rpc.get_block_hash(rollback_to).await?;
        self.store.set_sync_state(rollback_to, &hash)?;
        Ok(())
    }

    /// Access the underlying store (e.g. for name resolution).
    pub fn store(&self) -> &NameStore {
        &self.store
    }
}

fn log_block_result(result: &BlockResult) {
    if result.registrations > 0 || result.transfers > 0 {
        info!(
            "block {} ({}): {} registration(s), {} transfer(s), {} skipped",
            result.height, result.hash, result.registrations, result.transfers, result.skipped
        );
    } else {
        debug!("block {} ({}): no NSIT operations", result.height, result.hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::*;
    use titan_types::TitanName;

    /// Build a mock OP_RETURN output from a TitanOp.
    fn make_op_return_output(op: &titan_types::TitanOp) -> TxOutput {
        let payload = codec::encode(op);
        // Build the script: OP_RETURN (6a) + push_len + payload
        let mut script = vec![0x6a, payload.len() as u8];
        script.extend_from_slice(&payload);
        TxOutput {
            n: 0,
            script_pub_key: ScriptPubKey {
                hex: hex::encode(&script),
                script_type: "nulldata".to_string(),
                address: None,
            },
        }
    }

    /// Build a transaction with a given first-input address and outputs.
    fn make_tx(txid: &str, first_input_addr: Option<&str>, vout: Vec<TxOutput>) -> Transaction {
        let vin = vec![TxInput {
            txid: first_input_addr.map(|_| "prev_tx".to_string()),
            vout: first_input_addr.map(|_| 0),
            prevout: first_input_addr.map(|addr| PrevOut {
                script_pub_key: ScriptPubKey {
                    hex: String::new(),
                    script_type: "witness_v1_taproot".to_string(),
                    address: Some(addr.to_string()),
                },
            }),
        }];
        Transaction {
            txid: txid.to_string(),
            vin,
            vout,
        }
    }

    fn test_pubkey() -> [u8; 32] {
        let mut pk = [0u8; 32];
        pk[0] = 0xab;
        pk[31] = 0xcd;
        pk
    }

    fn other_pubkey() -> [u8; 32] {
        let mut pk = [0u8; 32];
        pk[0] = 0x11;
        pk[31] = 0x22;
        pk
    }

    fn make_register_op(name: &str) -> titan_types::TitanOp {
        titan_types::TitanOp {
            action: OpAction::Register,
            name: TitanName::new(name).unwrap(),
            pubkey: test_pubkey(),
        }
    }

    fn make_transfer_op(name: &str) -> titan_types::TitanOp {
        titan_types::TitanOp {
            action: OpAction::Transfer,
            name: TitanName::new(name).unwrap(),
            pubkey: other_pubkey(),
        }
    }

    #[test]
    fn process_block_registration() {
        let store = NameStore::open_memory().unwrap();
        let rpc = BitcoinRpc::new(RpcConfig {
            url: String::new(),
            user: String::new(),
            password: String::new(),
        });
        let indexer = Indexer::new(rpc, store);

        let op = make_register_op("westernbtc");
        let block = Block {
            hash: "00000000abc".to_string(),
            height: 800_000,
            previous_block_hash: None,
            next_block_hash: None,
            tx: vec![make_tx(
                "tx1",
                Some("bc1qowner"),
                vec![make_op_return_output(&op)],
            )],
        };

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.registrations, 1);
        assert_eq!(result.transfers, 0);
        assert_eq!(result.skipped, 0);

        let record = indexer
            .store()
            .get_name(&TitanName::new("westernbtc").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(record.pubkey, test_pubkey());
        assert_eq!(record.owner_address, "bc1qowner");
    }

    #[test]
    fn process_block_duplicate_registration() {
        let store = NameStore::open_memory().unwrap();
        let rpc = BitcoinRpc::new(RpcConfig {
            url: String::new(),
            user: String::new(),
            password: String::new(),
        });
        let indexer = Indexer::new(rpc, store);

        let op = make_register_op("westernbtc");
        let block = Block {
            hash: "00000000abc".to_string(),
            height: 800_000,
            previous_block_hash: None,
            next_block_hash: None,
            tx: vec![
                make_tx("tx1", Some("bc1qfirst"), vec![make_op_return_output(&op)]),
                make_tx("tx2", Some("bc1qsecond"), vec![make_op_return_output(&op)]),
            ],
        };

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.registrations, 1);
        assert_eq!(result.skipped, 1);

        // First-in-block wins
        let record = indexer
            .store()
            .get_name(&TitanName::new("westernbtc").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(record.owner_address, "bc1qfirst");
    }

    #[test]
    fn process_block_valid_transfer() {
        let store = NameStore::open_memory().unwrap();
        // Pre-register the name
        store
            .insert_name(
                &TitanName::new("westernbtc").unwrap(),
                &test_pubkey(),
                "bc1qowner",
                "tx0",
                799_999,
            )
            .unwrap();

        let rpc = BitcoinRpc::new(RpcConfig {
            url: String::new(),
            user: String::new(),
            password: String::new(),
        });
        let indexer = Indexer::new(rpc, store);

        let op = make_transfer_op("westernbtc");
        let block = Block {
            hash: "00000000def".to_string(),
            height: 800_000,
            previous_block_hash: None,
            next_block_hash: None,
            tx: vec![make_tx(
                "tx1",
                Some("bc1qowner"), // matches current owner
                vec![make_op_return_output(&op)],
            )],
        };

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.transfers, 1);
        assert_eq!(result.skipped, 0);

        let record = indexer
            .store()
            .get_name(&TitanName::new("westernbtc").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(record.pubkey, other_pubkey());
    }

    #[test]
    fn process_block_unauthorized_transfer() {
        let store = NameStore::open_memory().unwrap();
        store
            .insert_name(
                &TitanName::new("westernbtc").unwrap(),
                &test_pubkey(),
                "bc1qowner",
                "tx0",
                799_999,
            )
            .unwrap();

        let rpc = BitcoinRpc::new(RpcConfig {
            url: String::new(),
            user: String::new(),
            password: String::new(),
        });
        let indexer = Indexer::new(rpc, store);

        let op = make_transfer_op("westernbtc");
        let block = Block {
            hash: "00000000def".to_string(),
            height: 800_000,
            previous_block_hash: None,
            next_block_hash: None,
            tx: vec![make_tx(
                "tx1",
                Some("bc1qattacker"), // NOT the owner
                vec![make_op_return_output(&op)],
            )],
        };

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.transfers, 0);
        assert_eq!(result.skipped, 1);

        // Name unchanged
        let record = indexer
            .store()
            .get_name(&TitanName::new("westernbtc").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(record.pubkey, test_pubkey());
    }

    #[test]
    fn process_block_transfer_unregistered() {
        let store = NameStore::open_memory().unwrap();
        let rpc = BitcoinRpc::new(RpcConfig {
            url: String::new(),
            user: String::new(),
            password: String::new(),
        });
        let indexer = Indexer::new(rpc, store);

        let op = make_transfer_op("nonexistent");
        let block = Block {
            hash: "00000000abc".to_string(),
            height: 800_000,
            previous_block_hash: None,
            next_block_hash: None,
            tx: vec![make_tx(
                "tx1",
                Some("bc1qsomeone"),
                vec![make_op_return_output(&op)],
            )],
        };

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.transfers, 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn process_block_ignores_non_titn_outputs() {
        let store = NameStore::open_memory().unwrap();
        let rpc = BitcoinRpc::new(RpcConfig {
            url: String::new(),
            user: String::new(),
            password: String::new(),
        });
        let indexer = Indexer::new(rpc, store);

        let block = Block {
            hash: "00000000abc".to_string(),
            height: 800_000,
            previous_block_hash: None,
            next_block_hash: None,
            tx: vec![Transaction {
                txid: "tx1".to_string(),
                vin: vec![TxInput {
                    txid: None,
                    vout: None,
                    prevout: None,
                }],
                vout: vec![
                    // Normal output (not OP_RETURN)
                    TxOutput {
                        n: 0,
                        script_pub_key: ScriptPubKey {
                            hex: "0014abcdef".to_string(),
                            script_type: "witness_v0_keyhash".to_string(),
                            address: Some("bc1q...".to_string()),
                        },
                    },
                    // OP_RETURN but not NSIT
                    TxOutput {
                        n: 1,
                        script_pub_key: ScriptPubKey {
                            hex: "6a0468656c6c6f".to_string(), // OP_RETURN "hello"
                            script_type: "nulldata".to_string(),
                            address: None,
                        },
                    },
                ],
            }],
        };

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.registrations, 0);
        assert_eq!(result.transfers, 0);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn process_block_mixed_operations() {
        let store = NameStore::open_memory().unwrap();
        let rpc = BitcoinRpc::new(RpcConfig {
            url: String::new(),
            user: String::new(),
            password: String::new(),
        });
        let indexer = Indexer::new(rpc, store);

        let reg1 = make_register_op("alpha");
        let reg2 = make_register_op("beta");

        let block = Block {
            hash: "00000000abc".to_string(),
            height: 800_000,
            previous_block_hash: None,
            next_block_hash: None,
            tx: vec![
                make_tx("tx1", Some("bc1qa"), vec![make_op_return_output(&reg1)]),
                make_tx("tx2", Some("bc1qb"), vec![make_op_return_output(&reg2)]),
            ],
        };

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.registrations, 2);

        assert!(indexer
            .store()
            .get_name(&TitanName::new("alpha").unwrap())
            .unwrap()
            .is_some());
        assert!(indexer
            .store()
            .get_name(&TitanName::new("beta").unwrap())
            .unwrap()
            .is_some());
    }
}
