//! Port of `src/testing/types.ts`: the runner-independent contract of the
//! storage conformance suite.
//!
//! Divergences, continuing the module-wide numbering:
//!
//! - **D35 (conformance error channel).** Upstream case bodies are `async`
//!   and both assertions and storage operations throw; the port threads
//!   [`ConformanceResult`], where a failing assertion or an unexpected storage
//!   failure resolves to a [`ConformanceFailure`] whose `Display` carries the
//!   failure text.
//! - **D36 (assertion facade collapse).** Upstream wraps the assertions in an
//!   `expect(actual).toBe(expected)` facade inside `storage-conformance.ts`;
//!   the port's case bodies call the [`StorageConformanceAssertions`] contract
//!   directly with the same call sequence and messages, since the facade only
//!   adapts Vitest-style matchers.

use std::sync::Arc;

use serde_json::Value;

use crate::durable::storage::{Storage, StorageError};

/// Outcome of one conformance case body (D35).
pub type ConformanceResult = Result<(), ConformanceFailure>;

/// A conformance case failure; `Display` is the failure text (an assertion
/// description or a storage error message).
#[derive(Debug, Clone)]
pub struct ConformanceFailure {
    pub message: String,
}

impl ConformanceFailure {
    pub fn new(message: impl Into<String>) -> Self {
        ConformanceFailure {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ConformanceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConformanceFailure {}

impl From<StorageError> for ConformanceFailure {
    fn from(error: StorageError) -> Self {
        ConformanceFailure::new(error.to_string())
    }
}

/// One deferred storage operation handed to
/// [`StorageConformanceAssertions::rejects`] (upstream passes the operation's
/// promise). Owns its storage handle, so it is `'static`.
pub type StorageOperation = Box<dyn FnOnce() -> Result<(), StorageError> + Send>;

/// One conformance case body: the `use` callback of
/// `StorageConformanceProvider` (upstream passes the storage by reference;
/// the port passes the owned handle).
pub type ConformanceTest = Arc<dyn Fn(Arc<dyn Storage>) -> ConformanceResult + Send + Sync>;

/// The assertion vocabulary the suite runs through
/// (`testing/types.ts` `StorageConformanceAssertions`). Values cross the
/// contract as JSON (the port's record wire forms); `strictEqual` is JS
/// SameValue over JSON scalars, `deepEqual` structural equality,
/// `partialDeepEqual` subset matching.
pub trait StorageConformanceAssertions: Send + Sync {
    /// `ok(value, message?)`: assert truthiness.
    fn ok(&self, value: bool, message: &str) -> ConformanceResult;
    /// `strictEqual(actual, expected)`.
    fn strict_equal(&self, actual: &Value, expected: &Value) -> ConformanceResult;
    /// `deepEqual(actual, expected)`.
    fn deep_equal(&self, actual: &Value, expected: &Value) -> ConformanceResult;
    /// `partialDeepEqual(actual, expected)`.
    fn partial_deep_equal(&self, actual: &Value, expected: &Value) -> ConformanceResult;
    /// `greaterThan(actual, expected)`.
    fn greater_than(&self, actual: i64, expected: i64) -> ConformanceResult;
    /// `rejects(operation, messageIncludes)`: run the operation and require a
    /// rejection whose message includes the text.
    fn rejects(&self, operation: StorageOperation, message_includes: &str) -> ConformanceResult;
}

/// `(use) => Promise<void>`: supply a fresh [`Storage`], call the callback
/// exactly once, and propagate its outcome (`testing/types.ts`
/// `StorageConformanceProvider`).
pub type StorageConformanceProvider =
    Arc<dyn Fn(ConformanceTest) -> ConformanceResult + Send + Sync>;

/// Options of `createStorageConformance` (`testing/types.ts`
/// `StorageConformanceOptions`).
#[derive(Clone)]
pub struct StorageConformanceOptions {
    pub assertions: Arc<dyn StorageConformanceAssertions>,
    pub with_storage: StorageConformanceProvider,
}

/// One runner-independent case (`testing/types.ts`
/// `StorageConformanceCase`). Upstream closes `run` over
/// `options.withStorage`; the port passes the options to
/// [`StorageConformanceCase::run`] instead.
#[derive(Clone)]
pub struct StorageConformanceCase {
    /// The suite-visible case name.
    pub name: String,
    /// The case body.
    pub test: ConformanceTest,
}

impl StorageConformanceCase {
    /// `run()`: hand the body to `withStorage`.
    pub fn run(&self, options: &StorageConformanceOptions) -> ConformanceResult {
        (options.with_storage)(self.test.clone())
    }
}
