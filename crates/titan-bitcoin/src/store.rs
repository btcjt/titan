//! SQLite store for the Bitcoin name index.
//!
//! Stores name → pubkey mappings and block sync state.

// TODO: Phase 2 — implement SQLite operations
// - init_db()
// - get_name(name) -> Option<NameRecord>
// - insert_name(name, pubkey, txid, block_height, owner_address)
// - transfer_name(name, new_pubkey, txid, block_height, new_owner_address)
// - get_sync_state() -> (block_height, block_hash)
// - set_sync_state(block_height, block_hash)
