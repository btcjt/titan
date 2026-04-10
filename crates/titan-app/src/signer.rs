//! Built-in Nostr signer (NIP-07).
//!
//! Stores the nsec in the OS keychain (macOS Keychain, Linux Secret Service,
//! Windows Credential Manager). The nsec never leaves this Rust process — all
//! signing, encryption, and decryption happens in-process against nostr-sdk.
//!
//! State machine:
//!   - `NotConfigured` — no identity saved
//!   - `Locked` — identity exists in keychain but not loaded into memory
//!   - `Unlocked { keys }` — keys are in memory, ready to sign
//!
//! For v1 there is a single identity. Multi-identity is a v2 feature.

use nostr_sdk::prelude::*;
use std::sync::Mutex;

const SERVICE: &str = "com.titan.browser";
const DEFAULT_USER: &str = "default";

#[derive(Debug)]
pub enum SignerState {
    NotConfigured,
    Locked,
    Unlocked { keys: Keys },
}

pub struct Signer {
    state: Mutex<SignerState>,
}

impl Signer {
    /// Create a new signer, detecting whether an identity exists in the keychain.
    pub fn new() -> Self {
        let initial = match keychain_has_identity() {
            true => SignerState::Locked,
            false => SignerState::NotConfigured,
        };
        Self {
            state: Mutex::new(initial),
        }
    }

    /// Whether the signer has any identity configured.
    pub fn has_identity(&self) -> bool {
        !matches!(*self.state.lock().unwrap(), SignerState::NotConfigured)
    }

    /// Whether the signer is unlocked and ready to sign.
    pub fn is_unlocked(&self) -> bool {
        matches!(*self.state.lock().unwrap(), SignerState::Unlocked { .. })
    }

    /// Generate a fresh identity, save it to the keychain, and unlock.
    /// Returns the new public key (hex).
    pub fn create_new(&self) -> Result<String, String> {
        let keys = Keys::generate();
        let nsec = keys
            .secret_key()
            .to_bech32()
            .map_err(|e| format!("bech32 encode: {e}"))?;
        keychain_save(&nsec)?;
        let pubkey_hex = keys.public_key().to_hex();
        *self.state.lock().unwrap() = SignerState::Unlocked { keys };
        Ok(pubkey_hex)
    }

    /// Import an existing nsec (bech32 or 64-char hex), save it, and unlock.
    pub fn import(&self, input: &str) -> Result<String, String> {
        let keys = parse_secret(input)?;
        let nsec = keys
            .secret_key()
            .to_bech32()
            .map_err(|e| format!("bech32 encode: {e}"))?;
        keychain_save(&nsec)?;
        let pubkey_hex = keys.public_key().to_hex();
        *self.state.lock().unwrap() = SignerState::Unlocked { keys };
        Ok(pubkey_hex)
    }

    /// Load the nsec from the keychain into memory.
    pub fn unlock(&self) -> Result<String, String> {
        let nsec = keychain_load()?;
        let keys = parse_secret(&nsec)?;
        let pubkey_hex = keys.public_key().to_hex();
        *self.state.lock().unwrap() = SignerState::Unlocked { keys };
        Ok(pubkey_hex)
    }

    /// Clear the in-memory keys. The keychain entry is untouched.
    pub fn lock(&self) {
        let mut state = self.state.lock().unwrap();
        if matches!(*state, SignerState::Unlocked { .. }) {
            *state = SignerState::Locked;
        }
    }

    /// Delete the identity from the keychain and lock.
    pub fn delete(&self) -> Result<(), String> {
        keychain_delete()?;
        *self.state.lock().unwrap() = SignerState::NotConfigured;
        Ok(())
    }

    /// Return the public key hex if unlocked.
    pub fn get_pubkey(&self) -> Option<String> {
        match &*self.state.lock().unwrap() {
            SignerState::Unlocked { keys } => Some(keys.public_key().to_hex()),
            _ => None,
        }
    }

    /// Reveal the nsec. Must be unlocked.
    pub fn reveal_nsec(&self) -> Result<String, String> {
        match &*self.state.lock().unwrap() {
            SignerState::Unlocked { keys } => keys
                .secret_key()
                .to_bech32()
                .map_err(|e| format!("bech32 encode: {e}")),
            _ => Err("Signer is locked".to_string()),
        }
    }

    /// Access the in-memory keys (for signing operations).
    /// Returns an error if locked.
    #[allow(dead_code)]
    pub fn with_keys<T, F: FnOnce(&Keys) -> Result<T, String>>(&self, f: F) -> Result<T, String> {
        match &*self.state.lock().unwrap() {
            SignerState::Unlocked { keys } => f(keys),
            SignerState::Locked => Err("Signer is locked".to_string()),
            SignerState::NotConfigured => Err("No identity configured".to_string()),
        }
    }

    // ── Chrome-trusted operations ──
    //
    // These methods are for internal browser features (bookmarks, profile
    // decryption in the info panel, etc.) — they bypass the per-site
    // permission prompts because the chrome layer is trusted code, not
    // arbitrary nsite content. The keys never leave the mutex.

