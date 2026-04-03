//! nsite:// URL parsing.

use crate::name::TitanName;

/// A parsed nsite:// URL.
#[derive(Debug, Clone)]
pub enum NsiteUrl {
    /// nsite://name — resolved via Bitcoin name index.
    Name {
        name: TitanName,
        path: String,
    },
    /// nsite://npub1... — direct pubkey, skip name resolution.
    Npub {
        pubkey: [u8; 32],
        path: String,
    },
}

impl NsiteUrl {
    /// Parse an nsite:// URL string.
    ///
    /// Accepted formats:
    /// - `nsite://westernbtc` or `nsite://westernbtc/path/to/page`
    /// - `nsite://npub1...` or `nsite://npub1.../path/to/page`
    pub fn parse(url: &str) -> Result<Self, UrlError> {
        let rest = url
            .strip_prefix("nsite://")
            .ok_or(UrlError::InvalidScheme)?;

        if rest.is_empty() {
            return Err(UrlError::Empty);
        }

        // Split host from path
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };

        // Ensure path always starts with /
        let path = if path.is_empty() { "/" } else { path };

        // Check if it's an npub
        if host.starts_with("npub1") {
            let pubkey = decode_npub(host)?;
            return Ok(Self::Npub {
                pubkey,
                path: path.to_string(),
            });
        }

        // Otherwise treat as a Titan name
        let name = TitanName::new(host).map_err(|e| UrlError::InvalidName(e.to_string()))?;
        Ok(Self::Name {
            name,
            path: path.to_string(),
        })
    }

    /// The path component (always starts with /).
    pub fn path(&self) -> &str {
        match self {
            Self::Name { path, .. } | Self::Npub { path, .. } => path,
        }
    }

    /// The 32-byte pubkey (resolved or direct).
    pub fn pubkey(&self) -> Option<&[u8; 32]> {
        match self {
            Self::Npub { pubkey, .. } => Some(pubkey),
            Self::Name { .. } => None,
        }
    }
}

/// Decode an npub1... bech32 string to a 32-byte pubkey.
fn decode_npub(npub: &str) -> Result<[u8; 32], UrlError> {
    // Use bech32 decoding: npub1 prefix, 32 bytes of data
    // For now, we'll use hex as a placeholder until we wire up bech32
    // TODO: proper bech32m decoding via nostr crate
    if !npub.starts_with("npub1") || npub.len() < 10 {
        return Err(UrlError::InvalidNpub);
    }
    // Placeholder — will be replaced with proper bech32 decoding
    Err(UrlError::InvalidNpub)
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum UrlError {
    #[error("URL must start with nsite://")]
    InvalidScheme,
    #[error("empty URL")]
    Empty,
    #[error("invalid npub")]
    InvalidNpub,
    #[error("invalid name: {0}")]
    InvalidName(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_name_urls() {
        let url = NsiteUrl::parse("nsite://westernbtc").unwrap();
        match url {
            NsiteUrl::Name { name, path } => {
                assert_eq!(name.as_str(), "westernbtc");
                assert_eq!(path, "/");
            }
            _ => panic!("expected Name variant"),
        }

        let url = NsiteUrl::parse("nsite://my-site/blog/post.html").unwrap();
        match url {
            NsiteUrl::Name { name, path } => {
                assert_eq!(name.as_str(), "my-site");
                assert_eq!(path, "/blog/post.html");
            }
            _ => panic!("expected Name variant"),
        }
    }

    #[test]
    fn parse_errors() {
        assert!(NsiteUrl::parse("https://example.com").is_err());
        assert!(NsiteUrl::parse("nsite://").is_err());
        assert!(NsiteUrl::parse("nsite://-invalid").is_err());
    }
}
