//! Port of `src/testing/runner.ts`: registers the runner-independent cases
//! with a Vitest/Jest-compatible test runner.
//!
//! D37 (runner registration): upstream reads the global `describe` / `it` /
//! `expect` bindings off the runner object; the port carries them as function
//! fields ([`StorageConformanceRunner`]) and invokes them in the same order.

use std::sync::Arc;

use super::assertions::create_expect_assertions;
use super::storage_conformance::create_storage_conformance;
use super::types::{ConformanceResult, StorageConformanceOptions, StorageConformanceProvider};

/// One suite test body as `it` receives it: the case run handed to
/// `withStorage` (upstream `() => Promise<void>`).
pub type SuiteTest = Arc<dyn Fn() -> ConformanceResult + Send + Sync>;

/// `describe(name, suite)`: register a suite; the port's suite runs
/// synchronously inside the call.
pub type DescribeFn = Arc<dyn Fn(&str, &mut dyn FnMut()) + Send + Sync>;
/// `it(name, test)`: register one test.
pub type ItFn = Arc<dyn Fn(&str, SuiteTest) + Send + Sync>;

/// The runner surface (`runner.ts` `StorageConformanceRunner`).
#[derive(Clone)]
pub struct StorageConformanceRunner {
    /// `describe(name, suite)`: register a suite.
    pub describe: DescribeFn,
    /// The `expect(actual, message?)` matcher factory.
    pub expect: super::assertions::ExpectLike,
    /// `it(name, test)`: register one test.
    pub it: ItFn,
}

/// `registerStorageConformance(runner, name, withStorage)`
/// (`runner.ts`): build the cases and register them with the runner.
pub fn register_storage_conformance(
    runner: &StorageConformanceRunner,
    name: &str,
    with_storage: StorageConformanceProvider,
) {
    let options = StorageConformanceOptions {
        assertions: Arc::new(create_expect_assertions(Arc::clone(&runner.expect))),
        with_storage,
    };
    let cases = create_storage_conformance(&options);
    let suite_cases = Arc::new(cases);
    let suite_options = Arc::new(options);
    let it = Arc::clone(&runner.it);
    let mut suite = move || {
        for case in suite_cases.iter() {
            let case_name = case.name.clone();
            let case = case.clone();
            let case_options = Arc::clone(&suite_options);
            it(&case_name, Arc::new(move || case.run(&case_options)));
        }
    };
    (runner.describe)(name, &mut suite);
}
