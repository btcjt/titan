//! Top-level error types for Titan.

use crate::name::NameError;
use crate::url::UrlError;

#[derive(Debug, thiserror::Error)]
pub enum TitanError {
    #[error("name error: {0}")]
    Name(#[from] NameError),

    #[error("URL error: {0}")]
    Url(#[from] UrlError),

    #[error("name not found: {0}")]
    NameNotFound(String),

    #[error("manifest not found for pubkey")]
    ManifestNotFound,

    #[error("path not found in manifest: {0}")]
    PathNotFound(String),

    #[error("blob not found: {0}")]
    BlobNotFound(String),

    #[error("relay error: {0}")]
    Relay(String),

    #[error("bitcoin RPC error: {0}")]
    BitcoinRpc(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("{0}")]
    Other(String),
}
