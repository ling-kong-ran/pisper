//! Port of `src/harness/**`: the durable agent harness over one Session —
//! conversation documents (`config`, `usage`, `inbox`, `live`), prompt
//! planning (`prompt`), the shared type surface (`types`), and (later
//! slices of this phase) the registry, scheduler, generation, tool, events,
//! submissions, view, and facade modules.
//!
//! Ported here so far, dependency-first:
//!
//! - [`types`] — the harness type surface (`harness/types.ts`).
//! - [`prompt`] — section replay/render and `pi.system` entry planning
//!   (`harness/prompt.ts`).
//! - [`config`], [`usage`], [`inbox`], [`live`] — the built-in conversation
//!   documents and their boundary / ledger logic.
//!
//! Remaining upstream files (context.ts, events.ts, generation.ts,
//! harness.ts facade, output.ts, registry.ts, scheduler.ts, submissions.ts,
//! tool.ts, view.ts) are ported in later steps of this slice; re-exports
//! cover only what exists.

pub mod agent;
/// Upstream `harness/compaction.ts` (v1.0.0); its pure layer with the
/// phase-machine data shapes. See the module docs for the port scope.
pub mod compaction;
pub mod config;
pub mod context;
pub mod define;
pub mod events;
pub mod generation;
// Upstream file `harness/harness.ts` keeps the doubled segment name.
#[allow(clippy::module_inception)]
pub mod harness;
pub mod inbox;
pub mod json;
pub mod live;
pub mod output;
pub mod prompt;
pub mod provider;
pub mod registry;
pub mod scheduler;
pub mod submissions;
pub mod task_graph;
pub mod tool;
pub mod types;
pub mod usage;
pub mod view;

#[cfg(test)]
mod oracle_tests;
