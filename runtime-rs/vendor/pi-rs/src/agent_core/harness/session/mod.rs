//! Port of `packages/agent/src/harness/session/` (M3b Tasks 5 + 7): the
//! durable session envelope — the ROADMAP's byte-format compatibility
//! deliverable. Task 5 landed the entry vocabulary ([`types`]) and context
//! builders ([`context`]); Task 7 adds the stored-value addressing
//! ([`values`]), the commit algebra ([`commit`]), fork policy
//! ([`fork_policy`]), the materialized storage state ([`storage_state`]),
//! the memory backend ([`memory`]), the session/branch capability layer
//! ([`session`]), the JSONL storage + repo + legacy v3 migration
//! ([`jsonl`]), and the shared test decorators/conformance suites
//! ([`testing`]).
//!
//! Plan note (disclosed): the M3b plan text names
//! `pico3/session.ts`/`pico3/jsonl.ts` as this task's upstream sources, but
//! every other Task 7 requirement — the acceptance envelope
//! `{type,id,parentId,timestamp,message}`, the nine assigned oracle test
//! files (all importing `harness/session/*`), the Task 5
//! `findEntries`/`getEntry` projections, and the pre-flight "session/jsonl
//! before pico3 core" ruling — describes THIS subtree. `pico3/session.ts` is
//! the chord-transactional engine (numeric ids, no `parentId` envelope) and
//! hard-depends on `pico3/types.ts` (Task 8); it moves to Task 8 with the
//! rest of the pico3 core. See the task report for the full ruling trail.

pub mod commit;
pub mod context;
pub mod fork_policy;
pub mod jsonl;
pub mod memory;
// The inner `session` submodule mirrors the upstream file layout
// (session/session.ts) under one module root.
#[allow(clippy::module_inception)]
pub mod session;
pub mod storage_state;
pub mod testing;

#[cfg(test)]
mod testing_conformance;

pub mod types;
pub mod values;

pub use commit::*;
pub use context::*;
pub use fork_policy::*;
pub use memory::*;
pub use session::*;
pub use storage_state::*;
pub use testing::*;
pub use types::*;
pub use values::*;
