//! Port of `packages/agent/src/harness/env/` (M3b Task 6): the concrete
//! execution environment. Upstream ships a single backend,
//! `env/nodejs.ts` (924 lines); the port is [`nodejs::NodeExecutionEnv`] over
//! `tokio`/`std` filesystem and process primitives, implementing the
//! [`FileSystem`](crate::agent_core::harness::types::FileSystem),
//! [`Shell`](crate::agent_core::harness::types::Shell), and
//! [`ExecutionEnv`](crate::agent_core::harness::types::ExecutionEnv) traits
//! the M3b Task 2 types define.
//!
//! It replaces the `TestFsEnv` fixture (`harness/test_env.rs`) as the real
//! backend for consumers.

pub mod nodejs;

pub use nodejs::NodeExecutionEnv;
