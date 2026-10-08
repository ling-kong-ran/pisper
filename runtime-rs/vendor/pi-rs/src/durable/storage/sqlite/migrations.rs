//! Port of `src/storage/sqlite/migrations.ts`: the immutable, ordered schema
//! history and the atomic migration runner. The statement texts are byte-
//! identical to the upstream array (the oracle pins them), including the
//! `next_id is TEXT` note: node:sqlite rejects INTEGER results outside
//! JavaScript's safe integer range, and the port keeps the same layout.

use super::super::StorageError;
use super::database::SqliteDatabase;

/// One schema migration (`migrations.ts` `SqliteMigration`).
pub struct SqliteMigration {
    pub version: i64,
    pub statements: &'static [&'static str],
}

const INITIAL_SCHEMA: &[&str] = &[
    "CREATE TABLE durable_metadata (
		singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
		next_id TEXT NOT NULL,
		next_seq INTEGER NOT NULL
	) STRICT",
    "INSERT INTO durable_metadata (singleton, next_id, next_seq) VALUES (1, '2', 1)",
    "CREATE TABLE record_ids (
		id INTEGER PRIMARY KEY,
		record_type TEXT NOT NULL CHECK (record_type IN ('conversation', 'entry', 'task', 'submission', 'document'))
	) STRICT",
    "CREATE TABLE conversations (
		id INTEGER PRIMARY KEY,
		owner_conversation_id INTEGER,
		owner_task_id INTEGER,
		record TEXT NOT NULL CHECK (json_valid(record))
	) STRICT",
    "CREATE INDEX conversations_by_owner_conversation ON conversations (owner_conversation_id, id)",
    "CREATE INDEX conversations_by_owner_task ON conversations (owner_task_id, id)",
    "CREATE TABLE entries (
		id INTEGER PRIMARY KEY,
		conversation_id INTEGER NOT NULL,
		head INTEGER,
		commit_seq INTEGER NOT NULL,
		record TEXT NOT NULL CHECK (json_valid(record))
	) STRICT",
    "CREATE INDEX entries_by_conversation ON entries (conversation_id, id DESC)",
    "CREATE INDEX entry_heads_by_conversation ON entries (conversation_id, id DESC) WHERE head IS NOT NULL",
    "CREATE TABLE tasks (
		id INTEGER PRIMARY KEY,
		conversation_id INTEGER NOT NULL,
		kind TEXT NOT NULL,
		status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'waiting', 'completing', 'terminal')),
		abort_requested INTEGER NOT NULL CHECK (abort_requested IN (0, 1)),
		background INTEGER NOT NULL CHECK (background IN (0, 1)),
		record TEXT NOT NULL CHECK (json_valid(record))
	) STRICT",
    "CREATE INDEX tasks_by_status ON tasks (status, id)",
    "CREATE INDEX tasks_by_conversation ON tasks (conversation_id, id)",
    "CREATE INDEX tasks_by_kind ON tasks (kind, id)",
    "CREATE INDEX tasks_by_abort_requested ON tasks (abort_requested, id)",
    "CREATE INDEX tasks_by_background ON tasks (background, id)",
    "CREATE TABLE submissions (
		id INTEGER PRIMARY KEY,
		conversation_id INTEGER NOT NULL,
		request_id TEXT,
		status TEXT NOT NULL CHECK (status IN ('queued', 'placed', 'done', 'unanswered')),
		record TEXT NOT NULL CHECK (json_valid(record))
	) STRICT",
    "CREATE INDEX submissions_by_request ON submissions (conversation_id, request_id)",
    "CREATE INDEX submissions_by_conversation ON submissions (conversation_id, id)",
    "CREATE INDEX submissions_by_status ON submissions (status, id)",
    "CREATE TABLE documents (
		id INTEGER PRIMARY KEY,
		kind TEXT NOT NULL,
		family INTEGER NOT NULL CHECK (family IN (0, 1)),
		key_value TEXT NOT NULL,
		scope_kind TEXT NOT NULL CHECK (scope_kind IN ('session', 'conversation', 'task')),
		owner_id INTEGER NOT NULL,
		created_at INTEGER NOT NULL,
		retired_at INTEGER,
		record TEXT NOT NULL CHECK (json_valid(record))
	) STRICT",
    "CREATE INDEX documents_by_address
		ON documents (kind, scope_kind, owner_id, family, key_value, created_at DESC, retired_at)",
    "CREATE INDEX documents_by_scope ON documents (scope_kind, owner_id, id)",
    "CREATE INDEX documents_by_scope_kind ON documents (scope_kind, owner_id, kind, id)",
    "CREATE TABLE document_revisions (
		document_id INTEGER NOT NULL,
		seq INTEGER NOT NULL,
		kind TEXT NOT NULL CHECK (kind IN ('base', 'delta')),
		version INTEGER NOT NULL,
		content TEXT NOT NULL CHECK (json_valid(content)),
		PRIMARY KEY (document_id, seq)
	) STRICT",
    "CREATE INDEX document_revisions_by_kind ON document_revisions (document_id, kind, seq DESC)",
];

/// Immutable, ordered schema history. Append new migrations after the initial
/// schema ships (`SQLITE_MIGRATIONS`).
pub const SQLITE_MIGRATIONS: &[SqliteMigration] = &[SqliteMigration {
    version: 1,
    statements: INITIAL_SCHEMA,
}];

/// The newest migration's version (`CURRENT_SQLITE_SCHEMA_VERSION`).
pub fn current_sqlite_schema_version() -> i64 {
    SQLITE_MIGRATIONS
        .last()
        .map_or(0, |migration| migration.version)
}

/// Apply all pending schema migrations atomically (`applySqliteMigrations`).
pub fn apply_sqlite_migrations(database: &dyn SqliteDatabase) -> Result<(), StorageError> {
    for (index, migration) in SQLITE_MIGRATIONS.iter().enumerate() {
        if migration.version != index as i64 + 1 {
            return Err(StorageError::generic(
                "Durable SQLite migrations must have contiguous versions starting at 1",
            ));
        }
    }
    database.transaction(&mut || {
        database.exec(
            "CREATE TABLE IF NOT EXISTS durable_schema (
			singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
			version INTEGER NOT NULL CHECK (version >= 0)
		) STRICT",
        )?;
        database.run(
            "INSERT OR IGNORE INTO durable_schema (singleton, version) VALUES (1, 0)",
            &[],
        )?;
        let row = database.select_row(
            "SELECT version FROM durable_schema WHERE singleton = 1",
            &[],
        )?;
        let Some(row) = row else {
            return Err(StorageError::generic(
                "Durable SQLite schema metadata is missing",
            ));
        };
        let current_version = current_sqlite_schema_version();
        let stored_version = row.i64("version").unwrap_or(0);
        if stored_version > current_version {
            return Err(StorageError::generic(format!(
                "Durable SQLite schema version {} is newer than supported version {}",
                stored_version, current_version
            )));
        }
        for migration in SQLITE_MIGRATIONS {
            if migration.version <= stored_version {
                continue;
            }
            for statement_text in migration.statements {
                database.exec(statement_text)?;
            }
            database.run(
                "UPDATE durable_schema SET version = ? WHERE singleton = 1",
                &[migration.version.into()],
            )?;
        }
        Ok(())
    })
}
