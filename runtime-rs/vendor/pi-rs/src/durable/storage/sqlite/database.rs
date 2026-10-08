//! Port of `src/storage/sqlite/database.ts`: the engine-agnostic SQLite
//! facade consumed by [`super::storage::SqliteStorage`] — portable value,
//! statement, and database types.
//!
//! Queries and transaction callbacks are synchronous so the same storage core
//! can run over any SQLite engine; the rusqlite adapter lives in
//! [`super::node`].
//!
//! Divergences (structural, disclosed):
//! - **D31 (scoped statements).** Upstream `prepare()` returns a long-lived
//!   statement handle the caller rebinds across calls; the port's facade
//!   exposes the four concrete operations (`exec`/`run`/`select_row`/
//!   `select_all`) so the prepared statement is scoped to one call — rusqlite
//!   statements borrow the connection while the adapter drives it from behind
//!   a mutex, and a `dyn` facade cannot carry generic methods. Preparation is
//!   cached per SQL text exactly like the upstream
//!   `StatementCachingDatabase` (delegated to rusqlite's own statement
//!   cache); the [`SqliteStatement`] surface (repeated execution with fresh
//!   bindings) is the adapter's per-call handle.
//! - **D4 (error channel), continuing.** The facade surfaces failures as
//!   [`StorageError`] of [`StorageErrorKind::Generic`]; transaction
//!   rollbacks preserve the callback error, and a failed rollback replaces
//!   it with the upstream aggregate message.

use super::super::StorageError;

/// Values supported by the portable SQLite storage core (`database.ts`
/// `SqliteValue`).
#[derive(Debug, Clone, PartialEq)]
pub enum SqliteValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqliteValue {
    /// The stored integer, for numeric columns.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            SqliteValue::Integer(value) => Some(*value),
            SqliteValue::Real(value) => Some(*value as i64),
            _ => None,
        }
    }

    /// The stored text, for record and metadata columns.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            SqliteValue::Text(value) => Some(value),
            _ => None,
        }
    }
}

impl From<i64> for SqliteValue {
    fn from(value: i64) -> Self {
        SqliteValue::Integer(value)
    }
}

impl From<&str> for SqliteValue {
    fn from(value: &str) -> Self {
        SqliteValue::Text(value.to_owned())
    }
}

impl From<String> for SqliteValue {
    fn from(value: String) -> Self {
        SqliteValue::Text(value)
    }
}

/// One result row addressed by column name (`database.ts` `get<T>` rows).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SqliteRow {
    columns: Vec<(String, SqliteValue)>,
}

impl SqliteRow {
    pub(crate) fn from_pairs(columns: Vec<(String, SqliteValue)>) -> SqliteRow {
        SqliteRow { columns }
    }

    /// The named column's value; `None` when absent.
    pub fn get(&self, column: &str) -> Option<&SqliteValue> {
        self.columns
            .iter()
            .find(|(name, _)| name == column)
            .map(|(_, value)| value)
    }

    /// The named column's text.
    pub fn text(&self, column: &str) -> Option<&str> {
        self.get(column).and_then(SqliteValue::as_text)
    }

    /// The named column's integer.
    pub fn i64(&self, column: &str) -> Option<i64> {
        self.get(column).and_then(SqliteValue::as_i64)
    }
}

/// A prepared synchronous SQLite statement (`database.ts` `SqliteStatement`).
/// Implementations must support repeated execution with new bindings across
/// transactions.
pub trait SqliteStatement {
    fn run(&mut self, params: &[SqliteValue]) -> Result<(), StorageError>;
    /// The first row, or `None` when the statement selects nothing.
    fn get(&mut self, params: &[SqliteValue]) -> Result<Option<SqliteRow>, StorageError>;
    fn all(&mut self, params: &[SqliteValue]) -> Result<Vec<SqliteRow>, StorageError>;
}

/// Minimal database facade required by `SqliteStorage` (`database.ts`
/// `SqliteDatabase`).
///
/// When [`Self::transaction`]'s callback fails, the adapter must roll the
/// transaction back before returning that same error. If rollback fails, it
/// must return a different error (the upstream aggregate) so callers cannot
/// mistake the callback error for a guaranteed rollback. Callers must not
/// close the database while a transaction is running. The callback delivers
/// its result through its captured scope (the upstream callback's `T`); the
/// trait stays object-safe for `dyn` use.
pub trait SqliteDatabase: Send + Sync {
    /// `exec(sql)`.
    fn exec(&self, sql: &str) -> Result<(), StorageError>;
    /// `prepare(sql).run(params)` over the cached preparation (D31).
    fn run(&self, sql: &str, params: &[SqliteValue]) -> Result<(), StorageError>;
    /// `prepare(sql).get(params)` over the cached preparation.
    fn select_row(
        &self,
        sql: &str,
        params: &[SqliteValue],
    ) -> Result<Option<SqliteRow>, StorageError>;
    /// `prepare(sql).all(params)` over the cached preparation.
    fn select_all(&self, sql: &str, params: &[SqliteValue])
        -> Result<Vec<SqliteRow>, StorageError>;
    /// Run `callback` inside one synchronous transaction.
    fn transaction(
        &self,
        callback: &mut dyn FnMut() -> Result<(), StorageError>,
    ) -> Result<(), StorageError>;
    fn close(&self) -> Result<(), StorageError>;
}
