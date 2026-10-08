//! Port of `src/storage/sqlite/node.ts`: the engine adapter behind the
//! [`SqliteDatabase`](super::database::SqliteDatabase) facade. Upstream
//! adapts Node's built-in `node:sqlite`; the port adapts `rusqlite` with the
//! bundled SQLite (`rusqlite = "=0.32.1"`, `bundled` feature — no system
//! SQLite dependency), keeping the same connection settings and close
//! checkpoint semantics.

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;

use rusqlite::types::Value as RusqliteValue;
use rusqlite::OpenFlags;

use super::super::StorageError;
use super::database::{SqliteDatabase, SqliteRow, SqliteStatement, SqliteValue};
use super::storage::SqliteStorage;

/// Node SQLite connection settings for a durable storage file
/// (`NodeSqliteStorageOptions`).
#[derive(Debug, Clone, Default)]
pub struct RusqliteStorageOptions {
    /// SQLite WAL auto-checkpoint threshold. SQLite and this adapter default
    /// to 1,000 pages; 0 disables it.
    pub wal_auto_checkpoint_pages: Option<i64>,
    /// Time SQLite waits for a competing file lock. SQLite defaults to 0;
    /// this adapter defaults to 5,000 ms.
    pub busy_timeout_ms: Option<u64>,
}

/// Defaults (`node.ts:16-17`).
pub const DEFAULT_WAL_AUTO_CHECKPOINT_PAGES: i64 = 1_000;
pub const DEFAULT_BUSY_TIMEOUT_MS: u64 = 5_000;

fn to_rusqlite(value: &SqliteValue) -> RusqliteValue {
    match value {
        SqliteValue::Null => RusqliteValue::Null,
        SqliteValue::Integer(value) => RusqliteValue::Integer(*value),
        SqliteValue::Real(value) => RusqliteValue::Real(*value),
        SqliteValue::Text(value) => RusqliteValue::Text(value.clone()),
        SqliteValue::Blob(value) => RusqliteValue::Blob(value.clone()),
    }
}

fn from_rusqlite(value: RusqliteValue) -> SqliteValue {
    match value {
        RusqliteValue::Null => SqliteValue::Null,
        RusqliteValue::Integer(value) => SqliteValue::Integer(value),
        RusqliteValue::Real(value) => SqliteValue::Real(value),
        RusqliteValue::Text(value) => SqliteValue::Text(value),
        RusqliteValue::Blob(value) => SqliteValue::Blob(value),
    }
}

fn facade_error(error: impl std::fmt::Display) -> StorageError {
    StorageError::generic(error.to_string())
}

fn read_row(row: &rusqlite::Row<'_>, columns: &[String]) -> Result<SqliteRow, rusqlite::Error> {
    let mut pairs = Vec::with_capacity(columns.len());
    for (index, name) in columns.iter().enumerate() {
        let value: RusqliteValue = row.get(index)?;
        pairs.push((name.clone(), from_rusqlite(value)));
    }
    Ok(SqliteRow::from_pairs(pairs))
}

/// One cached-preparation statement handle (`NodeSqliteStatement`).
struct RusqliteStatement<'conn, 'statement> {
    statement: &'statement mut rusqlite::Statement<'conn>,
}

impl SqliteStatement for RusqliteStatement<'_, '_> {
    fn run(&mut self, params: &[SqliteValue]) -> Result<(), StorageError> {
        // `StatementSync.run` discards the change count.
        let _ = self
            .statement
            .execute(rusqlite::params_from_iter(params.iter().map(to_rusqlite)))
            .map_err(facade_error)?;
        Ok(())
    }

    fn get(&mut self, params: &[SqliteValue]) -> Result<Option<SqliteRow>, StorageError> {
        let columns: Vec<String> = self
            .statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut rows = self
            .statement
            .query(rusqlite::params_from_iter(params.iter().map(to_rusqlite)))
            .map_err(facade_error)?;
        match rows.next() {
            Ok(Some(row)) => Ok(Some(read_row(row, &columns).map_err(facade_error)?)),
            Ok(None) => Ok(None),
            Err(error) => Err(facade_error(error)),
        }
    }

