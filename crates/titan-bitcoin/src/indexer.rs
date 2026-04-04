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

    /// Find the first non-OP_RETURN output in a transaction (the ownership UTXO).
    fn first_non_opreturn_output(tx: &Transaction) -> Option<u32> {
        tx.vout
            .iter()
            .find(|o| o.script_pub_key.script_type != "nulldata")
            .map(|o| o.n)
    }

    /// Check if a transaction spends a specific UTXO (txid:vout) as any of its inputs.
    fn spends_utxo(tx: &Transaction, utxo_txid: &str, utxo_vout: u32) -> bool {
        tx.vin.iter().any(|input| {
            input.txid.as_deref() == Some(utxo_txid)
                && input.vout == Some(utxo_vout)
        })
    }

    /// Scan a single transaction for NSIT OP_RETURN outputs.
    ///
    /// UTXO ownership model:
    /// - Registration: first non-OP_RETURN output becomes the ownership UTXO
    /// - Transfer: must spend the current ownership UTXO; first non-OP_RETURN
    ///   output of the transfer tx becomes the new ownership UTXO
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
                    // Ownership UTXO = first non-OP_RETURN output of this tx
                    let owner_vout = match Self::first_non_opreturn_output(tx) {
                        Some(v) => v,
                        None => {
                            debug!("registration for '{}' has no non-OP_RETURN output, skipped", op.name);
                            result.skipped += 1;
                            continue;
                        }
                    };

                    let inserted = self.store.insert_name(
                        &op.name,
                        &op.pubkey,
                        &tx.txid,
                        owner_vout,
                        &tx.txid,
                        block_height,
                    )?;
                    if inserted {
                        debug!("registered name '{}' at height {block_height} (utxo {}:{})", op.name, tx.txid, owner_vout);
                        result.registrations += 1;
                    } else {
                        debug!("duplicate registration for '{}', skipped", op.name);
                        result.skipped += 1;
                    }
                }
                OpAction::Transfer => {
                    // Verify that this transaction spends the current ownership UTXO
                    let owner_utxo = self.store.get_owner_utxo(&op.name)?;
                    match owner_utxo {
                        Some((ref utxo_txid, utxo_vout))
                            if Self::spends_utxo(tx, utxo_txid, utxo_vout) =>
                        {
                            // New ownership UTXO = first non-OP_RETURN output of this tx
                            let new_owner_vout = match Self::first_non_opreturn_output(tx) {
                                Some(v) => v,
                                None => {
                                    debug!("transfer for '{}' has no non-OP_RETURN output, skipped", op.name);
                                    result.skipped += 1;
                                    continue;
                                }
                            };

                            self.store.transfer_name(
                                &op.name,
                                &op.pubkey,
                                &tx.txid,
                                new_owner_vout,
                                &tx.txid,
                                block_height,
                            )?;
                            debug!(
                                "transferred name '{}' at height {block_height} (new utxo {}:{})",
                                op.name, tx.txid, new_owner_vout
                            );
                            result.transfers += 1;
                        }
                        Some((ref utxo_txid, utxo_vout)) => {
                            debug!(
                                "transfer for '{}' rejected: tx doesn't spend ownership utxo {}:{}",
                                op.name, utxo_txid, utxo_vout
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

    fn dummy_rpc() -> BitcoinRpc {
        BitcoinRpc::new(RpcConfig {
            url: String::new(),
            user: String::new(),
            password: String::new(),
            wallet: None,
        })
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

    /// Build an OP_RETURN output for an NSIT payload.
    fn op_return_output(op: &titan_types::TitanOp, n: u32) -> TxOutput {
        let payload = codec::encode(op);
        let mut script = vec![0x6a, payload.len() as u8];
        script.extend_from_slice(&payload);
        TxOutput {
            n,
            script_pub_key: ScriptPubKey {
                hex: hex::encode(&script),
                script_type: "nulldata".to_string(),
                address: None,
            },
        }
    }

    /// Build a normal (non-OP_RETURN) output — this becomes the ownership UTXO.
    fn normal_output(n: u32) -> TxOutput {
        TxOutput {
            n,
            script_pub_key: ScriptPubKey {
                hex: "0014abcdef".to_string(),
                script_type: "witness_v0_keyhash".to_string(),
                address: Some("bc1qtest".to_string()),
            },
        }
    }

    /// Build a registration transaction: OP_RETURN + normal output (ownership UTXO).
    fn register_tx(txid: &str, op: &titan_types::TitanOp) -> Transaction {
        Transaction {
            txid: txid.to_string(),
            vin: vec![TxInput {
                txid: Some("funding_tx".to_string()),
                vout: Some(0),
                prevout: None,
            }],
            vout: vec![
                normal_output(0),       // ownership UTXO (vout 0)
                op_return_output(op, 1), // NSIT payload (vout 1)
            ],
        }
    }

    /// Build a transfer transaction that spends a specific UTXO as its first input.
    fn transfer_tx(
        txid: &str,
        spends_txid: &str,
        spends_vout: u32,
        op: &titan_types::TitanOp,
    ) -> Transaction {
        Transaction {
            txid: txid.to_string(),
            vin: vec![TxInput {
                txid: Some(spends_txid.to_string()),
                vout: Some(spends_vout),
                prevout: None,
            }],
            vout: vec![
                normal_output(0),       // new ownership UTXO (vout 0)
                op_return_output(op, 1), // NSIT transfer payload (vout 1)
            ],
        }
    }

    fn make_block(hash: &str, height: u64, txs: Vec<Transaction>) -> Block {
        Block {
            hash: hash.to_string(),
            height,
            previous_block_hash: None,
            next_block_hash: None,
            tx: txs,
        }
    }

    #[test]
    fn process_block_registration() {
        let indexer = Indexer::new(dummy_rpc(), NameStore::open_memory().unwrap());

        let op = make_register_op("westernbtc");
        let block = make_block("abc", 800_000, vec![register_tx("tx1", &op)]);

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.registrations, 1);
        assert_eq!(result.skipped, 0);

        let record = indexer.store().get_name(&TitanName::new("westernbtc").unwrap()).unwrap().unwrap();
        assert_eq!(record.pubkey, test_pubkey());
        assert_eq!(record.owner_txid, "tx1");
        assert_eq!(record.owner_vout, 0); // first non-OP_RETURN output
    }

    #[test]
    fn process_block_duplicate_registration() {
        let indexer = Indexer::new(dummy_rpc(), NameStore::open_memory().unwrap());

        let op = make_register_op("westernbtc");
        let block = make_block("abc", 800_000, vec![
            register_tx("tx1", &op),
            register_tx("tx2", &op),
        ]);

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.registrations, 1);
        assert_eq!(result.skipped, 1);

        // First-in-block wins
        let record = indexer.store().get_name(&TitanName::new("westernbtc").unwrap()).unwrap().unwrap();
        assert_eq!(record.owner_txid, "tx1");
    }

    #[test]
    fn process_block_valid_transfer() {
        let store = NameStore::open_memory().unwrap();
        // Pre-register: owned by UTXO tx0:0
        store.insert_name(
            &TitanName::new("westernbtc").unwrap(),
            &test_pubkey(), "tx0", 0, "tx0", 799_999,
        ).unwrap();

        let indexer = Indexer::new(dummy_rpc(), store);

        let op = make_transfer_op("westernbtc");
        // Transfer tx spends tx0:0 (the ownership UTXO)
        let block = make_block("def", 800_000, vec![
            transfer_tx("tx1", "tx0", 0, &op),
        ]);

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.transfers, 1);
        assert_eq!(result.skipped, 0);

        let record = indexer.store().get_name(&TitanName::new("westernbtc").unwrap()).unwrap().unwrap();
        assert_eq!(record.pubkey, other_pubkey());
        // New ownership UTXO is tx1:0
        assert_eq!(record.owner_txid, "tx1");
        assert_eq!(record.owner_vout, 0);
    }

    #[test]
    fn process_block_unauthorized_transfer() {
        let store = NameStore::open_memory().unwrap();
        store.insert_name(
            &TitanName::new("westernbtc").unwrap(),
            &test_pubkey(), "tx0", 0, "tx0", 799_999,
        ).unwrap();

        let indexer = Indexer::new(dummy_rpc(), store);

        let op = make_transfer_op("westernbtc");
        // Transfer tx spends "wrong_tx:0" — NOT the ownership UTXO
        let block = make_block("def", 800_000, vec![
            transfer_tx("tx1", "wrong_tx", 0, &op),
        ]);

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.transfers, 0);
        assert_eq!(result.skipped, 1);

        // Name unchanged
        let record = indexer.store().get_name(&TitanName::new("westernbtc").unwrap()).unwrap().unwrap();
        assert_eq!(record.pubkey, test_pubkey());
        assert_eq!(record.owner_txid, "tx0"); // still original
    }

    #[test]
    fn process_block_transfer_unregistered() {
        let indexer = Indexer::new(dummy_rpc(), NameStore::open_memory().unwrap());

        let op = make_transfer_op("nonexistent");
        let block = make_block("abc", 800_000, vec![
            transfer_tx("tx1", "whatever", 0, &op),
        ]);

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.transfers, 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn process_block_ignores_non_nsit_outputs() {
        let indexer = Indexer::new(dummy_rpc(), NameStore::open_memory().unwrap());

        let block = Block {
            hash: "abc".to_string(),
            height: 800_000,
            previous_block_hash: None,
            next_block_hash: None,
            tx: vec![Transaction {
                txid: "tx1".to_string(),
                vin: vec![TxInput { txid: None, vout: None, prevout: None }],
                vout: vec![
                    normal_output(0),
                    TxOutput {
                        n: 1,
                        script_pub_key: ScriptPubKey {
                            hex: "6a0468656c6c6f".to_string(),
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
    fn process_block_mixed_registrations() {
        let indexer = Indexer::new(dummy_rpc(), NameStore::open_memory().unwrap());

        let block = make_block("abc", 800_000, vec![
            register_tx("tx1", &make_register_op("alpha")),
            register_tx("tx2", &make_register_op("beta")),
        ]);

        let result = indexer.process_block(&block).unwrap();
        assert_eq!(result.registrations, 2);

        assert!(indexer.store().get_name(&TitanName::new("alpha").unwrap()).unwrap().is_some());
        assert!(indexer.store().get_name(&TitanName::new("beta").unwrap()).unwrap().is_some());
    }

    #[test]
    fn transfer_chain() {
        // Register → transfer → transfer again, each time ownership UTXO moves
        let store = NameStore::open_memory().unwrap();
        store.insert_name(
            &TitanName::new("myname").unwrap(),
            &test_pubkey(), "reg_tx", 0, "reg_tx", 100,
        ).unwrap();

        let indexer = Indexer::new(dummy_rpc(), store);

        // First transfer: spend reg_tx:0
        let op1 = make_transfer_op("myname");
        let block1 = make_block("b1", 101, vec![
            transfer_tx("xfer1", "reg_tx", 0, &op1),
        ]);
        indexer.process_block(&block1).unwrap();

        let record = indexer.store().get_name(&TitanName::new("myname").unwrap()).unwrap().unwrap();
        assert_eq!(record.owner_txid, "xfer1");
        assert_eq!(record.owner_vout, 0);

        // Second transfer: spend xfer1:0
        let op2 = titan_types::TitanOp {
            action: OpAction::Transfer,
            name: TitanName::new("myname").unwrap(),
            pubkey: test_pubkey(), // back to original pubkey
        };
        let block2 = make_block("b2", 102, vec![
            transfer_tx("xfer2", "xfer1", 0, &op2),
        ]);
        indexer.process_block(&block2).unwrap();

        let record = indexer.store().get_name(&TitanName::new("myname").unwrap()).unwrap().unwrap();
        assert_eq!(record.owner_txid, "xfer2");
        assert_eq!(record.owner_vout, 0);
        assert_eq!(record.pubkey, test_pubkey());
    }
}
