//! Bitcoin Core JSON-RPC client.
//!
//! Minimal client for the three RPCs needed by the block scanner:
//! `getblockchaininfo`, `getblockhash`, and `getblock` (verbosity=2).

use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Configuration for connecting to Bitcoin Core.
#[derive(Debug, Clone)]
pub struct RpcConfig {
    /// URL of the Bitcoin Core RPC server (e.g. "http://127.0.0.1:8332").
    pub url: String,
    /// RPC username from bitcoin.conf.
    pub user: String,
    /// RPC password from bitcoin.conf.
    pub password: String,
    /// Wallet name for wallet-specific RPCs (optional).
    pub wallet: Option<String>,
}

/// Bitcoin Core JSON-RPC client.
pub struct BitcoinRpc {
    client: Client,
    config: RpcConfig,
}

// ── JSON-RPC request / response envelopes ──

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'a str,
    id: u64,
    method: &'a str,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcErrorObj>,
}

#[derive(Debug, Deserialize)]
struct RpcErrorObj {
    code: i64,
    message: String,
}

// ── Domain types ──

/// Subset of `getblockchaininfo` we care about.
#[derive(Debug, Deserialize)]
pub struct BlockchainInfo {
    pub chain: String,
    pub blocks: u64,
    pub headers: u64,
    #[serde(rename = "bestblockhash")]
    pub best_block_hash: String,
    #[serde(rename = "initialblockdownload")]
    pub initial_block_download: bool,
}

/// A block with decoded transactions (verbosity=2).
#[derive(Debug, Deserialize)]
pub struct Block {
    pub hash: String,
    pub height: u64,
    #[serde(rename = "previousblockhash")]
    pub previous_block_hash: Option<String>,
    #[serde(rename = "nextblockhash")]
    pub next_block_hash: Option<String>,
    pub tx: Vec<Transaction>,
}

/// A decoded transaction within a block.
#[derive(Debug, Deserialize)]
pub struct Transaction {
    pub txid: String,
    pub vin: Vec<TxInput>,
    pub vout: Vec<TxOutput>,
}

/// Transaction input.
#[derive(Debug, Deserialize)]
pub struct TxInput {
    /// Previous output txid (absent for coinbase).
    pub txid: Option<String>,
    /// Previous output index.
    pub vout: Option<u32>,
    /// Decoded previous output (present with verbosity=2).
    #[serde(rename = "prevout")]
    pub prevout: Option<PrevOut>,
}

/// Previous output details (available at verbosity=2).
#[derive(Debug, Deserialize)]
pub struct PrevOut {
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: ScriptPubKey,
}

/// Script public key with optional address.
#[derive(Debug, Deserialize)]
pub struct ScriptPubKey {
    /// The script as hex.
    pub hex: String,
    /// The script type (e.g. "witness_v1_taproot", "nulldata").
    #[serde(rename = "type")]
    pub script_type: String,
    /// The address, if applicable.
    pub address: Option<String>,
}

/// Transaction output.
#[derive(Debug, Deserialize)]
pub struct TxOutput {
    pub n: u32,
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: ScriptPubKey,
}

// ── Wallet response types ──

/// Response from `fundrawtransaction`.
#[derive(Debug, Deserialize)]
pub struct FundedTransaction {
    pub hex: String,
    pub fee: f64,
    #[serde(rename = "changepos")]
    pub change_pos: i64,
}

/// Response from `signrawtransactionwithwallet`.
#[derive(Debug, Deserialize)]
pub struct SignedTransaction {
    pub hex: String,
    pub complete: bool,
}

// ── RPC error ──

/// Errors from Bitcoin Core RPC calls.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("RPC error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("null result from RPC")]
    NullResult,
}

// ── Implementation ──

impl BitcoinRpc {
    /// Create a new RPC client with the given configuration.
    pub fn new(config: RpcConfig) -> Self {
        let client = Client::new();
        Self { client, config }
    }

    /// Raw JSON-RPC call.
    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, RpcError> {
        let req = RpcRequest {
            jsonrpc: "1.0",
            id: 1,
            method,
            params,
        };

        let resp = self
            .client
            .post(&self.config.url)
            .basic_auth(&self.config.user, Some(&self.config.password))
            .json(&req)
            .send()
            .await?
            .json::<RpcResponse<T>>()
            .await?;

        if let Some(err) = resp.error {
            return Err(RpcError::Rpc {
                code: err.code,
                message: err.message,
            });
        }

        resp.result.ok_or(RpcError::NullResult)
    }

    /// Get blockchain info (chain, height, sync status).
    pub async fn get_blockchain_info(&self) -> Result<BlockchainInfo, RpcError> {
        self.call("getblockchaininfo", serde_json::json!([])).await
    }

    /// Get block hash at a given height.
    pub async fn get_block_hash(&self, height: u64) -> Result<String, RpcError> {
        self.call("getblockhash", serde_json::json!([height])).await
    }

    /// Get a full block with decoded transactions (verbosity=2).
    pub async fn get_block(&self, hash: &str) -> Result<Block, RpcError> {
        self.call("getblock", serde_json::json!([hash, 2])).await
    }