    fn all(&mut self, params: &[SqliteValue]) -> Result<Vec<SqliteRow>, StorageError> {
        let columns: Vec<String> = self
            .statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut rows = self
            .statement
            .query(rusqlite::params_from_iter(params.iter().map(to_rusqlite)))
            .map_err(facade_error)?;
        let mut result = Vec::new();
        loop {
            match rows.next() {
                Ok(Some(row)) => result.push(read_row(row, &columns).map_err(facade_error)?),
                Ok(None) => break,
                Err(error) => return Err(facade_error(error)),
            }
        }
        Ok(result)
    }
}

/// `SqliteDatabase` adapter backed by `rusqlite` (upstream: `node:sqlite`)
/// (`NodeSqliteDatabase`). Every facade call serializes on the connection so
/// the facade stays shareable through `&self` (rusqlite connections are `Send`
/// but not `Sync`), matching the single-threaded event-loop serialization
/// upstream relies on.
///
/// The serialization is a per-thread re-entrant gate rather than a plain
/// mutex over the connection: the portable core runs nested facade calls
/// *inside* transaction callbacks (`applySqliteMigrations` and
/// `SqliteStorage.commit` execute `exec`/`run`/`select_*` against the same
/// facade while `transaction` is on the stack), and a non-re-entrant lock
/// would self-deadlock there. Upstream never sees this because the event loop
/// serializes those calls for free. While a transaction is in flight the gate
/// is held for its whole duration, so other threads still block (and `close`
/// waits out an open transaction instead of racing it); nested same-thread
/// calls pass through and only take the short-lived connection mutex per
/// statement.
pub struct RusqliteDatabase {
    connection: Mutex<rusqlite::Connection>,
    closed: Mutex<bool>,
    gate: ConnectionGate,
}

/// The per-thread re-entrant serialization gate (`RusqliteDatabase` doc
/// comment): one owner thread at a time; the owner re-enters freely.
struct ConnectionGate {
    owner: Mutex<Option<ThreadId>>,
    available: Condvar,
}

/// One hold on the [`ConnectionGate`]; releases the gate on drop when it
/// actually acquired it (a re-entrant lease holds nothing).
struct GateLease<'a> {
    gate: &'a ConnectionGate,
    acquired: bool,
}

impl Drop for GateLease<'_> {
    fn drop(&mut self) {
        if !self.acquired {
            return;
        }
        let mut owner = self
            .gate
            .owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *owner = None;
        self.gate.available.notify_all();
    }
}

impl ConnectionGate {
    fn new() -> ConnectionGate {
        ConnectionGate {
            owner: Mutex::new(None),
            available: Condvar::new(),
        }
    }

    /// Acquire the gate for this call: passes straight through (without
    /// owning it) when the current thread already owns it — a nested call
    /// from inside a transaction callback — otherwise blocks until the gate
    /// is free and holds it until the lease drops.
    fn acquire(&self) -> GateLease<'_> {
        let current = std::thread::current().id();
        let mut owner = self
            .owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *owner == Some(current) {
            return GateLease {
                gate: self,
                acquired: false,
            };
        }
        while owner.is_some() {
            owner = self
                .available
                .wait(owner)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *owner = Some(current);
        GateLease {
            gate: self,
            acquired: true,
        }
    }
}

impl RusqliteDatabase {
    /// Wrap an existing connection (`new NodeSqliteDatabase(database)`).
    pub fn new(connection: rusqlite::Connection) -> Arc<RusqliteDatabase> {
        Arc::new(RusqliteDatabase {
            connection: Mutex::new(connection),
            closed: Mutex::new(false),
            gate: ConnectionGate::new(),
        })
    }

    fn lock_connection(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, StorageError> {
        self.connection
            .lock()
            .map_err(|poisoned| StorageError::generic(poisoned.to_string()))
    }

    /// `prepare(sql)` over the per-SQL statement cache (D31): prepare once,
    /// then hand the caller the [`SqliteStatement`] surface.
    fn prepared<R>(
        &self,
        sql: &str,
        use_fn: impl FnOnce(&mut dyn SqliteStatement) -> Result<R, StorageError>,
    ) -> Result<R, StorageError> {
        let _lease = self.gate.acquire();
        let connection = self.lock_connection()?;
        let mut statement = connection.prepare_cached(sql).map_err(facade_error)?;
        use_fn(&mut RusqliteStatement {
            statement: &mut statement,
        })
    }
}

impl SqliteDatabase for RusqliteDatabase {
    fn exec(&self, sql: &str) -> Result<(), StorageError> {
        let _lease = self.gate.acquire();
        let connection = self.lock_connection()?;
        connection.execute_batch(sql).map_err(facade_error)
    }

