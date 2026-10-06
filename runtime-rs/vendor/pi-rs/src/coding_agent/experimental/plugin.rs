//! Port of the upstream `plugin.ts` re-export face
//! (sha256 2d8b713f17d3240697c74374c7ead0ba1b4d0d891cc904e7d62a6be458a4c5e6):
//! the experimental plugin API surface that bundles may import.
//!
//! Upstream re-exports the concrete services and payload types from
//! `services/`; in this port the payload types live in
//! [`crate::coding_agent::experimental::services`] and the service objects
//! are their id constants. The two `local: true` services
//! (`pi.local.presentation-ui`, `pi.local.slash-commands`) are declared here
//! because the services slice did not carry them.

pub use crate::coding_agent::experimental::services::{
    service_ids, AgentCompactionRequest, AgentNavigationRequest, AgentOperationError,
    AgentOperationResponse, AgentPromptImage, AgentPromptRequest, AgentQueueResponse,
};

/// Upstream `services/presentation-ui.ts`
/// `defineService<PresentationUI>("pi.local.presentation-ui", { local: true })`.
pub const PRESENTATION_UI: &str = "pi.local.presentation-ui";

/// Upstream `services/slash-commands.ts`
/// `defineService<SlashCommands>("pi.local.slash-commands", { local: true })`.
pub const SLASH_COMMANDS: &str = "pi.local.slash-commands";

/// Upstream `services/agent-controller.ts` `AgentController` service id.
pub const AGENT_CONTROLLER: &str = service_ids::AGENT_CONTROLLER;

/// Upstream `services/plugins.ts` `PresentationPlugins` service id.
pub const PRESENTATION_PLUGINS: &str = service_ids::PRESENTATION_PLUGINS;

/// Upstream `services/slash-commands.ts` payload face: one completion item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandCompletion {
    pub name: String,
    pub description: String,
}

/// Upstream `services/slash-commands.ts` payload face: a contributed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandContribution {
    pub name: String,
    pub description: String,
}

/// Upstream `services/slash-commands.ts` payload face: a run result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandRunResult {
    pub message: Option<String>,
}
