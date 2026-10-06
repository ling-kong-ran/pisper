//! Port of `src/testing/**`: the runner-independent storage conformance
//! suite, its Vitest/Jest adapter, and the deterministic storage benchmark
//! seeders. Upstream ships these as a real package entry point (`./testing`),
//! so the module is not `#[cfg(test)]`-gated here either.
//!
//! Ported files (one-to-one): [`assertions`] (`assertions.ts`), [`runner`]
//! (`runner.ts`), [`storage_benchmark`] (`storage-benchmark.ts`),
//! [`storage_conformance`] (`storage-conformance.ts`), [`types`]
//! (`types.ts`); `index.ts` maps to this module's re-exports.
//!
//! Storage crosses the suite synchronously (D3) with the port's
//! [`crate::durable::storage::StorageError`] channel (D4); assertion,
//! provider, and case contracts are declared in [`types`]. Per-file
//! divergences continuing the module-wide numbering: D35 (conformance error
//! channel), D36 (assertion facade collapse), D37 (runner registration),
//! D38 (the JS prototype-identity checks in the prototype-like-keys case —
//! `Object.getPrototypeOf`, `Object.hasOwn`, `({}).polluted` — are vacuous
//! over the port's prototype-free maps but recorded identically in the
//! oracle traces), D39 (the lone-surrogate identity case is not portable and
//! is omitted).

pub mod assertions;
pub mod runner;
pub mod storage_benchmark;
pub mod storage_conformance;
pub mod types;

pub use assertions::{create_expect_assertions, ExpectActual, ExpectLike, ExpectResultLike};
pub use runner::{register_storage_conformance, StorageConformanceRunner, SuiteTest};
pub use storage_benchmark::{
    seed_storage_benchmark, seed_storage_write_benchmark, storage_benchmark_primary_record_count,
    storage_read_benchmarks, storage_write_benchmarks, StorageBenchmarkDataset,
    StorageBenchmarkScale, StorageReadBenchmark, StorageWriteBenchmark, STORAGE_MEMORY_SCALES,
    TIMING_SCALE,
};
pub use storage_conformance::create_storage_conformance;
pub use types::{
    ConformanceFailure, ConformanceResult, ConformanceTest, StorageConformanceAssertions,
    StorageConformanceCase, StorageConformanceOptions, StorageConformanceProvider,
    StorageOperation,
};

#[cfg(test)]
mod oracle_tests;
