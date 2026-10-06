//! Port of upstream `coding-agent/src/extensions/mcp/**` (HEAD `2bbfcca43`,
//! v0.99.1): the built-in MCP integration — the `mcpServers` config files, the
//! server connections, the `mcp__<server>__<tool>` tool bridge, the resource
//! tools, the OAuth sign-in surface, the `/mcp` manager, and the `pi mcp`
//! CLI.
//!
//! Provenance map (upstream file → submodule):
//!
//! | upstream | submodule |
//! |---|---|
//! | `extensions/mcp/index.ts` | [`index`] |
//! | `extensions/mcp/config.ts` | [`config`] |
//! | `extensions/mcp/log.ts` | [`log`] |
//! | `extensions/mcp/tools.ts` | [`tools`] |
//! | `extensions/mcp/resources.ts` | [`resources`] |
//! | `extensions/mcp/runtime.ts` | [`runtime`] |
//! | `extensions/mcp/oauth.ts` | [`oauth`] |
//! | `extensions/mcp/ui.ts` | [`ui`] |
//! | `extensions/mcp/cli.ts` | [`cli`] |
//! | `extensions/mcp/runtime.lazy.ts` / `cli.lazy.ts` | (the runtime is
//!   statically linked; the lazy-import boundary has no Rust equivalent) |
//!
//! The MCP client stack underneath is the ported `@earendil-works/pi-mcp`
//! ([`crate::mcp`]), the server registry the ported
//! `core/mcp-servers` ([`crate::coding_agent::core::mcp_servers`]), and the
//! extension surface the ported extensions system
//! ([`crate::coding_agent::extensions`]).
//!
//! Deterministic outputs (mcp.json parse/shape/validate errors, entry
//! precedence, tool-name mapping with hash suffixes, exposure resolution
//! through the stack, tool-list→agent-tool declaration JSON, resource listing
//! shapes, log formatting, `/mcp` menu data) are captured from the verbatim
//! upstream TypeScript under node (type stripping) into
//! `tests/fixtures/mcp_extension_oracle/` and pinned by tests in this module.

pub mod cli;
pub mod config;
pub mod index;
pub mod log;
pub mod oauth;
pub mod resources;
pub mod runtime;
pub mod tools;
pub mod ui;

#[cfg(test)]
mod oracle_tests;

/// Upstream `VERSION` (config.ts): the version the MCP client identifies as in
/// `initialize`. The port embeds the Rust package's version (disclosed).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub use cli::{run_mcp_command, McpCommandOptions};
pub use config::{
    add_mcp_server_config, load_mcp_config, remove_mcp_server_config, update_mcp_server_config,
    LoadedMcpConfig, LoadedMcpConfigOptions, McpConfigScope, McpServerConfigPatch, McpServerEntry,
};
pub use index::{create_mcp_extension, open_browser, McpExtensionOptions};
pub use log::{format_mcp_log_message, McpServerLog};
pub use resources::{
    create_mcp_resource_tool_definitions, is_mcp_app_resource, McpResourceServer,
    McpResourceToolOptions, LIST_MCP_RESOURCES_TOOL, LIST_MCP_RESOURCE_TEMPLATES_TOOL,
    READ_MCP_RESOURCE_TOOL,
};
pub use runtime::{
    create_default_transport, McpServerConnection, McpServerConnectionOptions, McpTransportFactory,
    ServerState,
};
pub use tools::{
    convert_mcp_result, create_mcp_result_schema, create_mcp_tool_definition, create_mcp_tool_name,
    limit_mcp_content, to_model_content, to_tool_exposure, truncate_middle,
    ConvertMcpResultOptions, McpToolCaller, McpToolDetails, MCP_OUTPUT_MAX_BYTES,
};
pub use ui::{McpMenu, McpMenuItem, McpUi};