    fn run(&self, sql: &str, params: &[SqliteValue]) -> Result<(), StorageError> {
        self.prepared(sql, |statement| statement.run(params))
    }

    fn select_row(
        &self,
        sql: &str,
        params: &[SqliteValue],
    ) -> Result<Option<SqliteRow>, StorageError> {
        self.prepared(sql, |statement| statement.get(params))
    }

    fn select_all(
        &self,
        sql: &str,
        params: &[SqliteValue],
    ) -> Result<Vec<SqliteRow>, StorageError> {
        self.prepared(sql, |statement| statement.all(params))
    }

    fn transaction(
        &self,
        callback: &mut dyn FnMut() -> Result<(), StorageError>,
    ) -> Result<(), StorageError> {
        // The gate spans the whole transaction so other threads cannot interleave
        // statements into it; this thread's own nested facade calls re-enter the
        // gate freely, so the connection mutex must stay free across the callback.
        let _lease = self.gate.acquire();
        {
            let connection = self.lock_connection()?;
            connection
                .execute_batch("BEGIN IMMEDIATE")
                .map_err(facade_error)?;
        }
        let result = callback();
        let connection = self.lock_connection()?;
        match result {
            Ok(()) => connection.execute_batch("COMMIT").map_err(facade_error),
            Err(error) => {
                if let Err(rollback_error) = connection.execute_batch("ROLLBACK") {
                    // The callback error must not masquerade as a guaranteed
                    // rollback (`AggregateError` upstream).
                    return Err(StorageError::generic(format!(
                        "SQLite transaction failed and rollback failed: {}; {}",
                        error, rollback_error
                    )));
                }
                Err(error)
            }
        }
    }

    fn close(&self) -> Result<(), StorageError> {
        {
            let mut closed = self
                .closed
                .lock()
                .map_err(|poisoned| StorageError::generic(poisoned.to_string()))?;
            if *closed {
                return Ok(());
            }
            *closed = true;
        }
        // Waits out an open transaction on another thread instead of racing it
        // (`close` during a transaction is a caller contract violation upstream;
        // serializing keeps it orderly here).
        let _lease = self.gate.acquire();
        let connection = self.lock_connection()?;
        // `PRAGMA wal_checkpoint(TRUNCATE)` in `finally`, then
        // `this.database.close()` (rusqlite closes on drop).
        let checkpoint = connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
        drop(connection);
        checkpoint.map_err(facade_error)
    }
}

/// Open and configure a rusqlite-backed SQLite database facade
/// (`openNodeSqliteDatabase`).
pub fn open_rusqlite_database(
    path: &str,
    options: &RusqliteStorageOptions,
) -> Result<Arc<RusqliteDatabase>, StorageError> {
    let checkpoint_pages = options
        .wal_auto_checkpoint_pages
        .unwrap_or(DEFAULT_WAL_AUTO_CHECKPOINT_PAGES);
    let timeout = options
        .busy_timeout_ms
        .unwrap_or(DEFAULT_BUSY_TIMEOUT_MS)
        .min(i64::MAX as u64);
    if path != ":memory:" {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| StorageError::generic(error.to_string()))?;
        }
    }
    let connection = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(facade_error)?;
    connection
        .busy_timeout(std::time::Duration::from_millis(timeout))
        .map_err(facade_error)?;
    let adapter = RusqliteDatabase::new(connection);
    let configure = || -> Result<(), StorageError> {
        adapter.exec("PRAGMA journal_mode = WAL")?;
        adapter.exec("PRAGMA synchronous = NORMAL")?;
        adapter.exec(&format!("PRAGMA wal_autocheckpoint = {checkpoint_pages}"))
    };
    match configure() {
        Ok(()) => Ok(adapter),
        Err(error) => {
            let _ = adapter.close();
            Err(error)
        }
    }
}

/// Open or create file-backed durable storage using the bundled SQLite
/// adapter (`openNodeSqliteStorage`).
pub fn open_rusqlite_storage(
    path: &str,
    options: &RusqliteStorageOptions,
) -> Result<Arc<SqliteStorage>, StorageError> {
    SqliteStorage::open(open_rusqlite_database(path, options)?)
}
