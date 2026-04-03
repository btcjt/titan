//! NSIT transaction builder — creates and broadcasts OP_RETURN transactions
//! for name registration and transfer using Bitcoin Core wallet RPCs.

use crate::codec;
use crate::rpc::{BitcoinRpc, RpcError};
use titan_types::{OpAction, TitanName, TitanOp};
use tracing::info;

/// Result of a successfully broadcast NSIT transaction.
#[derive(Debug)]
pub struct BroadcastResult {
    pub txid: String,
    pub name: String,
    pub action: OpAction,
    pub fee_btc: f64,
}

/// Build and broadcast a name registration transaction.
///
/// Flow (mirrors btc-nostrary's `sendOpReturnTx`):
/// 1. Encode the NSIT payload
/// 2. `getnewaddress` for change
/// 3. `createrawtransaction` with OP_RETURN output
/// 4. `fundrawtransaction` to add inputs and change
/// 5. `signrawtransactionwithwallet`
/// 6. `sendrawtransaction`
pub async fn register_name(
    rpc: &BitcoinRpc,
    name: &TitanName,
    pubkey: &[u8; 32],
) -> Result<BroadcastResult, TxError> {
    let op = TitanOp {
        action: OpAction::Register,
        name: name.clone(),
        pubkey: *pubkey,
    };
    broadcast_op(rpc, &op).await
}

/// Build and broadcast a name transfer transaction.
pub async fn transfer_name(
    rpc: &BitcoinRpc,
    name: &TitanName,
    new_pubkey: &[u8; 32],
) -> Result<BroadcastResult, TxError> {
    let op = TitanOp {
        action: OpAction::Transfer,
        name: name.clone(),
        pubkey: *new_pubkey,
    };
    broadcast_op(rpc, &op).await
}

/// Encode an NSIT operation and broadcast it as an OP_RETURN transaction.
async fn broadcast_op(rpc: &BitcoinRpc, op: &TitanOp) -> Result<BroadcastResult, TxError> {
    // 1. Encode the NSIT payload
    let payload = codec::encode(op);
    let data_hex = hex::encode(&payload);
    info!(
        "broadcasting {} for '{}' ({} bytes)",
        match op.action {
            OpAction::Register => "registration",
            OpAction::Transfer => "transfer",
        },
        op.name,
        payload.len()
    );

    // 2. Get a change address
    let change_address = rpc
        .get_new_address("titan-change")
        .await
        .map_err(TxError::Rpc)?;

    // 3. Create raw transaction with OP_RETURN output
    let raw_tx = rpc
        .create_op_return_tx(&data_hex)
        .await
        .map_err(TxError::Rpc)?;

    // 4. Fund the transaction (add inputs and change)
    let funded = rpc
        .fund_raw_transaction(&raw_tx, &change_address)
        .await
        .map_err(TxError::Rpc)?;

    // 5. Sign with wallet
    let signed = rpc
        .sign_raw_transaction(&funded.hex)
        .await
        .map_err(TxError::Rpc)?;

    if !signed.complete {
        return Err(TxError::SigningFailed);
    }

    // 6. Broadcast
    let txid = rpc
        .send_raw_transaction(&signed.hex)
        .await
        .map_err(TxError::Rpc)?;

    info!("broadcast txid: {txid} (fee: {} BTC)", funded.fee);

    Ok(BroadcastResult {
        txid,
        name: op.name.to_string(),
        action: op.action,
        fee_btc: funded.fee,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum TxError {
    #[error("RPC error: {0}")]
    Rpc(RpcError),
    #[error("transaction signing incomplete — wallet may be locked")]
    SigningFailed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nsit_payload_is_valid_hex() {
        let op = TitanOp {
            action: OpAction::Register,
            name: TitanName::new("westernbtc").unwrap(),
            pubkey: [0xab; 32],
        };
        let payload = codec::encode(&op);
        let hex_str = hex::encode(&payload);

        // Should be valid hex, 49 bytes for "westernbtc" (39 + 10)
        assert_eq!(hex_str.len(), 98); // 49 bytes * 2 hex chars
        assert!(hex_str.starts_with("4e534954")); // NSIT magic
    }
}
