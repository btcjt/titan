//! SQLite store for the Bitcoin name index.
//!
//! UTXO-based ownership model: each name is controlled by a specific
//! transaction output. To transfer, you must spend that UTXO.

use rusqlite::{params, Connection, OptionalExtension};
use titan_types::TitanName;

/// A registered name record from the index.
#[derive(Debug, Clone)]
pub struct NameRecord {
    /// The validated Titan name.
    pub name: TitanName,
    /// 32-byte x-only Schnorr pubkey (hex-encoded in DB).
    pub pubkey: [u8; 32],
    /// The UTXO that controls this name (txid:vout).
    pub owner_txid: String,
    pub owner_vout: u32,
    /// Transaction ID where this name was registered or last transferred.
    pub txid: String,
    /// Block height of the registration or last transfer.
    pub block_height: u64,
}

/// Current blockchain sync position.
#[derive(Debug, Clone)]
pub struct SyncState {
    pub block_height: u64,
    pub block_hash: String,
}

/// SQLite-backed name index.
pub struct NameStore {
    conn: Connection,
}

impl NameStore {
    /// Open (or create) the name index database at the given path.
    pub fn open(path: &str) -> Result<Self, StoreError> {
        let conn = Connection::open(path).map_err(StoreError::Sqlite)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Open an in-memory database (useful for tests).
    pub fn open_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory().map_err(StoreError::Sqlite)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Create tables if they don't exist.
    fn init_schema(&self) -> Result<(), StoreError> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS names (
                    name          TEXT PRIMARY KEY NOT NULL,
                    pubkey        TEXT NOT NULL,
                    owner_txid    TEXT NOT NULL,
                    owner_vout    INTEGER NOT NULL,
                    txid          TEXT NOT NULL,
                    block_height  INTEGER NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_names_block_height
                    ON names(block_height);

                CREATE INDEX IF NOT EXISTS idx_names_owner_utxo
                    ON names(owner_txid, owner_vout);

                CREATE TABLE IF NOT EXISTS sync_state (
                    id            INTEGER PRIMARY KEY CHECK (id = 1),
                    block_height  INTEGER NOT NULL,
                    block_hash    TEXT NOT NULL
                );
                ",
            )
            .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    /// Look up a name record. Returns `None` if the name is not registered.
    pub fn get_name(&self, name: &TitanName) -> Result<Option<NameRecord>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, pubkey, owner_txid, owner_vout, txid, block_height
                 FROM names WHERE name = ?1",
            )
            .map_err(StoreError::Sqlite)?;