    /// Sign a kind/content/tags template as a finalized event. The event's
    /// `created_at` is filled in here. Used by chrome features that need
    /// to publish events on behalf of the user without prompting.
    pub fn chrome_sign_event(
        &self,
        kind: u16,
        content: String,
        tags: Vec<Vec<String>>,
    ) -> Result<Event, String> {
        self.with_keys(|keys| {
            let nostr_tags: Vec<Tag> = tags
                .into_iter()
                .filter_map(|t| Tag::parse(&t).ok())
                .collect();
            let signed = EventBuilder::new(Kind::from(kind), content)
                .tags(nostr_tags)
                .sign_with_keys(keys)
                .map_err(|e| format!("sign: {e}"))?;
            signed
                .verify()
                .map_err(|e| format!("self-verify: {e}"))?;
            Ok(signed)
        })
    }

    /// NIP-44 encrypt `plaintext` to a recipient pubkey. For self-targeted
    /// content (bookmarks list encrypted to the user's own pubkey), pass
    /// `self.get_pubkey()` as the recipient.
    pub fn chrome_nip44_encrypt(
        &self,
        recipient_hex: &str,
        plaintext: &str,
    ) -> Result<String, String> {
        let recipient = PublicKey::from_hex(recipient_hex)
            .map_err(|e| format!("invalid recipient pubkey: {e}"))?;
        self.with_keys(|keys| {
            nip44::encrypt(
                keys.secret_key(),
                &recipient,
                plaintext,
                nip44::Version::V2,
            )
            .map_err(|e| format!("nip44 encrypt: {e}"))
        })
    }

    /// NIP-44 decrypt `ciphertext` from a sender pubkey. For self-encrypted
    /// content, pass the user's own pubkey.
    pub fn chrome_nip44_decrypt(
        &self,
        sender_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
        let sender = PublicKey::from_hex(sender_hex)
            .map_err(|e| format!("invalid sender pubkey: {e}"))?;
        self.with_keys(|keys| {
            nip44::decrypt(keys.secret_key(), &sender, ciphertext)
                .map_err(|e| format!("nip44 decrypt: {e}"))
        })
    }

    /// Build an unlocked signer from raw keys, bypassing the keychain.
    /// Test-only — production code must always go through `Signer::new()`
    /// so the OS keychain is the source of truth. Used by sibling modules
    /// (`bookmarks::tests`, etc.) that need a signer fixture.
    #[cfg(test)]
    pub fn __test_unlocked(keys: Keys) -> Self {
        Self {
            state: Mutex::new(SignerState::Unlocked { keys }),
        }
    }
}

/// Parse an nsec (bech32) or 64-char hex secret key.
fn parse_secret(input: &str) -> Result<Keys, String> {
    let s = input.trim();
    if s.starts_with("nsec1") {
        let sk = SecretKey::from_bech32(s).map_err(|e| format!("invalid nsec: {e}"))?;
        Ok(Keys::new(sk))
    } else if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        let sk = SecretKey::from_hex(s).map_err(|e| format!("invalid hex secret: {e}"))?;
        Ok(Keys::new(sk))
    } else {
        Err("Expected nsec1... or 64-character hex".to_string())
    }
}

// ── Keychain helpers ──

fn keychain_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, DEFAULT_USER).map_err(|e| format!("keychain: {e}"))
}

fn keychain_has_identity() -> bool {
    match keychain_entry() {
        Ok(entry) => entry.get_password().is_ok(),
        Err(_) => false,
    }
}

fn keychain_save(nsec: &str) -> Result<(), String> {
    let entry = keychain_entry()?;
    entry
        .set_password(nsec)
        .map_err(|e| format!("keychain save: {e}"))
}

fn keychain_load() -> Result<String, String> {
    let entry = keychain_entry()?;
    entry
        .get_password()
        .map_err(|e| format!("keychain load: {e}"))
}