    /// Raw JSON-RPC call to the wallet endpoint.
    async fn wallet_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, RpcError> {
        let url = match &self.config.wallet {
            Some(w) => format!("{}/wallet/{}", self.config.url, w),
            None => self.config.url.clone(),
        };

        let req = RpcRequest {
            jsonrpc: "1.0",
            id: 1,
            method,
            params,
        };

        let resp = self
            .client
            .post(&url)
            .basic_auth(&self.config.user, Some(&self.config.password))
            .json(&req)
            .send()
            .await?
            .json::<RpcResponse<T>>()
            .await?;

        if let Some(err) = resp.error {
            return Err(RpcError::Rpc {
                code: err.code,
                message: err.message,
            });
        }

        resp.result.ok_or(RpcError::NullResult)
    }

    // ── Wallet RPCs (for name registration/transfer) ──

    /// Get a new bech32 address from the wallet.
    pub async fn get_new_address(&self, label: &str) -> Result<String, RpcError> {
        self.wallet_call("getnewaddress", serde_json::json!([label, "bech32"]))
            .await
    }

    /// Create a raw transaction with an OP_RETURN output.
    /// `data_hex` is the hex-encoded payload (e.g. NSIT-encoded name registration).
    pub async fn create_op_return_tx(&self, data_hex: &str) -> Result<String, RpcError> {
        self.wallet_call(
            "createrawtransaction",
            serde_json::json!([[], [{"data": data_hex}]]),
        )
        .await
    }

    /// Fund a raw transaction (add inputs and change output).
    pub async fn fund_raw_transaction(
        &self,
        raw_tx_hex: &str,
        change_address: &str,
    ) -> Result<FundedTransaction, RpcError> {
        self.wallet_call(
            "fundrawtransaction",
            serde_json::json!([raw_tx_hex, {"changeAddress": change_address}]),
        )
        .await
    }

    /// Sign a raw transaction with the wallet's keys.
    pub async fn sign_raw_transaction(
        &self,
        raw_tx_hex: &str,
    ) -> Result<SignedTransaction, RpcError> {
        self.wallet_call(
            "signrawtransactionwithwallet",
            serde_json::json!([raw_tx_hex]),
        )
        .await
    }

    /// Broadcast a signed raw transaction.
    pub async fn send_raw_transaction(&self, raw_tx_hex: &str) -> Result<String, RpcError> {
        self.call("sendrawtransaction", serde_json::json!([raw_tx_hex]))
            .await
    }
}

impl TxOutput {
    /// If this is an OP_RETURN output, return the data payload (after OP_RETURN + push opcode).
    pub fn op_return_data(&self) -> Option<Vec<u8>> {
        if self.script_pub_key.script_type != "nulldata" {
            return None;
        }
        let script = hex::decode(&self.script_pub_key.hex).ok()?;
        // OP_RETURN (0x6a) followed by a push opcode
        if script.len() < 2 || script[0] != 0x6a {
            return None;
        }
        // Single-byte push: script[1] is the length, data follows
        let push_len = script[1] as usize;
        if script.len() < 2 + push_len {
            return None;
        }
        Some(script[2..2 + push_len].to_vec())
    }
}

impl Transaction {
    /// Get the address of the first input (the "owner" for TNP purposes).
    pub fn first_input_address(&self) -> Option<&str> {
        self.vin
            .first()?
            .prevout
            .as_ref()?
            .script_pub_key
            .address
            .as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_op_return_data() {
        // OP_RETURN (6a) + push 4 bytes (04) + "NSIT"
        let output = TxOutput {
            n: 0,
            script_pub_key: ScriptPubKey {
                hex: "6a044e534954".to_string(),
                script_type: "nulldata".to_string(),
                address: None,
            },
        };
        let data = output.op_return_data().expect("should parse");
        assert_eq!(data, b"NSIT");
    }

    #[test]
    fn non_op_return_returns_none() {
        let output = TxOutput {
            n: 0,
            script_pub_key: ScriptPubKey {
                hex: "76a91489abcdefab010000000000000000000000000000008ac".to_string(),
                script_type: "pubkeyhash".to_string(),
                address: Some("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string()),
            },
        };
        assert!(output.op_return_data().is_none());
    }

    #[test]
    fn first_input_address_coinbase() {
        let tx = Transaction {
            txid: "abc".to_string(),
            vin: vec![TxInput {
                txid: None,
                vout: None,
                prevout: None,
            }],
            vout: vec![],
        };
        assert!(tx.first_input_address().is_none());
    }

    #[test]
    fn first_input_address_normal() {
        let tx = Transaction {
            txid: "abc".to_string(),
            vin: vec![TxInput {
                txid: Some("prev_tx".to_string()),
                vout: Some(0),
                prevout: Some(PrevOut {
                    script_pub_key: ScriptPubKey {
                        hex: "0014abc".to_string(),
                        script_type: "witness_v0_keyhash".to_string(),
                        address: Some("bc1qowner123".to_string()),
                    },
                }),
            }],
            vout: vec![],
        };
        assert_eq!(tx.first_input_address(), Some("bc1qowner123"));
    }
}
