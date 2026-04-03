//! Titan name types — the Bitcoin OP_RETURN name protocol.

use serde::{Deserialize, Serialize};

/// Maximum name length in bytes (80 - 39 bytes overhead = 41).
pub const MAX_NAME_LEN: usize = 41;

/// Minimum name length.
pub const MIN_NAME_LEN: usize = 1;

/// Protocol magic bytes: "NSIT" (0x4E534954).
pub const MAGIC: [u8; 4] = [0x4E, 0x53, 0x49, 0x54];

/// Current protocol version.
pub const VERSION: u8 = 0x01;

/// A validated Titan name (lowercase a-z0-9 and hyphens).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TitanName(String);

impl TitanName {
    /// Create a new TitanName, validating the input.
    pub fn new(name: &str) -> Result<Self, NameError> {
        let name = name.to_ascii_lowercase();

        if name.len() < MIN_NAME_LEN {
            return Err(NameError::TooShort);
        }
        if name.len() > MAX_NAME_LEN {
            return Err(NameError::TooLong);
        }
        if name.starts_with('-') || name.ends_with('-') {
            return Err(NameError::InvalidHyphen);
        }
        if name.contains("--") {
            return Err(NameError::ConsecutiveHyphens);
        }
        if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(NameError::InvalidCharacter);
        }

        Ok(Self(name))
    }

    /// The name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name as bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for TitanName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for TitanName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Action byte in the OP_RETURN payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum OpAction {
    /// Register a new name (first-in-chain wins).
    Register = 0x00,
    /// Transfer a name to a new pubkey (requires owner signature).
    Transfer = 0x01,
}

impl OpAction {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x00 => Some(Self::Register),
            0x01 => Some(Self::Transfer),
            _ => None,
        }
    }
}

/// A decoded Titan OP_RETURN operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitanOp {
    pub action: OpAction,
    pub name: TitanName,
    /// 32-byte x-only Schnorr pubkey (same format as Nostr pubkeys).
    pub pubkey: [u8; 32],
}

/// Errors from name validation.
#[derive(Debug, Clone, thiserror::Error)]
pub enum NameError {
    #[error("name is too short (minimum {MIN_NAME_LEN} character)")]
    TooShort,
    #[error("name is too long (maximum {MAX_NAME_LEN} characters)")]
    TooLong,
    #[error("name cannot start or end with a hyphen")]
    InvalidHyphen,
    #[error("name cannot contain consecutive hyphens")]
    ConsecutiveHyphens,
    #[error("name contains invalid characters (only a-z, 0-9, and - allowed)")]
    InvalidCharacter,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names() {
        assert!(TitanName::new("westernbtc").is_ok());
        assert!(TitanName::new("a").is_ok());
        assert!(TitanName::new("my-site-123").is_ok());
        assert!(TitanName::new("x").is_ok());
        // 41 chars — max length
        assert!(TitanName::new(&"a".repeat(41)).is_ok());
    }

    #[test]
    fn invalid_names() {
        assert!(TitanName::new("").is_err()); // too short
        assert!(TitanName::new(&"a".repeat(42)).is_err()); // too long
        assert!(TitanName::new("-abc").is_err()); // leading hyphen
        assert!(TitanName::new("abc-").is_err()); // trailing hyphen
        assert!(TitanName::new("ab--cd").is_err()); // consecutive hyphens
        assert!(TitanName::new("Hello").is_ok()); // uppercase normalized
        assert_eq!(TitanName::new("Hello").unwrap().as_str(), "hello");
        assert!(TitanName::new("no spaces").is_err());
        assert!(TitanName::new("no.dots").is_err());
    }
}