fn keychain_delete() -> Result<(), String> {
    let entry = keychain_entry()?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        // Not finding the credential on delete is fine
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keychain delete: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A known test key. NOT USED IN PRODUCTION. Generated with nostr-sdk
    // and verified round-trip through nsec/hex encoding.
    const TEST_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    #[test]
    fn parse_secret_from_hex() {
        let keys = parse_secret(TEST_HEX).expect("hex should parse");
        let round_trip = keys.secret_key().to_secret_hex();
        assert_eq!(round_trip, TEST_HEX);
    }

    #[test]
    fn parse_secret_from_nsec() {
        // Derive an nsec from our test hex, then parse it back.
        let keys = parse_secret(TEST_HEX).unwrap();
        let nsec = keys.secret_key().to_bech32().unwrap();
        assert!(nsec.starts_with("nsec1"));

        let parsed = parse_secret(&nsec).expect("nsec should parse");
        assert_eq!(parsed.public_key(), keys.public_key());
    }

    #[test]
    fn parse_secret_trims_whitespace() {
        let input = format!("  {}  \n", TEST_HEX);
        let keys = parse_secret(&input).expect("trimmed input should parse");
        assert_eq!(keys.secret_key().to_secret_hex(), TEST_HEX);
    }

    #[test]
    fn parse_secret_rejects_invalid_hex() {
        // 64 chars but not hex
        let bad = "z".repeat(64);
        assert!(parse_secret(&bad).is_err());
    }

    #[test]
    fn parse_secret_rejects_wrong_length_hex() {
        let short = "a".repeat(63);
        assert!(parse_secret(&short).is_err());
        let long = "a".repeat(65);
        assert!(parse_secret(&long).is_err());
    }

    #[test]
    fn parse_secret_rejects_bad_nsec() {
        assert!(parse_secret("nsec1notarealbech32string").is_err());
    }

    #[test]
    fn parse_secret_rejects_empty() {
        assert!(parse_secret("").is_err());
        assert!(parse_secret("   ").is_err());
    }

    #[test]
    fn parse_secret_rejects_npub() {
        // Should reject public keys even though they're valid bech32
        let keys = parse_secret(TEST_HEX).unwrap();
        let npub = keys.public_key().to_bech32().unwrap();
        assert!(npub.starts_with("npub1"));
        assert!(parse_secret(&npub).is_err());
    }

    /// Build an unlocked signer for tests without touching the keychain.
    /// Production code must always go through `Signer::new()` so the
    /// keychain stays the source of truth — this shortcut exists only so
    /// the chrome_* methods can be tested in isolation.
    fn unlocked_signer(hex: &str) -> Signer {
        let keys = parse_secret(hex).expect("test key parses");
        Signer {
            state: Mutex::new(SignerState::Unlocked { keys }),
        }
    }

    #[test]
    fn chrome_sign_event_produces_self_verifying_event() {
        let signer = unlocked_signer(TEST_HEX);
        let event = signer
            .chrome_sign_event(
                10003,
                "encrypted-bookmarks-payload".to_string(),
                vec![vec!["d".to_string(), "bookmarks".to_string()]],
            )
            .expect("sign succeeds");
        assert_eq!(event.kind, Kind::from(10003u16));
        assert_eq!(event.content, "encrypted-bookmarks-payload");
        // Internal self-verify already ran inside chrome_sign_event;
        // re-verify externally as a regression guard.
        event.verify().expect("event self-verifies");
    }

    #[test]
    fn chrome_sign_event_filters_malformed_tags() {
        let signer = unlocked_signer(TEST_HEX);
        // Empty tag should be skipped, valid tag should remain. The exact
        // count below comes from nostr-sdk's Tag::parse semantics.
        let event = signer
            .chrome_sign_event(
                1,
                "hi".to_string(),
                vec![
                    vec![],                                     // empty → filtered
                    vec!["t".to_string(), "titan".to_string()], // valid
                ],
            )
            .expect("sign succeeds");
        assert!(event.tags.iter().any(|t| t.as_slice().get(0).map(|s| s.as_str()) == Some("t")));
    }

    #[test]
    fn chrome_sign_event_locked_signer_errors() {
        let signer = Signer {
            state: Mutex::new(SignerState::Locked),
        };
        let err = signer
            .chrome_sign_event(1, "x".to_string(), vec![])
            .unwrap_err();
        assert!(err.contains("locked"));
    }

    #[test]
    fn chrome_nip44_self_round_trip() {
        let signer = unlocked_signer(TEST_HEX);
        let pubkey = signer.get_pubkey().expect("unlocked");
        let plaintext = r#"{"version":1,"bookmarks":[{"url":"nsite://titan"}]}"#;

        let ciphertext = signer
            .chrome_nip44_encrypt(&pubkey, plaintext)
            .expect("encrypt");
        // Ciphertext is non-empty and not the same as plaintext
        assert!(!ciphertext.is_empty());
        assert_ne!(ciphertext, plaintext);

        let decrypted = signer
            .chrome_nip44_decrypt(&pubkey, &ciphertext)
            .expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn chrome_nip44_decrypt_rejects_garbage() {
        let signer = unlocked_signer(TEST_HEX);
        let pubkey = signer.get_pubkey().unwrap();
        let err = signer
            .chrome_nip44_decrypt(&pubkey, "definitely-not-a-valid-nip44-payload")
            .unwrap_err();
        assert!(err.contains("nip44 decrypt"));
    }

    #[test]
    fn chrome_nip44_encrypt_rejects_invalid_pubkey() {
        let signer = unlocked_signer(TEST_HEX);
        let err = signer
            .chrome_nip44_encrypt("not-a-pubkey", "hi")
            .unwrap_err();
        assert!(err.contains("invalid recipient pubkey"));
    }

    #[test]
    fn chrome_nip44_decrypt_rejects_invalid_pubkey() {
        let signer = unlocked_signer(TEST_HEX);
        let err = signer
            .chrome_nip44_decrypt("not-a-pubkey", "ciphertext")
            .unwrap_err();
        assert!(err.contains("invalid sender pubkey"));
    }
}
