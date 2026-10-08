//! Port of `packages/agent/src/harness/pico3/kinds/` — the built-in kinds
//! harness.ts registers (`harness.ts:157`): `generation`, `tool`,
//! `post_tools`, `collapse`, `job`, `plugin`. `kinds/entries.ts` (39) is the
//! entry-kind witness table (`kinds::entries`); `kinds/task-api.ts` (52) is
//! [`crate::agent_core::harness::pico3::runtime::ToolApi::base`]; the frame
//! application (`kinds/frames.ts`) is superseded by the pi-ai port's
//! [`PartialAssistant`] (see `generation.rs` module docs); the context
//! estimator (`packages/ai/src/utils/estimate.ts`) lands in [`estimate`]
//! beside its only consumer.

use std::sync::Arc;

pub mod collapse;
pub mod entries;
pub mod estimate;
pub mod generation;
pub mod job;
pub mod plugin;
pub mod post_tools;
pub mod tool;

/// Upstream `kinds` (`harness.ts:810`): the built-in kinds as typed
/// witnesses. The port hands out the registered kind instances.
pub struct BuiltinKinds {
    pub generation: Arc<generation::GenerationKind>,
    pub tool: Arc<tool::ToolKind>,
    pub post_tools: Arc<post_tools::PostToolsKind>,
    pub collapse: Arc<collapse::CollapseKind>,
    pub job: Arc<job::JobKind>,
    pub plugin: Arc<plugin::PluginKind>,
}

/// Construct the built-in set (`harness.ts:810`).
pub fn builtins() -> BuiltinKinds {
    BuiltinKinds {
        generation: generation::GenerationKind::new(),
        tool: tool::ToolKind::new(),
        post_tools: post_tools::PostToolsKind::new(),
        collapse: collapse::CollapseKind::new(),
        job: job::JobKind::new(),
        plugin: plugin::PluginKind::new(),
    }
}
