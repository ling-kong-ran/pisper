//! Port of `src/storage/sqlite/` — the SQLite storage backend over the
//! engine-agnostic facade (`index.ts` re-exports).
//!
//! - [`database`] — `database.ts`: the facade types (`SqliteValue`,
//!   `SqliteStatement`, `SqliteDatabase`).
//! - [`migrations`] — `migrations.ts`: the schema history and the atomic
//!   migration runner.
//! - [`node`] — `node.ts`, adapted from `node:sqlite` to `rusqlite`
//!   (`=0.32.1`, `bundled` feature; no system SQLite dependency).
//! - [`storage`] — `storage.ts`: [`SqliteStorage`], the portable storage
//!   core.
//!
//! Disclosed divergences: D31 (scoped statements, in [`database`]) and the
//! per-file notes in [`node`] / [`storage`].

pub mod database;
pub mod migrations;
pub mod node;
pub mod storage;

pub use database::{SqliteDatabase, SqliteRow, SqliteStatement, SqliteValue};
pub use migrations::{
    apply_sqlite_migrations, current_sqlite_schema_version, SqliteMigration, SQLITE_MIGRATIONS,
};
pub use node::{
    open_rusqlite_database, open_rusqlite_storage, RusqliteDatabase, RusqliteStorageOptions,
    DEFAULT_BUSY_TIMEOUT_MS, DEFAULT_WAL_AUTO_CHECKPOINT_PAGES,
};
pub use storage::SqliteStorage;

#[cfg(test)]
mod smoke_tests {
    use super::{open_rusqlite_storage, RusqliteStorageOptions, SqliteStorage};
    use crate::agent_core::chord_support::context::Context;
    use crate::durable::storage::Storage;
    use crate::durable::types::{ConversationRecord, StorageWrite};

    fn background() -> Context {
        Context::background()
    }

    /// Opens over the bundled engine, commits, and reads back through the
    /// record columns and the strict tables.
    #[test]
    fn opens_commits_and_reads_back() {
        let storage = open_rusqlite_storage(":memory:", &RusqliteStorageOptions::default())
            .expect("opens over the bundled engine");
        let id = storage.mint_id().expect("mints 2");
        assert_eq!(id, 2);
        let record = ConversationRecord {
            id,
            parent: None,
            owner: None,
        };
        let seq = storage
            .commit(
                &[StorageWrite::Conversation {
                    value: record.clone(),
                }],
                &background(),
            )
            .expect("commit");
        assert_eq!(seq, 1);
        assert_eq!(
            storage.conversation(id, &background()).expect("read"),
            Some(record)
        );
    }

    /// `SqliteStorage::open` over an explicit facade, and schema version
    /// bookkeeping.
    #[test]
    fn schema_metadata_and_close() {
        let connection = rusqlite::Connection::open_in_memory().expect("connection");
        let database = super::RusqliteDatabase::new(connection);
        let storage = SqliteStorage::open(database).expect("open");
        assert_eq!(super::current_sqlite_schema_version(), 1);
        storage.close(&background()).expect("close");
        assert!(storage.close(&background()).is_ok());
    }
}
