//! OP_RETURN binary codec for the Titan name protocol.
//!
//! ## Wire format (80 bytes max)
//!
//! ```text
//! Offset  Size  Field       Description
//! 0       4     magic       "TITN" (0x5449544E)
//! 4       1     version     0x01
//! 5       1     action      0x00=register, 0x01=transfer
//! 6       1     name_len    1-41
//! 7       N     name        [a-z0-9-]
//! 7+N     32    pubkey      32-byte x-only Schnorr pubkey
//! ```

use titan_types::{TitanName, TitanOp, OpAction};
use titan_types::name::{MAGIC, VERSION};

/// Maximum OP_RETURN payload size.
const MAX_PAYLOAD: usize = 80;

/// Fixed overhead: magic(4) + version(1) + action(1) + name_len(1) + pubkey(32) = 39.
const FIXED_OVERHEAD: usize = 39;

/// Encode a `TitanOp` into an OP_RETURN payload.
pub fn encode(op: &TitanOp) -> Vec<u8> {
    let name_bytes = op.name.as_bytes();
    let mut buf = Vec::with_capacity(FIXED_OVERHEAD + name_bytes.len());

    buf.extend_from_slice(&MAGIC);
    buf.push(VERSION);
    buf.push(op.action as u8);
    buf.push(name_bytes.len() as u8);
    buf.extend_from_slice(name_bytes);
    buf.extend_from_slice(&op.pubkey);

    debug_assert!(buf.len() <= MAX_PAYLOAD);
    buf
}

/// Attempt to decode an OP_RETURN payload as a `TitanOp`.
///
/// Returns `None` if the data is not a valid Titan payload (wrong magic,
/// invalid version, malformed name, etc.).
pub fn decode(data: &[u8]) -> Option<TitanOp> {
    // Minimum size: 4 (magic) + 1 (version) + 1 (action) + 1 (name_len) + 1 (min name) + 32 (pubkey) = 40
    if data.len() < FIXED_OVERHEAD + 1 {
        return None;
    }

    // Check magic
    if data[0..4] != MAGIC {
        return None;
    }

    // Check version
    if data[4] != VERSION {
        return None;
    }

    // Parse action
    let action = OpAction::from_byte(data[5])?;

    // Parse name length
    let name_len = data[6] as usize;
    if name_len == 0 || name_len > MAX_PAYLOAD - FIXED_OVERHEAD {
        return None;
    }

    // Check total length
    let expected_len = FIXED_OVERHEAD + name_len;
    if data.len() < expected_len {
        return None;
    }

    // Extract and validate name
    let name_bytes = &data[7..7 + name_len];
    let name_str = std::str::from_utf8(name_bytes).ok()?;
    let name = TitanName::new(name_str).ok()?;

    // Extract pubkey
    let pubkey_start = 7 + name_len;
    let pubkey_slice = &data[pubkey_start..pubkey_start + 32];
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(pubkey_slice);

    Some(TitanOp {
        action,
        name,
        pubkey,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use titan_types::name::MAX_NAME_LEN;

    fn test_pubkey() -> [u8; 32] {
        let mut pk = [0u8; 32];
        pk[0] = 0x0e;
        pk[1] = 0x29;
        pk[31] = 0xf2;
        pk
    }

    #[test]
    fn round_trip_register() {
        let op = TitanOp {
            action: OpAction::Register,
            name: TitanName::new("westernbtc").unwrap(),
            pubkey: test_pubkey(),
        };

        let encoded = encode(&op);
        assert!(encoded.len() <= 80);
        assert_eq!(encoded.len(), 39 + 10); // 10 = "westernbtc".len()

        let decoded = decode(&encoded).expect("should decode");
        assert_eq!(decoded.action, OpAction::Register);
        assert_eq!(decoded.name.as_str(), "westernbtc");
        assert_eq!(decoded.pubkey, test_pubkey());
    }

    #[test]
    fn round_trip_transfer() {
        let op = TitanOp {
            action: OpAction::Transfer,
            name: TitanName::new("westernbtc").unwrap(),
            pubkey: test_pubkey(),
        };

        let encoded = encode(&op);
        let decoded = decode(&encoded).expect("should decode");
        assert_eq!(decoded.action, OpAction::Transfer);
    }

    #[test]
    fn max_length_name() {
        let long_name = "a".repeat(MAX_NAME_LEN);
        let op = TitanOp {
            action: OpAction::Register,
            name: TitanName::new(&long_name).unwrap(),
            pubkey: test_pubkey(),
        };

        let encoded = encode(&op);
        assert_eq!(encoded.len(), 80); // exactly at limit
        let decoded = decode(&encoded).expect("should decode max-length name");
        assert_eq!(decoded.name.as_str(), long_name);
    }

    #[test]
    fn single_char_name() {
        let op = TitanOp {
            action: OpAction::Register,
            name: TitanName::new("x").unwrap(),
            pubkey: test_pubkey(),
        };

        let encoded = encode(&op);
        assert_eq!(encoded.len(), 40); // minimum size
        let decoded = decode(&encoded).expect("should decode");
        assert_eq!(decoded.name.as_str(), "x");
    }

    #[test]
    fn reject_wrong_magic() {
        let mut data = encode(&TitanOp {
            action: OpAction::Register,
            name: TitanName::new("test").unwrap(),
            pubkey: test_pubkey(),
        });
        data[0] = 0xFF; // corrupt magic
        assert!(decode(&data).is_none());
    }

    #[test]
    fn reject_wrong_version() {
        let mut data = encode(&TitanOp {
            action: OpAction::Register,
            name: TitanName::new("test").unwrap(),
            pubkey: test_pubkey(),
        });
        data[4] = 0x02; // unknown version
        assert!(decode(&data).is_none());
    }

    #[test]
    fn reject_truncated() {
        let data = encode(&TitanOp {
            action: OpAction::Register,
            name: TitanName::new("test").unwrap(),
            pubkey: test_pubkey(),
        });
        // Truncate — missing pubkey bytes
        assert!(decode(&data[..data.len() - 5]).is_none());
    }

    #[test]
    fn reject_empty() {
        assert!(decode(&[]).is_none());
        assert!(decode(&[0x54, 0x49]).is_none());
    }

    #[test]
    fn reject_invalid_action() {
        let mut data = encode(&TitanOp {
            action: OpAction::Register,
            name: TitanName::new("test").unwrap(),
            pubkey: test_pubkey(),
        });
        data[5] = 0xFF; // invalid action
        assert!(decode(&data).is_none());
    }
}
