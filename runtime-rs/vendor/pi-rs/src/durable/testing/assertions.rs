//! Port of `src/testing/assertions.ts`: adapts a Vitest/Jest-compatible
//! `expect` function without importing either runner.

use std::sync::Arc;

use serde_json::Value;

use super::types::{ConformanceResult, StorageConformanceAssertions, StorageOperation};

/// The actual passed to a host `expect(actual, message?)` (`assertions.ts`
/// passes any value; `rejects` passes the operation's promise, which the port
/// carries as the deferred operation).
pub enum ExpectActual {
    Value(Value),
    Operation(StorageOperation),
}

/// The matcher surface a host `expect(...)` returns (`assertions.ts`
/// `ExpectResult`); every method reports its outcome through
/// [`super::types::ConformanceResult`].
pub trait ExpectResultLike: Send + Sync {
    fn to_be(&self, expected: &Value) -> ConformanceResult;
    fn to_be_greater_than(&self, expected: i64) -> ConformanceResult;
    fn to_equal(&self, expected: &Value) -> ConformanceResult;
    fn to_match_object(&self, expected: &Value) -> ConformanceResult;
    fn to_be_truthy(&self) -> ConformanceResult;
    /// `rejects.toThrow(expected?)`.
    fn rejects_to_throw(&self, expected: &str) -> ConformanceResult;
}

/// `(actual, message?) => matcher` (`assertions.ts` `ExpectLike`).
pub type ExpectLike =
    Arc<dyn Fn(ExpectActual, Option<&str>) -> Arc<dyn ExpectResultLike> + Send + Sync>;

/// `createExpectAssertions(expect)` (`assertions.ts`).
pub fn create_expect_assertions(expect: ExpectLike) -> impl StorageConformanceAssertions + 'static {
    ExpectAssertions { expect }
}

#[derive(Clone)]
struct ExpectAssertions {
    expect: ExpectLike,
}

impl StorageConformanceAssertions for ExpectAssertions {
    fn ok(&self, value: bool, message: &str) -> ConformanceResult {
        self.expect(ExpectActual::Value(Value::from(value)), Some(message))
            .to_be_truthy()
    }

    fn strict_equal(&self, actual: &Value, expected: &Value) -> ConformanceResult {
        self.expect(ExpectActual::Value(actual.clone()), None)
            .to_be(expected)
    }

    fn deep_equal(&self, actual: &Value, expected: &Value) -> ConformanceResult {
        self.expect(ExpectActual::Value(actual.clone()), None)
            .to_equal(expected)
    }

    fn partial_deep_equal(&self, actual: &Value, expected: &Value) -> ConformanceResult {
        self.expect(ExpectActual::Value(actual.clone()), None)
            .to_match_object(expected)
    }

    fn greater_than(&self, actual: i64, expected: i64) -> ConformanceResult {
        self.expect(ExpectActual::Value(Value::from(actual)), None)
            .to_be_greater_than(expected)
    }

    fn rejects(&self, operation: StorageOperation, message_includes: &str) -> ConformanceResult {
        // The operation crosses the erased `expect` boundary, so it is
        // transplanted to `'static` behind a shared cell the matcher drains
        // once.
        let slot = std::sync::Mutex::new(Some(operation));
        self.expect(
            ExpectActual::Operation(Box::new(move || {
                let operation = slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                    .expect("rejects operation runs once");
                operation()
            })),
            None,
        )
        .rejects_to_throw(message_includes)
    }
}

impl ExpectAssertions {
    fn expect(&self, actual: ExpectActual, message: Option<&str>) -> Arc<dyn ExpectResultLike> {
        (self.expect)(actual, message)
    }
}
