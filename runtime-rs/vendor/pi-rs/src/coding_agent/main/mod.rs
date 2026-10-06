//! Startup orchestration from upstream coding-agent/src/main.ts.
//!
//! The real session selection and cwd-bound runtime factory live here, shared
//! by the native `pi-rust` binary and integration tests. Auth, metadata, print
//! and RPC have real process entry points. Interactive setup, legacy migrations,
//! native package commands and HTML export remain separate integration work.
pub mod auth;
pub mod entry;
pub mod input;
pub mod options;
pub mod runtime;
pub mod sessions;

#[cfg(test)]
mod options_tests;
#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod sessions_tests;
