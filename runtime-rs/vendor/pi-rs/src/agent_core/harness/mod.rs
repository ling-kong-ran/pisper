//! Port of the harness foundation from upstream
//! `packages/agent/src/harness/` (M3b Task 2): the six root files every
//! harness module builds on — [`types`], [`events`], [`messages`],
//! [`result`], [`config`], and [`context`]. Each item cites its upstream
//! source file and lines.
//!
//! Later M3b tasks add the remaining harness subtree under this module
//! ([`hooks`] (Task 4), `compaction` (Task 5), `execution`/`env` (Task 6),
//! `pico3` (Tasks 7-9), `agent_harness`/`telemetry` (Task 10)); the skills
//! and prompt-template loaders already live here.
//!
//! Disclosed substitutions (module docs carry the per-item notes):
//! - Upstream `Result<TValue, TError>` and its `ok`/`err`/`getOrThrow`/
//!   `getOrUndefined` helpers map onto [`std::result::Result`], `Ok`/`Err`,
//!   `unwrap`/`expect`, and `.ok()` — see [`types`] module docs.
//! - Upstream `AbortSignal` is [`tokio_util::sync::CancellationToken`] (the
//!   repo-wide convention); [`context::with_abort_signal`] combines signals
//!   with a small linker task.
//! - The event bus ([`events`]) is generic over a [`events::BusEvent`] trait:
//!   upstream `events.ts` only relies on the event's `type` discriminator,
//!   optional `lane`, and the `handler_error` shape, all of which the full
//!   `HarnessEvent` union (ported with `agent-harness.ts`, M3b Task 10) will
//!   implement. The oracle tests here use a fixture event with the same
//!   variants the upstream tests use.
//! - [`config`] also hosts `CompactionSettings` (upstream home:
//!   `harness/compaction/compaction.ts:147-161`) so the validator has its
//!   argument type before the compaction task ports; the compaction module
//!   will re-export it.

pub mod agent_harness;
pub mod compaction;
pub mod config;
pub mod context;
pub mod env;
pub mod events;
pub mod execution;
pub mod hooks;
pub mod messages;
pub mod pico3;
pub mod prompt_templates;
pub mod result;
pub mod runtime;
pub mod session;
pub mod skills;
pub mod telemetry;
pub mod tools;
pub mod types;
pub mod utils;

#[cfg(test)]
pub(crate) mod test_env;

pub use compaction::*;
pub use config::*;
pub use context::*;
pub use env::*;
pub use events::*;
pub use execution::*;
pub use hooks::*;
pub use messages::*;
pub use prompt_templates::*;
pub use result::*;
pub use session::*;
pub use skills::*;
pub use types::*;
pub use utils::*;
