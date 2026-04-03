//! Blossom blob fetching with SHA256 integrity verification.
//!
//! Blossom servers serve content-addressed blobs at `GET /<sha256hex>`.
//! We verify the downloaded content matches the expected hash.

use reqwest::Client;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

/// HTTP client for fetching blobs from Blossom servers.
pub struct BlossomClient {
    client: Client,
}

impl BlossomClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Fetch a blob by SHA256 hash, trying each server in order.
    ///
    /// Returns the verified blob bytes, or an error if no server had a valid copy.
    pub async fn fetch_blob(
        &self,
        hash: &str,
        servers: &[String],
    ) -> Result<Vec<u8>, BlossomError> {
        if servers.is_empty() {
            return Err(BlossomError::NoServers);
        }

        let mut last_err = BlossomError::NoServers;

        for server in servers {
            let url = format!("{}/{}", server.trim_end_matches('/'), hash);
            debug!("fetching blob from {url}");

            match self.fetch_and_verify(&url, hash).await {
                Ok(bytes) => return Ok(bytes),
                Err(e) => {
                    warn!("failed to fetch blob from {server}: {e}");
                    last_err = e;
                }
            }
        }

        Err(last_err)
    }

    async fn fetch_and_verify(&self, url: &str, expected_hash: &str) -> Result<Vec<u8>, BlossomError> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| BlossomError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(BlossomError::Http(format!("HTTP {}", resp.status())));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| BlossomError::Http(e.to_string()))?;

        // Verify SHA256
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual_hash = hex::encode(hasher.finalize());

        if actual_hash != expected_hash {
            return Err(BlossomError::HashMismatch {
                expected: expected_hash.to_string(),
                actual: actual_hash,
            });
        }

        debug!("verified blob {expected_hash} ({} bytes)", bytes.len());
        Ok(bytes.to_vec())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BlossomError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("no blossom servers available")]
    NoServers,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn sha256_verification_logic() {
        let content = b"hello world";
        let mut hasher = Sha256::new();
        hasher.update(content);
        let hash = hex::encode(hasher.finalize());

        // Verify our hash computation matches the known SHA256 of "hello world"
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[tokio::test]
    async fn fetch_no_servers() {
        let client = BlossomClient::new();
        let result = client.fetch_blob("abc123", &[]).await;
        assert!(matches!(result, Err(BlossomError::NoServers)));
    }
}