        stmt.query_row(params![name.as_str()], |row| {
            let name_str: String = row.get(0)?;
            let pubkey_hex: String = row.get(1)?;
            let owner_txid: String = row.get(2)?;
            let owner_vout: u32 = row.get(3)?;
            let txid: String = row.get(4)?;
            let block_height: u64 = row.get(5)?;

            Ok((name_str, pubkey_hex, owner_txid, owner_vout, txid, block_height))
        })
        .optional()
        .map_err(StoreError::Sqlite)?
        .map(|(name_str, pubkey_hex, owner_txid, owner_vout, txid, block_height)| {
            let name = TitanName::new(&name_str).map_err(|e| StoreError::Data(e.to_string()))?;
            let pubkey = hex_to_pubkey(&pubkey_hex)?;
            Ok(NameRecord {
                name,
                pubkey,
                owner_txid,
                owner_vout,
                txid,
                block_height,
            })
        })
        .transpose()
    }

    /// Register a new name. Fails if the name already exists (first-in-chain wins).
    ///
    /// `owner_txid` and `owner_vout` identify the UTXO that controls this name
    /// (the first non-OP_RETURN output of the registration transaction).
    pub fn insert_name(
        &self,
        name: &TitanName,
        pubkey: &[u8; 32],
        owner_txid: &str,
        owner_vout: u32,
        txid: &str,
        block_height: u64,
    ) -> Result<bool, StoreError> {
        let rows = self
            .conn
            .execute(
                "INSERT OR IGNORE INTO names (name, pubkey, owner_txid, owner_vout, txid, block_height)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    name.as_str(),
                    hex::encode(pubkey),
                    owner_txid,
                    owner_vout,
                    txid,
                    block_height,
                ],
            )
            .map_err(StoreError::Sqlite)?;
        Ok(rows > 0)
    }

    /// Transfer a name: update pubkey and ownership UTXO.
    pub fn transfer_name(
        &self,
        name: &TitanName,
        new_pubkey: &[u8; 32],
        new_owner_txid: &str,
        new_owner_vout: u32,
        txid: &str,
        block_height: u64,
    ) -> Result<bool, StoreError> {
        let rows = self
            .conn
            .execute(
                "UPDATE names SET pubkey = ?1, owner_txid = ?2, owner_vout = ?3, txid = ?4, block_height = ?5
                 WHERE name = ?6",
                params![
                    hex::encode(new_pubkey),
                    new_owner_txid,
                    new_owner_vout,
                    txid,
                    block_height,
                    name.as_str(),
                ],
            )
            .map_err(StoreError::Sqlite)?;
        Ok(rows > 0)
    }

    /// Get the ownership UTXO for a name (needed to verify transfer authorization).
    pub fn get_owner_utxo(&self, name: &TitanName) -> Result<Option<(String, u32)>, StoreError> {
        self.conn
            .query_row(
                "SELECT owner_txid, owner_vout FROM names WHERE name = ?1",
                params![name.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    /// Find a name by its ownership UTXO (used during transfer verification).
    pub fn find_by_utxo(&self, txid: &str, vout: u32) -> Result<Option<NameRecord>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, pubkey, owner_txid, owner_vout, txid, block_height
                 FROM names WHERE owner_txid = ?1 AND owner_vout = ?2",
            )
            .map_err(StoreError::Sqlite)?;

        stmt.query_row(params![txid, vout], |row| {
            let name_str: String = row.get(0)?;
            let pubkey_hex: String = row.get(1)?;
            let owner_txid: String = row.get(2)?;
            let owner_vout: u32 = row.get(3)?;
            let txid: String = row.get(4)?;
            let block_height: u64 = row.get(5)?;
            Ok((name_str, pubkey_hex, owner_txid, owner_vout, txid, block_height))
        })
        .optional()
        .map_err(StoreError::Sqlite)?
        .map(|(name_str, pubkey_hex, owner_txid, owner_vout, txid, block_height)| {
            let name = TitanName::new(&name_str).map_err(|e| StoreError::Data(e.to_string()))?;
            let pubkey = hex_to_pubkey(&pubkey_hex)?;
            Ok(NameRecord {
                name,
                pubkey,
                owner_txid,
                owner_vout,
                txid,
                block_height,
            })
        })
        .transpose()
    }

    /// Get the current sync position. Returns `None` if never synced.
    pub fn get_sync_state(&self) -> Result<Option<SyncState>, StoreError> {
        self.conn
            .query_row(
                "SELECT block_height, block_hash FROM sync_state WHERE id = 1",
                [],
                |row| {
                    Ok(SyncState {
                        block_height: row.get(0)?,
                        block_hash: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    /// Update the sync position (upsert).
    pub fn set_sync_state(&self, height: u64, hash: &str) -> Result<(), StoreError> {
        self.conn
            .execute(
                "INSERT INTO sync_state (id, block_height, block_hash)
                 VALUES (1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET block_height = ?1, block_hash = ?2",
                params![height, hash],
            )
            .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    /// Roll back all registrations at or above the given block height.
    pub fn rollback_to(&self, height: u64) -> Result<u64, StoreError> {
        let deleted = self
            .conn
            .execute(
                "DELETE FROM names WHERE block_height > ?1",
                params![height],
            )
            .map_err(StoreError::Sqlite)?;
        Ok(deleted as u64)
    }

    /// Resolve a name directly to its pubkey.
    pub fn resolve(&self, name: &TitanName) -> Result<Option<[u8; 32]>, StoreError> {
        self.conn
            .query_row(
                "SELECT pubkey FROM names WHERE name = ?1",
                params![name.as_str()],
                |row| {
                    let hex_str: String = row.get(0)?;
                    Ok(hex_str)
                },
            )
            .optional()
            .map_err(StoreError::Sqlite)?
            .map(|hex_str| hex_to_pubkey(&hex_str))
            .transpose()
    }
}

fn hex_to_pubkey(hex_str: &str) -> Result<[u8; 32], StoreError> {
    let bytes = hex::decode(hex_str).map_err(|e| StoreError::Data(e.to_string()))?;
    if bytes.len() != 32 {
        return Err(StoreError::Data(format!(
            "expected 32-byte pubkey, got {}",
            bytes.len()
        )));
    }
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&bytes);
    Ok(pubkey)
}

/// Store-specific errors.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("data error: {0}")]
    Data(String),
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn open_and_init() {
        let store = NameStore::open_memory().unwrap();
        assert!(store.get_sync_state().unwrap().is_none());
    }

    #[test]
    fn insert_and_lookup() {
        let store = NameStore::open_memory().unwrap();
        let name = TitanName::new("westernbtc").unwrap();

        let inserted = store
            .insert_name(&name, &test_pubkey(), "aabb", 1, "aabb", 800_000)
            .unwrap();
        assert!(inserted);

        let record = store.get_name(&name).unwrap().expect("should exist");
        assert_eq!(record.name.as_str(), "westernbtc");
        assert_eq!(record.pubkey, test_pubkey());
        assert_eq!(record.owner_txid, "aabb");
        assert_eq!(record.owner_vout, 1);
        assert_eq!(record.block_height, 800_000);
    }

    #[test]
    fn first_in_chain_wins() {
        let store = NameStore::open_memory().unwrap();
        let name = TitanName::new("westernbtc").unwrap();

        let first = store
            .insert_name(&name, &test_pubkey(), "tx1", 0, "tx1", 800_000)
            .unwrap();
        assert!(first);

        let second = store
            .insert_name(&name, &other_pubkey(), "tx2", 0, "tx2", 800_001)
            .unwrap();
        assert!(!second);

        let record = store.get_name(&name).unwrap().unwrap();
        assert_eq!(record.pubkey, test_pubkey());
    }

    #[test]
    fn transfer() {
        let store = NameStore::open_memory().unwrap();
        let name = TitanName::new("westernbtc").unwrap();

        store
            .insert_name(&name, &test_pubkey(), "tx1", 0, "tx1", 800_000)
            .unwrap();

        let transferred = store
            .transfer_name(&name, &other_pubkey(), "tx2", 1, "tx2", 800_100)
            .unwrap();
        assert!(transferred);

        let record = store.get_name(&name).unwrap().unwrap();
        assert_eq!(record.pubkey, other_pubkey());
        assert_eq!(record.owner_txid, "tx2");
        assert_eq!(record.owner_vout, 1);
    }

    #[test]
    fn transfer_nonexistent() {
        let store = NameStore::open_memory().unwrap();
        let name = TitanName::new("doesnotexist").unwrap();

        let transferred = store
            .transfer_name(&name, &other_pubkey(), "tx2", 0, "tx2", 800_100)
            .unwrap();
        assert!(!transferred);
    }

    #[test]
    fn find_by_utxo() {
        let store = NameStore::open_memory().unwrap();
        let name = TitanName::new("westernbtc").unwrap();

        store
            .insert_name(&name, &test_pubkey(), "tx1", 0, "tx1", 800_000)
            .unwrap();

        let found = store.find_by_utxo("tx1", 0).unwrap().unwrap();
        assert_eq!(found.name.as_str(), "westernbtc");

        assert!(store.find_by_utxo("tx1", 1).unwrap().is_none());
        assert!(store.find_by_utxo("other", 0).unwrap().is_none());
    }

    #[test]
    fn resolve_pubkey() {
        let store = NameStore::open_memory().unwrap();
        let name = TitanName::new("westernbtc").unwrap();

        assert!(store.resolve(&name).unwrap().is_none());

        store
            .insert_name(&name, &test_pubkey(), "tx1", 0, "tx1", 800_000)
            .unwrap();

        let pk = store.resolve(&name).unwrap().expect("should resolve");
        assert_eq!(pk, test_pubkey());
    }

    #[test]
    fn sync_state_roundtrip() {
        let store = NameStore::open_memory().unwrap();

        assert!(store.get_sync_state().unwrap().is_none());

        store.set_sync_state(800_000, "00000000abc").unwrap();
        let state = store.get_sync_state().unwrap().unwrap();
        assert_eq!(state.block_height, 800_000);
        assert_eq!(state.block_hash, "00000000abc");
    }

    #[test]
    fn rollback() {
        let store = NameStore::open_memory().unwrap();

        store
            .insert_name(&TitanName::new("alpha").unwrap(), &test_pubkey(), "tx1", 0, "tx1", 800_000)
            .unwrap();
        store
            .insert_name(&TitanName::new("beta").unwrap(), &test_pubkey(), "tx2", 0, "tx2", 800_001)
            .unwrap();
        store
            .insert_name(&TitanName::new("gamma").unwrap(), &test_pubkey(), "tx3", 0, "tx3", 800_002)
            .unwrap();

        let deleted = store.rollback_to(800_000).unwrap();
        assert_eq!(deleted, 2);

        assert!(store.get_name(&TitanName::new("alpha").unwrap()).unwrap().is_some());
        assert!(store.get_name(&TitanName::new("beta").unwrap()).unwrap().is_none());
        assert!(store.get_name(&TitanName::new("gamma").unwrap()).unwrap().is_none());
    }

    #[test]
    fn get_owner_utxo() {
        let store = NameStore::open_memory().unwrap();
        let name = TitanName::new("westernbtc").unwrap();

        assert!(store.get_owner_utxo(&name).unwrap().is_none());

        store
            .insert_name(&name, &test_pubkey(), "tx1", 2, "tx1", 800_000)
            .unwrap();

        let (txid, vout) = store.get_owner_utxo(&name).unwrap().unwrap();
        assert_eq!(txid, "tx1");
        assert_eq!(vout, 2);
    }
}
