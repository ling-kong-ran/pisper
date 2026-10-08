//! Port of upstream `coding-agent/src/extensions/mcp/index.ts` (HEAD
//! `2bbfcca43`): the built-in MCP integration.
//!
//! Connects the servers from `mcp.json` and the servers extensions register
//! with `pi.registerMcpServer()` when a session starts, and servers registered
//! later right away. A server in `mcp.json` takes precedence over a registered
//! server of the same name. Tools are registered as `mcp__<server>__<tool>`.
//! By default (`"exposure": "codemode"`) the tools are only callable from
//! codemode scripts; `"deferred"` declares the tools once the `tool_search`
//! tool loads them; `"direct"` declares them right away; `"hidden"` makes them
//! unreachable. `toolExposure` overrides the exposure of single tools. Servers
//! with resources are reached through the `list_mcp_resources`,
//! `list_mcp_resource_templates`, and `read_mcp_resource` tools
//! ([`super::resources`]).
//!
//! Problems found at startup (config errors, failed connections, servers that
//! need a sign-in) are reported once. `/mcp` opens a manager to sign in,
//! reconnect, enable or disable servers, and change their exposure; the last
//! two are saved to the `mcp.json` that defines the server, or apply to the
//! current session for registered servers.
//!
//! Disclosed seams:
//! - **TUI manager view** ([`super::ui`]): the `/mcp` manager runs over the
//!   [`McpUi`] seam; the interactive `McpManagerView` component is cropped,
//!   and the port's in-session manager drives the blocking dialogs. Menu data
//!   ([`McpMenu`]) is oracle-pinned.
//! - **Codemode detection**: `isCodemodeTool` is a name check (`"codemode"`);
//!   the codemode extension belongs to its own slice. `tool_search` detection
//!   uses the ported structural check.
//! - **`registeredConfig` snapshots**: upstream stringifies the validated raw
//!   config object; the port serializes a fixed-order projection of the typed
//!   config (the snapshot only feeds re-registration change detection, so the
//!   exact bytes are internal).
//! - **`pending` scheduling**: upstream defers the first connection with
//!   `setImmediate` so the first render happens first; the port spawns the
//!   connect-all work on Tokio (the handler returns before connections start).
//! - `openBrowser` is the verbatim strategy port (`rundll32 url.dll` on
//!   Windows, `open` on macOS, `xdg-open` elsewhere; no shell).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};

use futures::future::BoxFuture;
use futures::FutureExt as _;
use serde_json::{json, Value};

use crate::coding_agent::core::mcp_servers::{
    get_mcp_tool_exposure, mcp_namespace, validate_mcp_server_config, McpExposure,
};
use crate::coding_agent::core::path_join;
use crate::coding_agent::extensions::loader::ExtensionApi;
use crate::coding_agent::extensions::tool_search::is_tool_search_tool;
use crate::coding_agent::extensions::types::ProviderRegistryHandle;
use crate::coding_agent::extensions::types::{
    AbortSignal, CommandFuture, ExtensionCommandContext, ExtensionContext, ExtensionMode,
    HandlerResult, ToolDefinition, ToolExposure, ToolInfo, ToolNamespace,
};

use super::config::{
    exposure_json_name, load_mcp_config, update_mcp_server_config, LoadedMcpConfig,
    LoadedMcpConfigOptions, McpConfigScope, McpServerConfigPatch, McpServerEntry,
};
use super::oauth::{sign_in_mcp_server, McpOAuthCredentialStore, McpSignInError, McpSignInPrompt};
use super::resources::{
    create_mcp_resource_tool_definitions, locale_compare, McpResourceServer, McpResourceToolOptions,
};
use super::runtime::{
    create_default_transport, McpServerConnection, McpServerConnectionOptions, McpServerLog,
    McpTransportFactory, ProviderTokenResolver, ServerState,
};
use super::tools::{create_mcp_tool_definition, create_mcp_tool_name, McpToolCaller};
use super::ui::{McpMenu, McpMenuItem, McpUi, MenuBuilder, MenuListener, MenuUnsubscribe};

pub const CODEMODE_TOOL_NAME: &str = "codemode";
pub const TOOL_SEARCH_TOOL_NAME: &str = "tool_search";

const DEFAULT_STARTUP_WAIT_MS: u64 = 10_000;

/// Upstream `McpExtensionOptions`.
#[derive(Clone, Default)]
pub struct McpExtensionOptions {
    /// Defaults to reading `mcp.json` from the agent directory and the trusted
    /// project.
    pub load_config: Option<LoadConfigHook>,
    /// Defaults to stdio and streamable HTTP transports built from the server
    /// config.
    pub create_transport: Option<McpTransportFactory>,
    /// Defaults to `mcp-auth.json` in the agent directory.
    pub credentials: Option<Arc<McpOAuthCredentialStore>>,
    /// File server log messages are appended to. Defaults to `mcp.log` in the
    /// agent directory.
    pub log_path: Option<String>,
    /// Opens the OAuth authorization URL. Defaults to the platform browser.
    pub open_url: Option<OpenUrlHook>,
    /// Saves `/mcp` changes to the server's config file. Defaults to editing
    /// its `mcp.json`. `Err` is the upstream throw.
    pub update_config: Option<UpdateConfigHook>,
    /// How long the first prompt waits for servers that are still connecting
    /// at startup, in milliseconds. Their tools become available when they
    /// connect. Default: 10000.
    pub startup_wait_ms: Option<u64>,
}

/// A configured server. Disabled servers have no connection (upstream
/// `McpServer`).
#[derive(Clone)]
struct McpServer {
    entry: McpServerEntry,
    connection: Option<Arc<McpServerConnection>>,
    /// For servers extensions registered: the config as registered, to detect
    /// re-registrations.
    registered_config: Option<String>,
    /// Result of the last `/mcp` action that failed, shown in the manager.
    message: Option<String>,
    /// Settles when the connection started for the server connected or failed
    /// (upstream `server.ready`).
    ready: Option<futures::future::Shared<BoxFuture<'static, ()>>>,
}

const EXPOSURE_DESCRIPTIONS: [(&str, &str); 3] = [
    (
        "codemode",
        "called from codemode scripts, which find them with searchTools()",
    ),
    (
        "deferred",
        "not declared until tool_search loads them, then called directly; no codemode needed",
    ),
    ("direct", "declared to the model like built-in tools"),
];

fn exposure_name(exposure: McpExposure) -> &'static str {
    match exposure {
        McpExposure::Codemode => "codemode",
        McpExposure::Deferred => "deferred",
        McpExposure::Direct => "direct",
        McpExposure::Hidden => "hidden",
    }
}

fn exposure_description(exposure: McpExposure) -> Option<&'static str> {
    let name = exposure_name(exposure);
    EXPOSURE_DESCRIPTIONS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, description)| *description)
}

fn first_line(text: &str) -> &str {
    text.split('\n').next().unwrap_or("")
}

fn is_enabled(server: &McpServer) -> bool {
    server.entry.config.enabled()
}

fn exposure_of(entry: &McpServerEntry) -> McpExposure {
    entry.config.exposure()
}

/// Exposures the server's tools can have, known from its config before it
/// connects (upstream `configuredExposures`).
fn configured_exposures(entry: &McpServerEntry) -> Vec<McpExposure> {
    let mut exposures = vec![exposure_of(entry)];
    for (_, exposure) in entry.config.tool_exposure().iter() {
        if !exposures.contains(exposure) {
            exposures.push(*exposure);
        }
    }
    exposures
}

/// Whether some of the server's tools are declared to the model, so the first
/// prompt waits for them (upstream `hasDirectTools`).
fn has_direct_tools(entry: &McpServerEntry) -> bool {
    configured_exposures(entry).contains(&McpExposure::Direct)
}

/// Whether some of the server's tools are reached through codemode or
/// tool_search (upstream `hasIndirectTools`).
fn has_indirect_tools(entry: &McpServerEntry) -> bool {
    let exposures = configured_exposures(entry);
    exposures.contains(&McpExposure::Codemode) || exposures.contains(&McpExposure::Deferred)
}

/// Name of the system prompt section that lists the servers whose tools are
/// not declared (upstream `MCP_SERVERS_SECTION`).
pub const MCP_SERVERS_SECTION: &str = "mcp_servers";
/// Characters of a server description in the section, as Codex allows for
/// deferred namespaces (upstream `MAX_SERVER_DESCRIPTION_CHARS`).
const MAX_SERVER_DESCRIPTION_CHARS: usize = 250;
/// Characters of the whole section. Descriptions shrink to fit; when the
/// server lines alone do not fit, the last servers are left out and counted
/// in a closing line (upstream `MAX_SERVERS_SECTION_CHARS`).
pub const MAX_SERVERS_SECTION_CHARS: usize = 4096;

/// The section's first line. It explains only the ways of reaching tools that
/// the listed servers use (upstream `serversSectionIntro`).
fn servers_section_intro(reaches: &[&str]) -> String {
    let mut intro = "MCP servers whose tools are not declared to you.".to_string();
    if reaches.contains(&"codemode") {
        intro += " Call the tools of `codemode` servers from codemode scripts.";
    }
    if reaches.contains(&"tool_search") {
        intro += " Load the tools of `tool_search` servers with `tool_search`.";
    }
    intro
}

/// Upstream `truncate`: cut to `max` characters, ellipsis-terminated; the
/// empty string when `max <= 1`.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    if max <= 1 {
        return String::new();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    while cut.ends_with(char::is_whitespace) {
        cut.pop();
    }
    cut.push('\u{2026}');
    cut
}

/// What the `mcp_servers` section needs of a server (upstream
/// `McpServerListing`).
pub struct McpServerListing<'a> {
    pub entry: &'a McpServerEntry,
    pub connection_instructions: Option<String>,
}

/// First line of the configured description, or of the server instructions
/// once connected (upstream `serverSummary`).
fn server_summary(server: &McpServerListing) -> String {
    let text = server
        .entry
        .config
        .description()
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(|description| description.to_string())
        .or_else(|| server.connection_instructions.clone())
        .unwrap_or_default();
    first_line(&text).trim().to_string()
}

/// The `mcp_servers` section: every enabled server with codemode or deferred
/// tools, with how its tools are reached and a one-line summary. The model
/// learns of the servers from it, since neither codemode nor tool_search
/// lists them. `None` when there are no such servers (upstream
/// `renderServersSection`).
pub fn render_servers_section(servers: &[McpServerListing]) -> Option<String> {
    let mut listed: Vec<&McpServerListing> = servers
        .iter()
        .filter(|server| server.entry.config.enabled() && has_indirect_tools(server.entry))
        .collect();
    listed.sort_by(|a, b| locale_compare(a.entry.name.as_str(), b.entry.name.as_str()));
    if listed.is_empty() {
        return None;
    }
    let reaches: Vec<&str> = listed
        .iter()
        .map(|server| {
            if configured_exposures(server.entry).contains(&McpExposure::Codemode) {
                "codemode"
            } else {
                "tool_search"
            }
        })
        .collect();
    let intro = servers_section_intro(&reaches);
    let heads: Vec<String> = listed
        .iter()
        .enumerate()
        .map(|(index, server)| {
            format!(
                "- {} ({})",
                mcp_namespace(&server.entry.name),
                reaches[index]
            )
        })
        .collect();
    let omitted = |count: usize| -> Vec<String> {
        (count > 0)
            .then(|| {
                format!(
                    "- … {count} more server{}; find their tools with searchTools()",
                    if count == 1 { "" } else { "s" }
                )
            })
            .into_iter()
            .collect()
    };
    // Characters of the intro, the first `kept` server lines without
    // descriptions, and the omission line.
    let size = |kept: usize| -> usize {
        [intro.clone()]
            .into_iter()
            .chain(heads.iter().take(kept).cloned())
            .chain(omitted(listed.len() - kept))
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .count()
    };
    let mut kept = listed.len();
    while kept > 0 && size(kept) > MAX_SERVERS_SECTION_CHARS {
        kept -= 1;
    }
    // Each description also takes a ": " separator.
    let per_server = (MAX_SERVERS_SECTION_CHARS.saturating_sub(size(kept)))
        .checked_div(kept)
        .unwrap_or(0)
        .saturating_sub(2)
        .min(MAX_SERVER_DESCRIPTION_CHARS);
    let mut lines: Vec<String> = Vec::new();
    for (index, server) in listed.iter().take(kept).enumerate() {
        let summary = if per_server > 0 {
            truncate(&server_summary(server), per_server)
        } else {
            String::new()
        };
        if summary.is_empty() {
            lines.push(heads[index].clone());
        } else {
            lines.push(format!("{}: {}", heads[index], summary));
        }
    }
    Some(
        [intro]
            .into_iter()
            .chain(lines)
            .chain(omitted(listed.len() - kept))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Whether a codemode script needs the server: it names the server's
/// namespace, or searches, enumerates, or describes tools or namespaces,
/// which may name the server in other forms (upstream `scriptNeedsServer`).
fn script_needs_server(code: &str, server: &str) -> bool {
    if code
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|word| {
            matches!(
                word,
                "searchTools" | "describeNamespace" | "describeTool" | "ALL_TOOLS"
            )
        })
    {
        return true;
    }
    code.contains(&mcp_namespace(server))
}

/// Short state for lists and the startup report. `with_error` appends the
/// first line of a failure (upstream `describeState`).
fn describe_state(server: &McpServer, with_error: bool) -> String {
    if !is_enabled(server) {
        return "disabled".to_string();
    }
    let Some(connection) = &server.connection else {
        return "starting".to_string();
    };
    match connection.state() {
        ServerState::NeedsAuth => "needs sign-in".to_string(),
        ServerState::Failed => {
            if with_error {
                format!(
                    "failed: {}",
                    first_line(connection.error().as_deref().unwrap_or("unknown error"))
                )
            } else {
                "failed".to_string()
            }
        }
        ServerState::Connected => {
            let tools = connection.tools().len();
            let count = connection.resources().len();
            let resource_count = if count > 0 {
                format!(" · {count} resource{}", if count == 1 { "" } else { "s" })
            } else {
                String::new()
            };
            format!(
                "connected · {tools} tool{}{resource_count}",
                if tools == 1 { "" } else { "s" }
            )
        }
        ServerState::Connecting => "connecting…".to_string(),
        other => other.as_str().to_string(),
    }
}

/// Servers that need the user first (upstream `attentionRank`).
fn attention_rank(server: &McpServer) -> u8 {
    if !is_enabled(server) {
        return 5;
    }
    match server
        .connection
        .as_ref()
        .map(|connection| connection.state())
    {
        Some(ServerState::NeedsAuth) => 0,
        Some(ServerState::Failed) => 1,
        Some(ServerState::Disconnected) => 2,
        Some(ServerState::Connected) => 4,
        _ => 3,
    }
}

fn describe_transport(entry: &McpServerEntry) -> String {
    let config = &entry.config;
    if let Some(url) = config.url() {
        return url.to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(command) = config.command() {
        parts.push(command.to_string());
    }
    for arg in config.args().unwrap_or_default() {
        parts.push(arg.to_string());
    }
    parts.join(" ")
}

const MCP_USAGE: &str =
    "Usage: /mcp, /mcp login [server], /mcp logout [server], /mcp reconnect [server]";

/// Open a URL or file in the platform browser/default handler, without a
/// shell (upstream `openBrowser`).
pub fn open_browser(target: &str) {
    #[cfg(target_os = "macos")]
    let (command, args) = ("open", vec![target.to_string()]);
    #[cfg(windows)]
    let (command, args) = (
        "rundll32",
        vec![
            "url.dll,FileProtocolHandler".to_string(),
            target.to_string(),
        ],
    );
    #[cfg(not(any(target_os = "macos", windows)))]
    let (command, args) = ("xdg-open", vec![target.to_string()]);
    // Launch is best-effort: callers still present the target to the user, so
    // a missing launcher never crashes the process (upstream `.on("error")`).
    let _ = std::process::Command::new(command)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Fixed-order projection of a validated server config, standing in for
/// `JSON.stringify(config)` (see the module docs).
pub(crate) fn config_value(
    config: &crate::coding_agent::core::mcp_servers::McpServerConfig,
) -> Value {
    let mut map = serde_json::Map::new();
    if let Some(command) = config.command() {
        map.insert("command".into(), Value::from(command));
        if let Some(args) = config.args() {
            map.insert("args".into(), json!(args));
        }
        if let Some(env) = config.string_record("env") {
            map.insert("env".into(), string_record_value(&env));
        }
        if let Some(cwd) = config.cwd() {
            map.insert("cwd".into(), Value::from(cwd));
        }
    }
    if let Some(url) = config.url() {
        map.insert("url".into(), Value::from(url));
        if let Some(headers) = config.string_record("headers") {
            map.insert("headers".into(), string_record_value(&headers));
        }
        if let Some(oauth) = config.oauth() {
            let mut oauth_map = serde_json::Map::new();
            if let Some(client_id) = &oauth.client_id {
                oauth_map.insert("clientId".into(), Value::from(client_id.clone()));
            }
            if let Some(client_secret) = &oauth.client_secret {
                oauth_map.insert("clientSecret".into(), Value::from(client_secret.clone()));
            }
            if let Some(port) = oauth.callback_port {
                oauth_map.insert("callbackPort".into(), Value::from(port));
            }
            if let Some(callback_url) = &oauth.callback_url {
                oauth_map.insert("callbackUrl".into(), Value::from(callback_url.clone()));
            }
            if let Some(scope) = &oauth.scope {
                oauth_map.insert("scope".into(), Value::from(scope.clone()));
            }
            if let Some(client_name) = &oauth.client_name {
                oauth_map.insert("clientName".into(), Value::from(client_name.clone()));
            }
            map.insert("oauth".into(), Value::Object(oauth_map));
        }
    }
    if let Some(timeout) = config.timeout() {
        map.insert(
            "timeout".into(),
            serde_json::Number::from_f64(timeout)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
    }
    map.insert(
        "exposure".into(),
        Value::from(exposure_name(config.exposure())),
    );
    if !config.tool_exposure().is_empty() {
        let mut tools = serde_json::Map::new();
        for (tool, exposure) in config.tool_exposure().iter() {
            tools.insert(tool.clone(), Value::from(exposure_name(*exposure)));
        }
        map.insert("toolExposure".into(), Value::Object(tools));
    }
    if !config.enabled() {
        map.insert("enabled".into(), Value::Bool(false));
    }
    if let Some(description) = config.description() {
        map.insert("description".into(), Value::from(description));
    }
    Value::Object(map)
}

fn string_record_value(record: &[(String, String)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in record {
        map.insert(key.clone(), Value::from(value.clone()));
    }
    Value::Object(map)
}

/// Upstream `options.loadConfig`.
pub type LoadConfigHook = Arc<dyn Fn(&ExtensionContext) -> LoadedMcpConfig + Send + Sync>;
/// Upstream `options.updateConfig` (`Err` is the upstream throw).
pub type UpdateConfigHook =
    Arc<dyn Fn(&McpServerEntry, McpServerConfigPatch) -> Result<(), String> + Send + Sync>;
/// Upstream `options.openUrl`.
pub type OpenUrlHook = Arc<dyn Fn(&str) + Send + Sync>;
/// The `getClient` thunk of a registered MCP tool.
pub type GetClientHook =
    Arc<dyn Fn() -> BoxFuture<'static, Result<Arc<dyn McpToolCaller>, String>> + Send + Sync>;

/// Shared mutable state of one extension instance (the upstream factory
/// closure's variables).
struct McpExtensionState {
    pi: ExtensionApi,
    options: McpExtensionOptions,
    servers: Vec<McpServer>,
    /// Servers from `mcp.json`, which take precedence over registered servers
    /// of the same name.
    configured_entries: Vec<McpServerEntry>,
    config_errors: Vec<String>,
    /// The trusted project's `mcp.json`, where `/mcp` saves project overrides
    /// of global servers (upstream `projectConfig`).
    project_config: Option<String>,
    /// Registered servers that `mcp.json` overrides, shown in `/mcp`.
    overridden: Vec<String>,
    /// Between session_start and session_shutdown. Registrations before that
    /// are read on session_start.
    session_active: bool,
    auto_enable_codemode: bool,
    /// Whether the "codemode tools unreachable" warning was shown since the
    /// session started.
    warned_unreachable: bool,
    /// The startup connect-all work, shared between session_start and the
    /// first prompt.
    pending: Option<futures::future::Shared<BoxFuture<'static, ()>>>,
    /// Whether a prompt already waited for the startup connections since the
    /// session started.
    waited_for_startup: bool,
    startup_wait_ms: u64,
    /// Bumped on every session start and shutdown so a runtime load that
    /// resolves late is dropped.
    generation: u64,
    /// Working directory of the session, for stdio servers.
    session_cwd: String,
    credentials: Option<Arc<McpOAuthCredentialStore>>,
    /// The session's model registry, which resolves `auth.provider` tokens
    /// (upstream `modelRegistry`).
    model_registry: Option<std::sync::Arc<dyn ProviderRegistryHandle>>,
    server_log: Option<Arc<McpServerLog>>,
    /// pi tool name to the `<server>\0<tool>` it was assigned to, so names
    /// stay unique and stable.
    tool_owners: HashMap<String, String>,
    /// Tool names currently offered by each server.
    server_tools: HashMap<String, BTreeSet<String>>,
    /// Last definition registered under each tool name, to re-register
    /// withdrawn tools as hidden.
    definitions: HashMap<String, ToolDefinition>,
    /// Stored tokens of servers waiting for a sign-in, as they were when the
    /// sign-in was needed (keyed by connection identity; the snapshot fills
    /// in asynchronously, upstream `tokensAtSignIn`).
    tokens_at_sign_in: HashMap<usize, (Arc<McpServerConnection>, TokenSnapshot)>,
    /// Exposure the resource tools were last registered with; `None` until a
    /// server has resources.
    resource_tools_exposure: Option<McpExposure>,
    listeners: Vec<(usize, std::sync::Weak<dyn Fn() + Send + Sync>)>,
    next_listener_id: usize,
    /// The owning handle, for callbacks that need an owned state.
    self_handle: Weak<Mutex<McpExtensionState>>,
}

type SharedState = Arc<Mutex<McpExtensionState>>;

/// The token snapshot taken when a server started waiting for a sign-in
/// (upstream the `string` value of `tokensAtSignIn`).
type TokenSnapshot = Arc<std::sync::Mutex<Option<String>>>;

fn connection_id(connection: &Arc<McpServerConnection>) -> usize {
    Arc::as_ptr(connection) as usize
}

fn default_load_config(ctx: &ExtensionContext) -> LoadedMcpConfig {
    load_mcp_config(LoadedMcpConfigOptions {
        agent_dir: crate::coding_agent::core::get_agent_dir(),
        cwd: ctx.cwd().unwrap_or_default(),
        project_trusted: ctx.is_project_trusted().unwrap_or(false),
    })
}

/// Upstream `createMcpExtension(options)`: the built-in MCP extension
/// factory.
pub fn create_mcp_extension(
    options: McpExtensionOptions,
) -> crate::coding_agent::extensions::loader::ExtensionFactory {
    Arc::new(move |pi: &ExtensionApi| {
        let startup_wait_ms = options.startup_wait_ms.unwrap_or(DEFAULT_STARTUP_WAIT_MS);
        let state: SharedState = Arc::new_cyclic(|handle| {
            Mutex::new(McpExtensionState {
                pi: pi.clone(),
                options: options.clone(),
                servers: Vec::new(),
                configured_entries: Vec::new(),
                config_errors: Vec::new(),
                project_config: None,
                overridden: Vec::new(),
                session_active: false,
                auto_enable_codemode: true,
                warned_unreachable: false,
                pending: None,
                waited_for_startup: false,
                startup_wait_ms,
                generation: 0,
                session_cwd: std::env::current_dir()
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                credentials: None,
                model_registry: None,
                server_log: None,
                tool_owners: HashMap::new(),
                server_tools: HashMap::new(),
                definitions: HashMap::new(),
                tokens_at_sign_in: HashMap::new(),
                resource_tools_exposure: None,
                listeners: Vec::new(),
                next_listener_id: 0,
                self_handle: handle.clone(),
            })
        });
        register_events(&state)?;
        register_mcp_command(&state)?;
        Ok(())
    })
}

fn lock(state: &SharedState) -> std::sync::MutexGuard<'_, McpExtensionState> {
    state
        .lock()
        .expect("mcp extension state cannot be poisoned")
}

/// `ctx.ui.notify(message, type)` over the runner UI handle (the port's
/// ExtensionContext reaches notifications through the UI surface).
fn notify_ctx(ctx: &ExtensionContext, message: &str, notify_type: Option<&str>) {
    if let Ok(ui) = ctx.ui() {
        ui.notify(message, notify_type);
    }
}

// ---------------------------------------------------------------------------------------
// Event registration
// ---------------------------------------------------------------------------------------

fn register_events(state: &SharedState) -> Result<(), String> {
    let pi = lock(state).pi.clone();

    // Upstream `pi.on("session_start", ...)` (synchronous).
    {
        let state = Arc::clone(state);
        pi.on(
            "session_start",
            crate::coding_agent::extensions::types::sync_handler(move |_event, ctx| {
                session_start(&state, ctx);
                Ok(None::<HandlerResult>)
            }),
        )?;
    }

    // Upstream `pi.on("before_agent_start", ...)`: the first prompt waits for
    // servers whose tools are declared to the model, so they are declared in
    // its first request, but not indefinitely: a slow or hanging server must
    // not hold up the prompt. Other servers are waited for when a script or
    // search needs them (`tool_call` below). Every prompt lists the servers in
    // the `mcp_servers` section as they are when it starts; pi appends the
    // section to the conversation when it changed, for example after a server
    // connected.
    {
        let state = Arc::clone(state);
        pi.on(
            "before_agent_start",
            Arc::new(move |event: &mut Value, ctx: &ExtensionContext| {
                let state = Arc::clone(&state);
                let ctx = ctx.clone();
                Box::pin(async move {
                    wait_for_direct_servers(&state, &ctx).await;
                    let section = {
                        let guard = lock(&state);
                        let listings: Vec<McpServerListing> = guard
                            .servers
                            .iter()
                            .map(|server| McpServerListing {
                                entry: &server.entry,
                                connection_instructions: server
                                    .connection
                                    .as_ref()
                                    .and_then(|connection| connection.instructions()),
                            })
                            .collect();
                        render_servers_section(&listings)
                    };
                    let options = event
                        .get_mut("systemPromptOptions")
                        .and_then(Value::as_object_mut);
                    if let Some(options) = options {
                        let sections = options
                            .entry("sections")
                            .or_insert_with(|| Value::Object(serde_json::Map::new()));
                        if let Some(sections) = sections.as_object_mut() {
                            match section {
                                Some(section) => {
                                    sections.insert(
                                        MCP_SERVERS_SECTION.to_string(),
                                        Value::from(section),
                                    );
                                }
                                None => {
                                    sections.remove(MCP_SERVERS_SECTION);
                                }
                            }
                        }
                    }
                    Ok(None)
                })
            }),
        )?;
    }

    // A codemode script waits for the servers it names, or for every server
    // when it searches or enumerates tools, so their tools are registered
    // before the script runs. tool_search and the resource tools reach every
    // server, so they wait for all of them.
    {
        let state = Arc::clone(state);
        pi.on(
            "tool_call",
            Arc::new(move |event: &mut Value, ctx: &ExtensionContext| {
                let state = Arc::clone(&state);
                let ctx = ctx.clone();
                let tool_name = event
                    .get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let input = event.get("input").cloned().unwrap_or(Value::Null);
                Box::pin(async move {
                    let tool = lock(&state)
                        .pi
                        .get_all_tools()
                        .unwrap_or_default()
                        .into_iter()
                        .find(|candidate| candidate.name == tool_name);
                    let Some(tool) = tool else {
                        return Ok(None);
                    };
                    let pending_servers: Vec<McpServer> = {
                        let guard = lock(&state);
                        guard
                            .servers
                            .iter()
                            .filter(|server| {
                                is_enabled(server)
                                    && server
                                        .connection
                                        .as_ref()
                                        .map(|connection| {
                                            connection.state() != ServerState::Connected
                                        })
                                        .unwrap_or(true)
                                    && server.ready.is_some()
                            })
                            .cloned()
                            .collect()
                    };
                    if pending_servers.is_empty() {
                        return Ok(None);
                    }
                    let waiting: Vec<McpServer> = if is_codemode_tool(&tool) {
                        let source = input
                            .get("code")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        pending_servers
                            .into_iter()
                            .filter(|server| script_needs_server(source, &server.entry.name))
                            .collect()
                    } else if is_tool_search_tool_info(&tool) || resource_tool_name(&tool_name) {
                        pending_servers
                    } else {
                        Vec::new()
                    };
                    wait_for_servers(&waiting, ctx.signal().ok().flatten()).await;
                    Ok(None)
                })
            }),
        )?;
    }

    // Upstream `pi.on("turn_start", ...)`: pick up sign-ins done outside the
    // session, such as `pi mcp login` run by the agent.
    {
        let state = Arc::clone(state);
        pi.on(
            "turn_start",
            Arc::new(move |_event: &mut Value, ctx: &ExtensionContext| {
                let state = Arc::clone(&state);
                let ctx = ctx.clone();
                Box::pin(async move {
                    let has_waiters = !lock(&state).tokens_at_sign_in.is_empty();
                    if has_waiters {
                        reconnect_signed_in(&state, &ctx).await;
                    }
                    Ok(None)
                })
            }),
        )?;
    }

    // Upstream `pi.on("mcp_servers_change", ...)`: servers registered or
    // unregistered during the session connect or disconnect right away.
    {
        let state = Arc::clone(state);
        pi.on(
            "mcp_servers_change",
            Arc::new(move |_event: &mut Value, ctx: &ExtensionContext| {
                let state = Arc::clone(&state);
                let ctx = ctx.clone();
                Box::pin(async move {
                    on_mcp_servers_change(&state, &ctx).await;
                    Ok(None)
                })
            }),
        )?;
    }

    // Upstream `pi.on("session_shutdown", ...)`.
    {
        let state = Arc::clone(state);
        pi.on(
            "session_shutdown",
            Arc::new(move |_event: &mut Value, _ctx: &ExtensionContext| {
                let state = Arc::clone(&state);
                Box::pin(async move {
                    session_shutdown(&state).await;
                    Ok(None)
                })
            }),
        )?;
    }
    Ok(())
}

fn session_start(state: &SharedState, ctx: &ExtensionContext) {
    let (enabled_count, generation) = {
        let mut guard = lock(state);
        let loaded = match &guard.options.load_config {
            Some(load) => load(ctx),
            None => default_load_config(ctx),
        };
        guard.config_errors = loaded.errors.clone();
        guard.project_config = loaded.project_config.clone();
        guard.auto_enable_codemode = loaded.auto_enable_codemode.unwrap_or(true);
        guard.warned_unreachable = false;
        guard.waited_for_startup = false;
        guard.session_cwd = ctx.cwd().unwrap_or_default();
        guard.model_registry = ctx.model_registry().ok();
        guard.generation += 1;
        guard.session_active = true;
        guard.configured_entries = loaded.servers.clone();
        let registered = registered_servers(&mut guard);
        guard.overridden = registered.overridden;
        guard.servers = loaded
            .servers
            .into_iter()
            .map(|entry| McpServer {
                entry,
                connection: None,
                registered_config: None,
                message: None,
                ready: None,
            })
            .chain(registered.servers)
            .collect();
        emit_change(&mut guard);
        let enabled_count = guard
            .servers
            .iter()
            .filter(|server| is_enabled(server))
            .count();
        (enabled_count, guard.generation)
    };
    // Pisper patch: ensure_discovery_active locks the same SharedState;
    // release this initialization guard before activating discovery tools.
    // The first prompt only waits for directly exposed server tools.
    ensure_discovery_active(state, ctx);
    if enabled_count == 0 {
        report_problems(state, ctx, None);
        return;
    }
    // The MCP client loads only now, so sessions without servers never pay for
    // it. Waiting one event loop turn lets the first render happen before
    // loading and connecting (upstream `setImmediate`).
    let pending_state = Arc::clone(state);
    let pending_ctx = ctx.clone();
    // Populate ready handles before returning so the first prompt can await
    // direct tools even before Tokio has polled the background task.
    let is_current = is_current_generation(state, generation);
    let waits = {
        let mut guard = lock(state);
        let mut waits = Vec::new();
        for index in 0..guard.servers.len() {
            if is_enabled(&guard.servers[index]) {
                waits.push(start_connection(
                    &mut guard,
                    index,
                    Arc::clone(&is_current),
                    None,
                ));
            }
        }
        waits
    };
    let raw: BoxFuture<'static, ()> = Box::pin(async move {
        connect_all(&pending_state, &pending_ctx, generation, waits).await;
    });
    let pending = raw.shared();
    lock(state).pending = Some(pending.clone());
    // Shared futures are lazy: keep the awaitable handle for /mcp while
    // actively driving ordinary session startup without a command invocation.
    tokio::spawn(pending);
}

type IsCurrent = Arc<dyn Fn() -> bool + Send + Sync>;

fn is_current_generation(state: &SharedState, current: u64) -> IsCurrent {
    let state = Arc::clone(state);
    Arc::new(move || lock(&state).generation == current)
}

/// Whether a tool name is one of the resource tools, which reach every
/// server (upstream `RESOURCE_TOOL_NAMES`).
fn resource_tool_name(name: &str) -> bool {
    matches!(
        name,
        super::resources::LIST_MCP_RESOURCES_TOOL
            | super::resources::LIST_MCP_RESOURCE_TEMPLATES_TOOL
            | super::resources::READ_MCP_RESOURCE_TOOL
    )
}

/// Wait for the direct-tools servers that are still connecting, until they
/// settle or [`DEFAULT_STARTUP_WAIT_MS`] passes (upstream
/// `waitForDirectServers`).
async fn wait_for_direct_servers(state: &SharedState, ctx: &ExtensionContext) {
    let ready: Vec<futures::future::Shared<BoxFuture<'static, ()>>> = {
        let mut guard = lock(state);
        if guard.waited_for_startup {
            return;
        }
        guard.waited_for_startup = true;
        guard
            .servers
            .iter()
            .filter(|server| is_enabled(server) && has_direct_tools(&server.entry))
            .filter_map(|server| server.ready.clone())
            .collect()
    };
    if ready.is_empty() {
        return;
    }
    let wait_ms = lock(state).startup_wait_ms;
    let finished = tokio::select! {
        _ = futures::future::join_all(ready) => true,
        _ = tokio::time::sleep(std::time::Duration::from_millis(wait_ms)) => false,
    };
    if !finished {
        notify_ctx(
            ctx,
            "MCP servers are still connecting; their tools become available once connected.",
            Some("info"),
        );
    }
}

/// Wait for servers still connecting, until they settle or `signal` aborts
/// (upstream `waitForServers`).
async fn wait_for_servers(waiting: &[McpServer], signal: Option<Arc<AbortSignal>>) {
    let ready: Vec<futures::future::Shared<BoxFuture<'static, ()>>> = waiting
        .iter()
        .filter_map(|server| server.ready.clone())
        .collect();
    if ready.is_empty() || signal.as_ref().is_some_and(|signal| signal.is_aborted()) {
        return;
    }
    // Upstream races `Promise.all(ready)` against an abort listener; the
    // port races against the signal's cancellation future.
    let abort = async {
        match &signal {
            Some(signal) => signal.cancelled().await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = futures::future::join_all(ready) => {}
        () = abort => {}
    }
}

/// Connect the server in the background. `server.ready` settles when it
/// connected or failed; failures show in its state. It starts after `after`;
/// `is_current` stops it when the session ended meanwhile. Upstream returns
/// the rejecting promise; the statically linked runtime cannot fail to load,
/// so the port's handle never errors.
fn start_connection(
    guard: &mut McpExtensionState,
    server_index: usize,
    is_current: IsCurrent,
    after: Option<futures::future::Shared<BoxFuture<'static, ()>>>,
) -> futures::future::Shared<BoxFuture<'static, ()>> {
    let state = downgrade_state(guard);
    let generation = guard.generation;
    let raw: BoxFuture<'static, ()> = Box::pin(async move {
        if let Some(after) = after {
            after.await;
        }
        if !is_current() {
            return;
        }
        let connection = {
            let Some(state) = state.upgrade() else {
                return;
            };
            let mut guard = lock(&state);
            // Shutdown or reload can invalidate the index after the preceding
            // is_current callback. Check again while holding the state guard.
            if !guard.session_active
                || guard.generation != generation
                || server_index >= guard.servers.len()
            {
                return;
            }
            create_connection(&mut guard, server_index).ok()
        };
        if !is_current() {
            return;
        }
        if let Some(connection) = connection {
            let _ = connection.get_client().await;
        }
    });
    let joined = tokio::spawn(raw);
    let shared: futures::future::Shared<BoxFuture<'static, ()>> = async move {
        let _ = joined.await;
    }
    .boxed()
    .shared();
    guard.servers[server_index].ready = Some(shared.clone());
    shared
}

/// The body of the startup `pending` promise (upstream the
/// `.then(loadMcpRuntime).then(...)` chain): connect every enabled server in
/// the background, then report.
async fn connect_all(
    state: &SharedState,
    ctx: &ExtensionContext,
    current: u64,
    waits: Vec<futures::future::Shared<BoxFuture<'static, ()>>>,
) {
    let is_current = is_current_generation(state, current);
    for wait in waits {
        wait.await;
    }
    if !is_current() {
        return;
    }
    report_problems(state, ctx, None);
}

async fn on_mcp_servers_change(state: &SharedState, ctx: &ExtensionContext) {
    let current = {
        let guard = lock(state);
        if !guard.session_active {
            return;
        }
        guard.generation
    };
    let (removed, added) = {
        let mut guard = lock(state);
        let registered = registered_servers(&mut guard);
        guard.overridden = registered.overridden;
        let next: HashMap<String, Option<String>> = registered
            .servers
            .iter()
            .map(|server| (server.entry.name.clone(), server.registered_config.clone()))
            .collect();
        // Unregistered servers and re-registered ones with a new config are
        // dropped; the latter come back below.
        let removed: Vec<McpServer> = guard
            .servers
            .iter()
            .filter(|server| {
                server.entry.scope == Some(McpConfigScope::Extension)
                    && next.get(&server.entry.name).cloned().flatten().as_deref()
                        != server.registered_config.as_deref()
            })
            .cloned()
            .collect();
        guard.servers.retain(|server| {
            !removed
                .iter()
                .any(|removed| removed.entry.name == server.entry.name)
        });
        for server in &removed {
            hide_tools(&mut guard, &server.entry.name);
        }
        let added: Vec<McpServer> = registered
            .servers
            .into_iter()
            .filter(|server| find_server(&guard, &server.entry.name).is_none())
            .collect();
        guard.servers.extend(added.iter().cloned());
        emit_change(&mut guard);
        (removed, added)
    };
    for server in &removed {
        if let Some(connection) = &server.connection {
            let _ = connection.close().await;
        }
    }
    let connecting: Vec<McpServer> = added.into_iter().filter(is_enabled).collect();
    ensure_discovery_active(state, ctx);
    if lock(state).generation != current || connecting.is_empty() {
        return;
    }
    let is_current = is_current_generation(state, current);
    let mut waits: Vec<futures::future::Shared<BoxFuture<'static, ()>>> = Vec::new();
    for server in &connecting {
        let index = {
            let guard = lock(state);
            guard
                .servers
                .iter()
                .position(|candidate| candidate.entry.name == server.entry.name)
        };
        let Some(index) = index else {
            continue;
        };
        let mut guard = lock(state);
        waits.push(start_connection(
            &mut guard,
            index,
            Arc::clone(&is_current),
            None,
        ));
    }
    for wait in &waits {
        wait.clone().await;
    }
    if !is_current() {
        // The session ended meanwhile: close what connected for it.
        let stale: Vec<Arc<McpServerConnection>> = {
            let guard = lock(state);
            guard
                .servers
                .iter()
                .filter(|server| {
                    connecting
                        .iter()
                        .any(|added| added.entry.name == server.entry.name)
                })
                .filter_map(|server| server.connection.clone())
                .collect()
        };
        for connection in &stale {
            let _ = connection.close().await;
        }
        return;
    }
    report_problems(state, ctx, Some(&connecting));
}

async fn session_shutdown(state: &SharedState) {
    let closing = {
        let mut guard = lock(state);
        guard.session_active = false;
        guard.generation += 1;
        let closing: Vec<Arc<McpServerConnection>> = guard
            .servers
            .iter()
            .filter_map(|server| server.connection.clone())
            .collect();
        guard.servers = Vec::new();
        emit_change(&mut guard);
        closing
    };
    for connection in closing {
        let _ = connection.close().await;
    }
}

// ---------------------------------------------------------------------------------------
// Shared helpers (the factory closure's inner functions)
// ---------------------------------------------------------------------------------------

fn find_server<'a>(guard: &'a McpExtensionState, name: &str) -> Option<&'a McpServer> {
    guard
        .servers
        .iter()
        .find(|server| server.entry.name == name)
}

/// Servers extensions registered, except names `mcp.json` defines, which take
/// precedence (upstream `registeredServers`).
fn registered_servers(guard: &mut McpExtensionState) -> RegisteredServers {
    let mut registered: Vec<McpServer> = Vec::new();
    let mut overridden_names: Vec<String> = Vec::new();
    for server in guard.pi.get_mcp_servers().unwrap_or_default() {
        let configured = guard
            .configured_entries
            .iter()
            .find(|entry| mcp_namespace(&entry.name) == mcp_namespace(&server.name))
            .cloned();
        if let Some(configured) = configured {
            overridden_names.push(format!(
                "\"{}\" registered by {} is overridden by \"{}\" in {}",
                server.name, server.extension_path, configured.name, configured.source
            ));
            continue;
        }
        let registered_config =
            serde_json::to_string(&config_value(&server.config)).unwrap_or_default();
        registered.push(McpServer {
            entry: McpServerEntry {
                name: server.name,
                config: server.config,
                source: server.extension_path,
                scope: Some(McpConfigScope::Extension),
                override_: None,
            },
            connection: None,
            registered_config: Some(registered_config),
            message: None,
            ready: None,
        });
    }
    RegisteredServers {
        servers: registered,
        overridden: overridden_names,
    }
}

struct RegisteredServers {
    servers: Vec<McpServer>,
    overridden: Vec<String>,
}

fn emit_change(guard: &mut McpExtensionState) {
    guard.listeners.retain(|(_, listener)| {
        if let Some(strong) = listener.upgrade() {
            strong();
            true
        } else {
            false
        }
    });
}

/// Create the server's connection (upstream `createConnection`; the runtime is
/// statically linked, so there is no lazy load).
fn create_connection(
    guard: &mut McpExtensionState,
    server_index: usize,
) -> Result<Arc<McpServerConnection>, String> {
    let entry = guard.servers[server_index].entry.clone();
    let cwd = guard.session_cwd.clone();
    let credentials = get_credentials(guard);
    let log = get_server_log(guard);
    let state_for_on_tools = downgrade_state(guard);
    let state_for_change = downgrade_state(guard);
    let create_transport: McpTransportFactory = {
        let custom = guard.options.create_transport.clone();
        Arc::new(move |entry, cwd, auth_provider| match &custom {
            Some(create) => create(entry, cwd, auth_provider),
            None => create_default_transport(entry, cwd, auth_provider),
        })
    };
    let on_tools: Arc<dyn Fn(&McpServerConnection) + Send + Sync> = {
        Arc::new(move |connection: &McpServerConnection| {
            // The runtime hands the same connection object; rebuild the Arc
            // through the state table entry (installed right below).
            if let Some(state) = state_for_on_tools.upgrade() {
                if let Some(connection) = connection_handle(&state, connection) {
                    register_tools(&state, &connection);
                }
            }
        })
    };
    let on_change: Arc<dyn Fn(&McpServerConnection) + Send + Sync> = {
        Arc::new(move |connection: &McpServerConnection| {
            if let Some(state) = state_for_change.upgrade() {
                if let Some(connection) = connection_handle(&state, connection) {
                    on_connection_change(&state, &connection);
                }
            }
        })
    };
    let connection = McpServerConnection::new(McpServerConnectionOptions {
        entry,
        cwd,
        create_transport,
        credentials,
        provider_token: guard.model_registry.clone().map(|registry| {
            let resolver: ProviderTokenResolver = Arc::new(move |provider: &str| {
                let registry = Arc::clone(&registry);
                let provider = provider.to_string();
                Box::pin(async move { registry.get_api_key_for_provider(provider).await })
            });
            resolver
        }),
        on_tools,
        on_change: Some(on_change),
        log: Some(log),
    });
    guard.servers[server_index].connection = Some(Arc::clone(&connection));
    emit_change(guard);
    Ok(connection)
}

/// Recover the `Arc` handle of a connection from the state table (callbacks
/// receive `&McpServerConnection`).
fn connection_handle(
    state: &SharedState,
    connection: &McpServerConnection,
) -> Option<Arc<McpServerConnection>> {
    let guard = lock(state);
    guard
        .servers
        .iter()
        .filter_map(|server| server.connection.clone())
        .find(|candidate| std::ptr::eq(Arc::as_ptr(candidate), std::ptr::from_ref(connection)))
}

fn downgrade_state(guard: &McpExtensionState) -> Weak<Mutex<McpExtensionState>> {
    guard.self_handle.clone()
}

fn get_credentials(guard: &mut McpExtensionState) -> Arc<McpOAuthCredentialStore> {
    if let Some(credentials) = &guard.credentials {
        return Arc::clone(credentials);
    }
    let credentials = guard
        .options
        .credentials
        .clone()
        .unwrap_or_else(|| Arc::new(McpOAuthCredentialStore::new()));
    guard.credentials = Some(Arc::clone(&credentials));
    credentials
}

fn get_server_log(guard: &mut McpExtensionState) -> Arc<McpServerLog> {
    if let Some(log) = &guard.server_log {
        return Arc::clone(log);
    }
    let path = guard
        .options
        .log_path
        .clone()
        .unwrap_or_else(|| path_join(&crate::coding_agent::core::get_agent_dir(), "mcp.log"));
    let log = Arc::new(McpServerLog::new(path));
    guard.server_log = Some(Arc::clone(&log));
    log
}

/// Upstream `onConnectionChange`: track the stored tokens of servers waiting
/// for a sign-in, so a sign-in done by another process is noticed.
fn on_connection_change(state: &SharedState, connection: &Arc<McpServerConnection>) {
    let mut guard = lock(state);
    let id = connection_id(connection);
    if connection.state() != ServerState::NeedsAuth {
        guard.tokens_at_sign_in.remove(&id);
    } else if let std::collections::hash_map::Entry::Vacant(slot) =
        guard.tokens_at_sign_in.entry(id)
    {
        let snapshot = Arc::new(std::sync::Mutex::new(None));
        slot.insert((Arc::clone(connection), Arc::clone(&snapshot)));
        // The snapshot is taken asynchronously (the file-backed store reads
        // under a lock); until it lands the server cannot be mistaken for
        // signed-in.
        let state_for_snapshot = Arc::clone(state);
        let connection_for_snapshot = Arc::clone(connection);
        drop(guard);
        tokio::spawn(async move {
            let tokens = stored_tokens(&state_for_snapshot, &connection_for_snapshot).await;
            if connection_for_snapshot.state() == ServerState::NeedsAuth {
                *snapshot.lock().expect("token snapshot cannot be poisoned") = Some(tokens);
            }
        });
        return;
    }
    emit_change(&mut guard);
}

/// The stored tokens of a connection, as JSON (upstream `storedTokens`).
async fn stored_tokens(state: &SharedState, connection: &Arc<McpServerConnection>) -> String {
    let credentials = lock(state).credentials.clone();
    let Some(url) = connection.oauth_url() else {
        return "null".to_string();
    };
    let Some(credentials) = credentials else {
        return "null".to_string();
    };
    let tokens = credentials.tokens(connection.name(), &url).await;
    serde_json::to_string(&tokens.unwrap_or(Value::Null)).unwrap_or_else(|_| "null".to_string())
}

/// Reconnect servers that need a sign-in when their credentials were stored
/// since (upstream `reconnectSignedIn`).
async fn reconnect_signed_in(state: &SharedState, ctx: &ExtensionContext) {
    let waiting: Vec<(Arc<McpServerConnection>, TokenSnapshot)> = {
        let guard = lock(state);
        guard.tokens_at_sign_in.values().cloned().collect()
    };
    let mut signed_in: Vec<Arc<McpServerConnection>> = Vec::new();
    for (connection, snapshot) in waiting {
        let Some(at_sign_in) = snapshot
            .lock()
            .expect("token snapshot cannot be poisoned")
            .clone()
        else {
            continue;
        };
        if stored_tokens(state, &connection).await != at_sign_in {
            signed_in.push(connection);
        }
    }
    if signed_in.is_empty() {
        return;
    }
    {
        let mut guard = lock(state);
        for connection in &signed_in {
            guard.tokens_at_sign_in.remove(&connection_id(connection));
        }
    }
    for connection in &signed_in {
        let _ = connection.reconnect().await;
    }
    ensure_discovery_active(state, ctx);
}

/// One message for everything that needs the user after startup, or only for
/// `only`, servers that connected later (upstream `reportProblems`).
fn report_problems(state: &SharedState, ctx: &ExtensionContext, only: Option<&[McpServer]>) {
    let mut lines: Vec<String> = Vec::new();
    {
        let guard = lock(state);
        if only.is_none() {
            for error in &guard.config_errors {
                lines.push(format!("config: {error}"));
            }
        }
        for server in only.unwrap_or(&guard.servers) {
            let state = server
                .connection
                .as_ref()
                .map(|connection| connection.state());
            if matches!(
                state,
                Some(ServerState::NeedsAuth) | Some(ServerState::Failed)
            ) {
                lines.push(format!(
                    "{}: {}",
                    server.entry.name,
                    describe_state(server, true)
                ));
            }
        }
    }
    if lines.is_empty() {
        return;
    }
    notify_ctx(
        ctx,
        &format!(
            "MCP servers need attention:\n{}\nRun /mcp to fix.",
            lines
                .iter()
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        Some("warning"),
    );
}

/// Enabled servers with resources whose exposure is not `hidden`, which the
/// resource tools reach (upstream `serversWithResources`).
fn servers_with_resources(guard: &McpExtensionState) -> Vec<McpServer> {
    guard
        .servers
        .iter()
        .filter(|server| {
            server
                .connection
                .as_ref()
                .is_some_and(|connection| connection.has_resources())
                && is_enabled(server)
                && exposure_of(&server.entry) != McpExposure::Hidden
        })
        .cloned()
        .collect()
}

/// The live resource-server handles the resource tools call through (upstream
/// `resourceServers`).
fn resource_servers(guard: &McpExtensionState) -> Vec<Arc<dyn McpResourceServer>> {
    servers_with_resources(guard)
        .into_iter()
        .filter_map(|server| {
            server
                .connection
                .map(|connection| Arc::clone(&connection) as Arc<dyn McpResourceServer>)
        })
        .collect()
}

fn resource_connection_ids(guard: &McpExtensionState) -> Vec<usize> {
    servers_with_resources(guard)
        .into_iter()
        .filter_map(|server| {
            server
                .connection
                .map(|connection| connection_id(&connection))
        })
        .collect()
}

/// Register the server's tools with the extension (upstream `registerTools`).
fn register_tools(state: &SharedState, connection: &Arc<McpServerConnection>) {
    let mut guard = lock(state);
    let server = connection.name().to_string();
    let entry = find_server(&guard, &server)
        .map(|server| server.entry.clone())
        .unwrap_or_else(|| connection.entry.clone());
    let description = entry
        .config
        .description()
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(str::to_string);
    let instructions = connection.instructions();
    let namespace = ToolNamespace {
        name: mcp_namespace(&server),
        description,
        instructions,
    };
    let previous = guard.server_tools.get(&server).cloned().unwrap_or_default();
    let mut current: BTreeSet<String> = BTreeSet::new();
    let tools = connection.tools();
    let connection_id_value = connection_id(connection);
    // Like Codex, all tools whose names sanitize to the same name get the
    // hash suffix, so which one would keep the plain name does not depend on
    // the order of the list.
    let mut plain: Vec<String> = Vec::new();
    let mut seen_plain: std::collections::HashSet<String> = std::collections::HashSet::new();
    for tool in &tools {
        let name = create_mcp_tool_name(&server, &tool.name, |_| false);
        if seen_plain.insert(name.clone()) {
            plain.push(name);
        }
    }
    for tool in &tools {
        let owner = format!("{server}\0{}", tool.name);
        let taken = |candidate: &str| {
            let guard = &guard;
            guard
                .tool_owners
                .get(candidate)
                .map(|existing| existing != &owner)
                .unwrap_or(false)
                || current.contains(candidate)
                || plain.iter().filter(|name| **name == candidate).count() > 1
        };
        let name = create_mcp_tool_name(&server, &tool.name, taken);
        guard.tool_owners.insert(name.clone(), owner);
        current.insert(name.clone());
        let get_client: GetClientHook = {
            let connection = Arc::clone(connection);
            Arc::new(move || {
                let connection = Arc::clone(&connection);
                Box::pin(async move { Ok(Arc::clone(&connection) as Arc<dyn McpToolCaller>) })
            })
        };
        let state_for_readable = Arc::clone(state);
        let readable_connection_id = connection_id_value;
        let readable_resources: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
            let guard = lock(&state_for_readable);
            resource_connection_ids(&guard).contains(&readable_connection_id)
        });
        let definition = create_mcp_tool_definition(super::tools::McpToolDefinitionOptions {
            server: server.clone(),
            tool: tool.clone(),
            name,
            exposure: get_mcp_tool_exposure(&entry.config, &tool.name),
            namespace: namespace.clone(),
            timeout_ms: connection.timeout_ms(),
            get_client,
            readable_resources: Some(readable_resources),
        });
        guard
            .definitions
            .insert(definition.name.clone(), definition.clone());
        let _ = guard.pi.register_tool(definition);
    }
    guard.server_tools.insert(server.clone(), current.clone());
    // Tools cannot be unregistered, so tools the server dropped are
    // re-registered as hidden. When the server offers them again they are
    // registered with their configured exposure above.
    for name in previous {
        if !current.contains(&name) {
            if let Some(definition) = guard.definitions.get(&name) {
                let mut hidden = definition.clone();
                hidden.exposure = ToolExposure::Hidden;
                let _ = guard.pi.register_tool(hidden);
            }
        }
    }
    sync_resource_tools(&mut guard);
}

/// Make a disabled server's tools unreachable (upstream `hideTools`).
fn hide_tools(guard: &mut McpExtensionState, server: &str) {
    let names: Vec<String> = guard
        .server_tools
        .get(server)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    for name in names {
        if let Some(definition) = guard.definitions.get(&name) {
            let mut hidden = definition.clone();
            hidden.exposure = ToolExposure::Hidden;
            let _ = guard.pi.register_tool(hidden);
        }
    }
    guard
        .server_tools
        .insert(server.to_string(), BTreeSet::new());
    sync_resource_tools(guard);
}

/// Register the resource tools with the widest exposure of the servers they
/// reach: `direct` when one of them is direct, and so on. They are hidden when
/// no server has resources (upstream `syncResourceTools`).
fn sync_resource_tools(guard: &mut McpExtensionState) {
    let exposures: Vec<McpExposure> = servers_with_resources(guard)
        .iter()
        .map(|server| exposure_of(&server.entry))
        .collect();
    let exposure = [
        McpExposure::Direct,
        McpExposure::Codemode,
        McpExposure::Deferred,
    ]
    .into_iter()
    .find(|candidate| exposures.contains(candidate));
    let next = exposure.unwrap_or(McpExposure::Hidden);
    if next == guard.resource_tools_exposure.unwrap_or(McpExposure::Hidden)
        && guard.resource_tools_exposure.is_some()
    {
        return;
    }
    if guard.resource_tools_exposure.is_none() && next == McpExposure::Hidden {
        return;
    }
    let was_direct = guard.resource_tools_exposure == Some(McpExposure::Direct);
    guard.resource_tools_exposure = Some(next);
    let state_for_servers = downgrade_state(guard);
    let servers: Arc<dyn Fn() -> Vec<Arc<dyn McpResourceServer>> + Send + Sync> =
        Arc::new(move || match state_for_servers.upgrade() {
            Some(state) => {
                let guard = lock(&state);
                resource_servers(&guard)
            }
            None => Vec::new(),
        });
    let definitions = create_mcp_resource_tool_definitions(McpResourceToolOptions {
        exposure: next,
        servers,
    });
    for definition in &definitions {
        let _ = guard.pi.register_tool(definition.clone());
    }
    if was_direct {
        let names: BTreeSet<String> = definitions
            .iter()
            .map(|definition| definition.name.clone())
            .collect();
        if let Ok(active) = guard.pi.get_active_tools() {
            let filtered: Vec<String> = active
                .into_iter()
                .filter(|name| !names.contains(name))
                .collect();
            let _ = guard.pi.set_active_tools(&filtered);
        }
    }
}

/// Tools that are not declared to the model are reached through the codemode
/// tool (scripts call them) or the tool_search tool (it declares them). Either
/// reaches every such tool. Activate the one the tools' exposure asks for:
/// codemode for `codemode` unless `autoEnableCodemode` is false, tool_search
/// for `deferred` (upstream `ensureDiscoveryActive`). Since v1.0.0 the
/// exposures come from the config, so the tool is active before the servers
/// connect.
fn ensure_discovery_active(state: &SharedState, ctx: &ExtensionContext) {
    let mut guard = lock(state);
    let mut exposures: Vec<McpExposure> = Vec::new();
    for server in &guard.servers {
        if is_enabled(server) {
            for exposure in configured_exposures(&server.entry) {
                if !exposures.contains(&exposure) {
                    exposures.push(exposure);
                }
            }
        }
    }
    let needs_codemode = exposures.contains(&McpExposure::Codemode);
    let needs_tool_search = exposures.contains(&McpExposure::Deferred);
    if !needs_codemode && !needs_tool_search {
        return;
    }
    // Other extensions' tools of the same names cannot reach MCP tools, so
    // never activate them.
    let tools = guard.pi.get_all_tools().unwrap_or_default();
    let has_codemode = tools.iter().any(is_codemode_tool);
    let has_tool_search = tools.iter().any(is_tool_search_tool_info);
    let mut active = guard.pi.get_active_tools().unwrap_or_default();
    let mut activate: Vec<String> = Vec::new();
    if needs_codemode
        && has_codemode
        && guard.auto_enable_codemode
        && !active.iter().any(|name| name == CODEMODE_TOOL_NAME)
    {
        activate.push(CODEMODE_TOOL_NAME.to_string());
    }
    if needs_tool_search
        && has_tool_search
        && !active.iter().any(|name| name == TOOL_SEARCH_TOOL_NAME)
    {
        activate.push(TOOL_SEARCH_TOOL_NAME.to_string());
    }
    if !activate.is_empty() {
        active.extend(activate.iter().cloned());
        let _ = guard.pi.set_active_tools(&active);
    }
    let reachable = |name: &str| active.iter().any(|candidate| candidate == name);
    if has_codemode && reachable(CODEMODE_TOOL_NAME) {
        return;
    }
    if has_tool_search && reachable(TOOL_SEARCH_TOOL_NAME) {
        return;
    }
    if guard.warned_unreachable {
        return;
    }
    guard.warned_unreachable = true;
    let reason = if needs_codemode && has_codemode && !guard.auto_enable_codemode {
        " (autoEnableCodemode is false)"
    } else {
        ""
    };
    notify_ctx(
        ctx,
        &format!(
            "MCP tools are only reachable from the codemode or tool_search tool, but neither is active{reason}; they cannot be called."
        ),
        Some("warning"),
    );
}

/// Upstream `isCodemodeTool` (the codemode extension is its own slice; the
/// detection is the tool name here).
fn is_codemode_tool(info: &ToolInfo) -> bool {
    info.name == CODEMODE_TOOL_NAME
}

fn is_tool_search_tool_info(info: &ToolInfo) -> bool {
    is_tool_search_tool(&info.name, &info.parameters)
}

// ---------------------------------------------------------------------------------------
// The `/mcp` command
// ---------------------------------------------------------------------------------------

fn register_mcp_command(state: &SharedState) -> Result<(), String> {
    let pi = lock(state).pi.clone();
    let command_state = Arc::clone(state);
    let handler: crate::coding_agent::extensions::types::CommandHandler =
        Arc::new(move |args: &str, ctx: &ExtensionCommandContext| {
            let state = Arc::clone(&command_state);
            let args = args.to_string();
            let ctx = ctx.clone();
            Ok(Some(CommandFuture::spawn(async move {
                run_mcp_command_handler(&state, &args, &ctx).await;
                Ok(())
            })?))
        });
    pi.register_command(
        "mcp",
        Some(
            "Manage MCP servers: sign in, reconnect, enable or disable, and change exposure"
                .to_string(),
        ),
        handler,
    )
}

async fn run_mcp_command_handler(state: &SharedState, args: &str, ctx: &ExtensionCommandContext) {
    await_pending(state).await;
    let parts: Vec<&str> = args
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .collect();
    let action = parts.first().copied();
    let name = parts.get(1).copied();
    let extra_len = parts.len().saturating_sub(2);
    let Some(action) = action else {
        let is_tui = ctx
            .mode()
            .map(|mode| mode == ExtensionMode::Tui)
            .unwrap_or(false);
        if is_tui {
            let ui: Arc<dyn McpUi> = Arc::new(manager_ui(ctx));
            manage(state, &ui, &ctx.base).await;
        } else {
            notify_ctx(ctx, &format_status(state), Some("info"));
        }
        return;
    };
    if extra_len > 0 {
        notify_ctx(ctx, MCP_USAGE, Some("warning"));
        return;
    }
    match action {
        "login" => {
            if let Some(server) = pick_server(state, name, ctx, &oauth_pick()).await {
                login_command(state, &server, ctx).await;
            }
        }
        "logout" => {
            if let Some(server) = pick_server(state, name, ctx, &oauth_pick()).await {
                let removed = sign_out(state, &server).await;
                let message = if removed {
                    format!("Signed out of MCP server \"{}\".", server.entry.name)
                } else {
                    format!(
                        "No stored credentials for MCP server \"{}\".",
                        server.entry.name
                    )
                };
                notify_ctx(ctx, &message, Some("info"));
            }
        }
        "reconnect" => {
            if let Some(server) = pick_server(state, name, ctx, &reconnect_pick()).await {
                let failure = reconnect(state, &server).await;
                match failure {
                    Some(failure) => notify_ctx(ctx, &failure, Some("error")),
                    None => {
                        ensure_discovery_active(state, &ctx.base);
                        let message = format!(
                            "Reconnected to MCP server \"{}\" ({}).",
                            server.entry.name,
                            describe_state(&server, true)
                        );
                        notify_ctx(ctx, &message, Some("info"));
                    }
                }
            }
        }
        _ => notify_ctx(ctx, MCP_USAGE, Some("warning")),
    }
}

async fn await_pending(state: &SharedState) {
    let pending = lock(state).pending.clone();
    if let Some(pending) = pending {
        pending.await;
    }
}

/// Eligibility or preference of a server for a `/mcp` subcommand.
type ServerPredicate = Arc<dyn Fn(&McpExtensionState, &McpServer) -> bool + Send + Sync>;

struct PickOptions {
    eligible: ServerPredicate,
    preferred: ServerPredicate,
    none: &'static str,
}

fn uses_oauth_server(_guard: &McpExtensionState, server: &McpServer) -> bool {
    server
        .connection
        .as_ref()
        .and_then(|connection| connection.oauth_url())
        .is_some()
}

fn oauth_pick() -> PickOptions {
    PickOptions {
        eligible: Arc::new(uses_oauth_server),
        preferred: Arc::new(|_guard, server| {
            server
                .connection
                .as_ref()
                .is_some_and(|connection| connection.state() == ServerState::NeedsAuth)
        }),
        none: "No enabled MCP server uses OAuth. Only HTTP servers without an Authorization header do.",
    }
}

fn reconnect_pick() -> PickOptions {
    PickOptions {
        eligible: Arc::new(|_guard, server| server.connection.is_some()),
        preferred: Arc::new(|_guard, server| {
            matches!(
                server
                    .connection
                    .as_ref()
                    .map(|connection| connection.state()),
                Some(ServerState::Failed) | Some(ServerState::Disconnected)
            )
        }),
        none: "No enabled MCP server to reconnect.",
    }
}

/// Resolve the server for a subcommand, asking when the name is omitted and
/// ambiguous (upstream `pickServer`).
async fn pick_server(
    state: &SharedState,
    name: Option<&str>,
    ctx: &ExtensionCommandContext,
    options: &PickOptions,
) -> Option<McpServer> {
    if let Some(name) = name {
        let found = {
            let guard = lock(state);
            find_server(&guard, name).cloned()
        };
        let Some(server) = found else {
            notify_ctx(
                ctx,
                &format!("No MCP server named \"{name}\"."),
                Some("error"),
            );
            return None;
        };
        let eligible = {
            let guard = lock(state);
            (options.eligible)(&guard, &server)
        };
        if !eligible {
            notify_ctx(ctx, options.none, Some("error"));
            return None;
        }
        return Some(server);
    }
    let candidates: Vec<McpServer> = {
        let guard = lock(state);
        guard
            .servers
            .iter()
            .filter(|server| (options.eligible)(&guard, server))
            .cloned()
            .collect()
    };
    if candidates.is_empty() {
        notify_ctx(ctx, options.none, Some("info"));
        return None;
    }
    let preferred_count = {
        let guard = lock(state);
        candidates
            .iter()
            .filter(|server| (options.preferred)(&guard, server))
            .count()
    };
    if candidates.len() == 1 {
        return candidates.into_iter().next();
    }
    if preferred_count == 1 {
        let preferred = {
            let guard = lock(state);
            candidates
                .iter()
                .find(|server| (options.preferred)(&guard, server))
                .cloned()
        };
        if let Some(preferred) = preferred {
            return Some(preferred);
        }
    }
    let choice = ctx
        .ui()
        .ok()?
        .select(
            "MCP server",
            &candidates
                .iter()
                .map(|server| server.entry.name.clone())
                .collect::<Vec<_>>(),
            &Default::default(),
        )
        .await
        .ok()
        .flatten();
    candidates
        .into_iter()
        .find(|server| Some(&server.entry.name) == choice.as_ref())
}

// ---------------------------------------------------------------------------------------
// Status, subcommands (no TUI)
// ---------------------------------------------------------------------------------------

fn format_status(state: &SharedState) -> String {
    let guard = lock(state);
    if guard.servers.is_empty() && guard.config_errors.is_empty() && guard.overridden.is_empty() {
        return format!(
            "No MCP servers configured. Add them to {} or .pi/mcp.json.",
            resolve_agent_mcp_json()
        );
    }
    let mut lines: Vec<String> = Vec::new();
    for server in &guard.servers {
        let name = &server.entry.name;
        let exposure = exposure_name(exposure_of(&server.entry));
        let connection = server.connection.as_ref();
        if let Some(connection) = connection {
            if connection.state() == ServerState::NeedsAuth {
                lines.push(format!(
                    "{name}: needs sign-in, run /mcp login {name} ({exposure})"
                ));
                continue;
            }
        }
        let tools = match connection {
            Some(connection) if connection.state() == ServerState::Connected => {
                format!(", {} tools", connection.tools().len())
            }
            _ => String::new(),
        };
        let state_text = if !is_enabled(server) {
            "disabled".to_string()
        } else {
            match connection {
                Some(connection) if connection.state() == ServerState::Disconnected => {
                    "disconnected, reconnects on next call".to_string()
                }
                Some(connection) => connection.state().as_str().to_string(),
                None => "starting".to_string(),
            }
        };
        let error = match connection {
            Some(connection)
                if connection.error().is_some() && connection.state() != ServerState::Connected =>
            {
                format!(
                    "\n    {}",
                    connection
                        .error()
                        .unwrap_or_default()
                        .split('\n')
                        .collect::<Vec<_>>()
                        .join("\n    ")
                )
            }
            _ => String::new(),
        };
        lines.push(format!("{name}: {state_text}{tools} ({exposure}){error}"));
    }
    for error in &guard.config_errors {
        lines.push(format!("config error: {error}"));
    }
    for line in &guard.overridden {
        lines.push(format!("overridden: {line}"));
    }
    lines.join("\n")
}

fn resolve_agent_mcp_json() -> String {
    path_join(&crate::coding_agent::core::get_agent_dir(), "mcp.json")
}

async fn reconnect(state: &SharedState, server: &McpServer) -> Option<String> {
    let Some(connection) = &server.connection else {
        return Some(format!("MCP server \"{}\" is disabled.", server.entry.name));
    };
    let _ = state;
    connection.reconnect().await.err()
}

async fn sign_out(state: &SharedState, server: &McpServer) -> bool {
    let Some(connection) = &server.connection else {
        return false;
    };
    let Some(url) = connection.oauth_url() else {
        return false;
    };
    let credentials = {
        let mut guard = lock(state);
        get_credentials(&mut guard)
    };
    let removed = credentials.remove(&server.entry.name, &url).await;
    connection.sign_out().await;
    removed
}

/// Save a config change; returns an error message when the file could not be
/// updated. Changes to registered servers only apply to the current session;
/// `in_project` adds a project override (upstream `saveConfig`). Takes the
/// caller-held state guard's update hook; never locks the state itself.
fn save_config_with(
    update: Option<UpdateConfigHook>,
    project_config: Option<String>,
    server: &mut McpServer,
    patch: McpServerConfigPatch,
    in_project: bool,
) -> Option<String> {
    let override_path = if in_project {
        project_config
    } else {
        server.entry.override_.clone()
    };
    let target = override_path
        .clone()
        .unwrap_or_else(|| server.entry.source.clone());
    let previous_override = server.entry.override_.clone();
    server.entry.override_ = override_path;
    if server.entry.scope != Some(McpConfigScope::Extension) {
        let result = match &update {
            Some(update) => update(&server.entry, patch),
            None => update_mcp_server_config(
                &target,
                &server.entry.name,
                patch,
                previous_override.is_some(),
            ),
        };
        if let Err(error) = result {
            server.entry.override_ = previous_override;
            return Some(format!("Could not update {target}: {error}"));
        }
    }
    server.entry = merged_entry(&server.entry, patch);
    None
}

/// `{ ...entry, config: { ...config, ...patch } }` through revalidation (the
/// upstream spread keeps key order; [`validate_mcp_server_config`] does the
/// same).
fn merged_entry(entry: &McpServerEntry, patch: McpServerConfigPatch) -> McpServerEntry {
    let mut raw = match config_value(&entry.config) {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    if let Some(enabled) = patch.enabled {
        raw.insert("enabled".to_string(), Value::Bool(enabled));
    }
    if let Some(exposure) = patch.exposure {
        raw.insert(
            "exposure".to_string(),
            Value::from(exposure_json_name(exposure)),
        );
    }
    let ordered = crate::ai::types::ordered_map::OrderedMap::from_pairs(raw);
    let config =
        validate_mcp_server_config(&entry.name, &ordered).unwrap_or_else(|_| entry.config.clone());
    McpServerEntry {
        name: entry.name.clone(),
        config,
        source: entry.source.clone(),
        scope: entry.scope,
        override_: entry.override_.clone(),
    }
}

async fn set_enabled(
    state: &SharedState,
    server_name: &str,
    enabled: bool,
    in_project: bool,
) -> Option<String> {
    let (failed, old_connection) = {
        let mut guard = lock(state);
        let index = guard
            .servers
            .iter()
            .position(|server| server.entry.name == server_name)?;
        let update = guard.options.update_config.clone();
        let project_config = guard.project_config.clone();
        let failed = save_config_with(
            update,
            project_config,
            &mut guard.servers[index],
            McpServerConfigPatch {
                enabled: Some(enabled),
                exposure: None,
            },
            in_project,
        );
        let old_connection = guard.servers[index].connection.clone();
        if !enabled {
            guard.servers[index].connection = None;
            hide_tools(&mut guard, server_name);
            emit_change(&mut guard);
        }
        (failed, old_connection)
    };
    if failed.is_some() {
        return failed;
    }
    if !enabled {
        if let Some(connection) = old_connection {
            let _ = connection.close().await;
        }
        return None;
    }
    let wait = {
        let mut guard = lock(state);
        let index = guard
            .servers
            .iter()
            .position(|server| server.entry.name == server_name)?;
        start_connection(&mut guard, index, Arc::new(|| true), None)
    };
    wait.await;
    None
}

fn set_exposure(state: &SharedState, server_name: &str, exposure: McpExposure) -> Option<String> {
    let failed = {
        let mut guard = lock(state);
        let index = guard
            .servers
            .iter()
            .position(|server| server.entry.name == server_name)?;
        let update = guard.options.update_config.clone();
        let project_config = guard.project_config.clone();
        save_config_with(
            update,
            project_config,
            &mut guard.servers[index],
            McpServerConfigPatch {
                enabled: None,
                exposure: Some(exposure),
            },
            false,
        )
    };
    if failed.is_some() {
        return failed;
    }
    let connected = {
        let guard = lock(state);
        guard
            .servers
            .iter()
            .find(|server| server.entry.name == server_name)
            .and_then(|server| server.connection.clone())
            .filter(|connection| connection.state() == ServerState::Connected)
    };
    // Re-register the connected server's tools with the new exposure (the
    // registration locks the state itself, so it runs outside the guard).
    if let Some(connection) = connected {
        register_tools(state, &connection);
    }
    let mut guard = lock(state);
    sync_resource_tools(&mut guard);
    // Tools no longer exposed directly leave the declared set; direct tools
    // are activated on registration.
    let indirect: BTreeSet<String> = guard
        .pi
        .get_all_tools()
        .unwrap_or_default()
        .into_iter()
        .filter(|tool| tool.exposure != ToolExposure::Direct)
        .map(|tool| tool.name)
        .collect();
    let tools = guard
        .server_tools
        .get(server_name)
        .cloned()
        .unwrap_or_default();
    if let Ok(active) = guard.pi.get_active_tools() {
        let filtered: Vec<String> = active
            .into_iter()
            .filter(|name| !tools.contains(name) || !indirect.contains(name))
            .collect();
        let _ = guard.pi.set_active_tools(&filtered);
    }
    emit_change(&mut guard);
    None
}

async fn sign_in(
    state: &SharedState,
    server: &McpServer,
    prompt: Arc<dyn McpSignInPrompt>,
) -> Option<String> {
    let connection = server.connection.clone();
    let url = connection
        .as_ref()
        .and_then(|connection| connection.oauth_url());
    let (Some(connection), Some(url)) = (connection, url) else {
        return Some(format!(
            "MCP server \"{}\" does not use OAuth.",
            server.entry.name
        ));
    };
    let (store, settings, challenge) = {
        let mut guard = lock(state);
        let credentials = get_credentials(&mut guard);
        (
            credentials.for_server(&server.entry.name, &url),
            connection.oauth_settings(),
            connection.challenge(),
        )
    };
    if let Err(error) = sign_in_mcp_server(&url, store, settings, challenge, prompt).await {
        return Some(match error {
            McpSignInError::Cancelled => "Sign-in cancelled.".to_string(),
            McpSignInError::Failed(message) => format!("Sign-in failed: {message}"),
        });
    }
    // The challenge that asked for this sign-in (for example for more scope)
    // is answered.
    connection.clear_challenge();
    if let Err(error) = connection.reconnect().await {
        return Some(format!("Signed in, but {error}"));
    }
    None
}

/// Build the sign-in prompt for a command context (upstream the
/// `loginCommand` prompt object).
fn command_sign_in_prompt(
    ctx: &ExtensionCommandContext,
    name: &str,
    open_url: Arc<dyn Fn(&str) + Send + Sync>,
) -> Arc<dyn McpSignInPrompt> {
    Arc::new(CommandSignInPrompt {
        ctx: ctx.clone(),
        name: name.to_string(),
        open_url,
    })
}

struct CommandSignInPrompt {
    ctx: ExtensionCommandContext,
    name: String,
    open_url: Arc<dyn Fn(&str) + Send + Sync>,
}

impl McpSignInPrompt for CommandSignInPrompt {
    fn show_authorization_url(&self, url: &url::Url) {
        notify_ctx(
            &self.ctx,
            &format!(
                "Sign in to MCP server \"{}\" in your browser:\n{}",
                self.name,
                url.as_str()
            ),
            Some("info"),
        );
        (self.open_url)(url.as_str());
    }

    fn prompt_for_redirect_url<'a>(
        &'a self,
        signal: Arc<AbortSignal>,
    ) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let Ok(ui) = self.ctx.ui() else {
                return None;
            };
            ui.input(
                &format!(
                    "Waiting for sign-in to \"{}\". If the browser cannot reach this machine, paste the URL it was redirected to.",
                    self.name
                ),
                Some("http://127.0.0.1:.../callback?code=..."),
                &crate::coding_agent::extensions::types::ExtensionUiDialogOptions {
                    signal: Some(signal),
                    timeout: None,
                },
            )
            .await
            .ok()
            .flatten()
        })
    }
}

async fn login_command(state: &SharedState, server: &McpServer, ctx: &ExtensionCommandContext) {
    let name = server.entry.name.clone();
    if !ctx.has_ui().unwrap_or(false) {
        notify_ctx(
            ctx,
            &format!("Signing in to MCP server \"{name}\" requires interactive mode."),
            Some("error"),
        );
        return;
    }
    // v1.0.0: in the TUI the sign-in runs through the manager view, which
    // shows the URL with a copy key (upstream `showMcpManager`); elsewhere the
    // URL is announced as a notification.
    let failure = if ctx
        .mode()
        .map(|mode| mode == ExtensionMode::Tui)
        .unwrap_or(false)
    {
        let ui: Arc<dyn McpUi> = Arc::new(manager_ui(ctx));
        sign_in_with_ui(state, &ui, server, &name).await
    } else {
        let open_url = {
            let guard = lock(state);
            guard
                .options
                .open_url
                .clone()
                .unwrap_or_else(|| Arc::new(open_browser))
        };
        let prompt = command_sign_in_prompt(ctx, &name, open_url);
        sign_in(state, server, prompt).await
    };
    if let Some(failure) = failure {
        let notify_type = if failure == "Sign-in cancelled." {
            "info"
        } else {
            "error"
        };
        notify_ctx(ctx, &failure, Some(notify_type));
        return;
    }
    ensure_discovery_active(state, &ctx.base);
    let tools = {
        let guard = lock(state);
        find_server(&guard, &name)
            .and_then(|server| server.connection.as_ref())
            .map(|connection| connection.tools().len())
            .unwrap_or(0)
    };
    notify_ctx(
        ctx,
        &format!("Signed in to MCP server \"{name}\" ({tools} tools)."),
        Some("info"),
    );
}

// ---------------------------------------------------------------------------------------
// Manager (`/mcp` in the TUI)
// ---------------------------------------------------------------------------------------

fn servers_menu(state: &SharedState) -> McpMenu {
    let guard = lock(state);
    let mut sorted = guard.servers.clone();
    sorted.sort_by(|a, b| {
        attention_rank(a)
            .cmp(&attention_rank(b))
            .then_with(|| locale_compare(&a.entry.name, &b.entry.name))
    });
    McpMenu {
        title: "MCP servers".to_string(),
        error: {
            let joined = {
                let guard = &guard;
                guard
                    .config_errors
                    .iter()
                    .map(|error| format!("config: {error}"))
                    .chain(
                        guard
                            .overridden
                            .iter()
                            .map(|line| format!("overridden: {line}")),
                    )
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            (!joined.is_empty()).then_some(joined)
        },
        items: sorted
            .iter()
            .map(|server| McpMenuItem {
                value: server.entry.name.clone(),
                label: server.entry.name.clone(),
                description: Some(format!(
                    "{} · {} · {}",
                    describe_state(server, true),
                    exposure_name(exposure_of(&server.entry)),
                    if server.entry.override_.is_some() {
                        "global, project override".to_string()
                    } else {
                        server
                            .entry
                            .scope
                            .map(|scope| scope.as_str().to_string())
                            .unwrap_or_else(|| server.entry.source.clone())
                    }
                )),
            })
            .collect(),
        empty: Some(format!(
            "No MCP servers configured. Add them to {} or .pi/mcp.json.",
            resolve_agent_mcp_json()
        )),
        selected: None,
        confirm_label: "manage".to_string(),
        cancel_label: "close".to_string(),
        details: None,
    }
}

fn server_menu(state: &SharedState, name: &str) -> McpMenu {
    let guard = lock(state);
    let Some(server) = find_server(&guard, name) else {
        return McpMenu {
            title: name.to_string(),
            items: Vec::new(),
            empty: Some("This server is no longer configured.".to_string()),
            confirm_label: String::new(),
            cancel_label: "back".to_string(),
            ..McpMenu::default()
        };
    };
    let entry = &server.entry;
    let connection = server.connection.as_ref();
    let saved = match entry.scope {
        Some(McpConfigScope::Extension) => "for this session".to_string(),
        _ if entry.override_.is_some() => "saved to the project mcp.json".to_string(),
        Some(scope) => format!("saved to the {} mcp.json", scope.as_str()),
        None => "saved to mcp.json".to_string(),
    };
    // Global servers without an override can be turned on or off for the
    // trusted project alone.
    let in_project = entry.scope == Some(McpConfigScope::Global)
        && entry.override_.is_none()
        && guard.project_config.is_some();
    let in_project_saved = "saved to the project mcp.json".to_string();
    let mut items: Vec<McpMenuItem> = Vec::new();
    if !is_enabled(server) {
        items.push(McpMenuItem {
            value: "enable".to_string(),
            label: "Enable".to_string(),
            description: Some(saved.clone()),
        });
        if in_project {
            items.push(McpMenuItem {
                value: "enable-project".to_string(),
                label: "Enable in this project".to_string(),
                description: Some(in_project_saved),
            });
        }
    } else {
        let state = connection.map(|connection| connection.state());
        if state == Some(ServerState::NeedsAuth) {
            items.push(McpMenuItem {
                value: "signin".to_string(),
                label: "Sign in".to_string(),
                description: Some("opens the browser".to_string()),
            });
        }
        if state == Some(ServerState::Connected) {
            if let Some(connection) = connection {
                items.push(McpMenuItem {
                    value: "tools".to_string(),
                    label: "Tools".to_string(),
                    description: Some(format!("{} offered", connection.tools().len())),
                });
            }
        }
        if matches!(
            state,
            Some(ServerState::Failed)
                | Some(ServerState::Disconnected)
                | Some(ServerState::Connected)
                | Some(ServerState::NeedsAuth)
        ) {
            items.push(McpMenuItem {
                value: "reconnect".to_string(),
                label: "Reconnect".to_string(),
                description: None,
            });
        }
        if state == Some(ServerState::Connected) && connection.and_then(|c| c.oauth_url()).is_some()
        {
            items.push(McpMenuItem {
                value: "signout".to_string(),
                label: "Sign out".to_string(),
                description: Some("deletes the stored credentials".to_string()),
            });
        }
        items.push(McpMenuItem {
            value: "exposure".to_string(),
            label: "Exposure".to_string(),
            description: Some(exposure_name(exposure_of(entry)).to_string()),
        });
        items.push(McpMenuItem {
            value: "disable".to_string(),
            label: "Disable".to_string(),
            description: Some(saved),
        });
        if in_project {
            items.push(McpMenuItem {
                value: "disable-project".to_string(),
                label: "Disable in this project".to_string(),
                description: Some(in_project_saved),
            });
        }
    }
    let mut details_rows: Vec<String> = vec![
        describe_transport(entry),
        format!(
            "{}: {}",
            entry
                .scope
                .map(|scope| scope.as_str().to_string())
                .unwrap_or_else(|| "config".to_string()),
            entry.source
        ),
    ];
    if let Some(override_path) = &entry.override_ {
        details_rows.push(format!("project override: {override_path}"));
    }
    details_rows.push(format!("State: {}", describe_state(server, false)));
    let details = details_rows.join("\n");
    let error = [
        server.message.clone(),
        match connection {
            Some(connection) if connection.state() != ServerState::Connected => connection.error(),
            _ => None,
        },
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    McpMenu {
        title: format!("MCP server {name}"),
        details: Some(details),
        error: (!error.is_empty()).then_some(error),
        selected: items.first().map(|item| item.value.clone()),
        items,
        confirm_label: "select".to_string(),
        cancel_label: "back".to_string(),
        empty: None,
    }
}

async fn choose_exposure(state: &SharedState, ui: &dyn McpUi, server_name: &str) -> Option<String> {
    let (current, is_extension, saved_to) = {
        let guard = lock(state);
        let server = find_server(&guard, server_name)?;
        (
            exposure_of(&server.entry),
            server.entry.scope == Some(McpConfigScope::Extension),
            server
                .entry
                .override_
                .clone()
                .unwrap_or_else(|| server.entry.source.clone()),
        )
    };
    let current_name = exposure_name(current).to_string();
    let build: MenuBuilder = {
        let server_name = server_name.to_string();
        let closure_current = current_name.clone();
        Arc::new(move || {
            let current_name = &closure_current;
            let mut items: Vec<McpMenuItem> = Vec::new();
            for (name, description) in EXPOSURE_DESCRIPTIONS {
                items.push(McpMenuItem {
                    value: name.to_string(),
                    label: format!("{}{name}", if name == *current_name { "✓ " } else { "  " }),
                    description: Some(description.to_string()),
                });
            }
            McpMenu {
                title: format!("Exposure of {server_name}"),
                details: Some(if is_extension {
                    format!("Applies to this session; the server is registered by {saved_to}.")
                } else {
                    format!("Saved to {saved_to}.")
                }),
                items,
                selected: Some(current_name.clone()),
                confirm_label: "save".to_string(),
                cancel_label: "back".to_string(),
                empty: None,
                error: None,
            }
        })
    };
    let choice = ui.menu(build, None).await?;
    if choice == current_name {
        return None;
    }
    let exposure = match choice.as_str() {
        "codemode" => McpExposure::Codemode,
        "deferred" => McpExposure::Deferred,
        "direct" => McpExposure::Direct,
        _ => McpExposure::Hidden,
    };
    set_exposure(state, server_name, exposure)
}

async fn run_action(
    state: &SharedState,
    ui: &Arc<dyn McpUi>,
    ctx: &ExtensionContext,
    server_name: &str,
    action: &str,
) {
    let mut message: Option<String> = None;
    match action {
        "signin" => {
            let server = {
                let guard = lock(state);
                find_server(&guard, server_name).cloned()
            };
            if let Some(server) = server {
                message = sign_in_with_ui(state, ui, &server, &server.entry.name).await;
            }
        }
        "reconnect" => {
            // A failure shows as the connection's state and error.
            ui.status(&format!("MCP server {server_name}"), "Reconnecting…");
            let server = {
                let guard = lock(state);
                find_server(&guard, server_name).cloned()
            };
            if let Some(server) = server {
                reconnect(state, &server).await;
            }
        }
        "signout" => {
            let server = {
                let guard = lock(state);
                find_server(&guard, server_name).cloned()
            };
            if let Some(server) = server {
                sign_out(state, &server).await;
            }
        }
        "tools" => {
            let build: MenuBuilder = {
                let state = Arc::clone(state);
                let server_name = server_name.to_string();
                Arc::new(move || {
                    let guard = lock(&state);
                    match find_server(&guard, &server_name) {
                        Some(server) => tools_menu_data(&guard, server),
                        None => McpMenu {
                            title: format!("Tools of {server_name}"),
                            items: Vec::new(),
                            empty: Some("The server offers no tools.".to_string()),
                            confirm_label: "back".to_string(),
                            cancel_label: "back".to_string(),
                            ..McpMenu::default()
                        },
                    }
                })
            };
            ui.menu(build, None).await;
        }
        "exposure" => {
            message = choose_exposure(state, ui.as_ref(), server_name).await;
        }
        "enable" | "disable" | "enable-project" | "disable-project" => {
            let enable = action.starts_with("enable");
            ui.status(
                &format!("MCP server {server_name}"),
                if enable {
                    "Connecting…"
                } else {
                    "Disconnecting…"
                },
            );
            message = set_enabled(state, server_name, enable, action.ends_with("-project")).await;
        }
        _ => {}
    }
    {
        let mut guard = lock(state);
        if let Some(index) = guard
            .servers
            .iter_mut()
            .position(|candidate| candidate.entry.name == server_name)
        {
            guard.servers[index].message = message;
        }
    }
    ensure_discovery_active(state, ctx);
    emit_change(&mut lock(state));
}

/// The data of the tools-of-server menu (upstream the `showTools` builder).
fn tools_menu_data(_guard: &McpExtensionState, server: &McpServer) -> McpMenu {
    let exposure = exposure_of(&server.entry);
    let overridden = !server.entry.config.tool_exposure().is_empty();
    let exposure_text = if exposure == McpExposure::Hidden {
        "unreachable".to_string()
    } else {
        exposure_description(exposure)
            .unwrap_or("unreachable")
            .to_string()
    };
    let tools = server
        .connection
        .as_ref()
        .map(|connection| connection.tools())
        .unwrap_or_default();
    McpMenu {
        title: format!("Tools of {}", server.entry.name),
        details: Some(format!(
            "Exposure {}: {}{}",
            exposure_name(exposure),
            exposure_text,
            if overridden {
                "\nSome tools override it with toolExposure."
            } else {
                ""
            }
        )),
        items: tools
            .iter()
            .map(|tool| {
                let tool_exposure = get_mcp_tool_exposure(&server.entry.config, &tool.name);
                let description = first_line(tool.description.as_deref().unwrap_or(""));
                McpMenuItem {
                    value: tool.name.clone(),
                    label: tool.name.clone(),
                    description: Some(if tool_exposure == exposure {
                        description.to_string()
                    } else {
                        format!("[{}] {description}", exposure_name(tool_exposure))
                    }),
                }
            })
            .collect(),
        empty: Some("The server offers no tools.".to_string()),
        confirm_label: "back".to_string(),
        cancel_label: "back".to_string(),
        ..McpMenu::default()
    }
}

/// Sign in with the manager view's sign-in screen, which shows the URL with a
/// copy key (upstream `signInWithUi`).
async fn sign_in_with_ui(
    state: &SharedState,
    ui: &Arc<dyn McpUi>,
    server: &McpServer,
    name: &str,
) -> Option<String> {
    let title = format!("Sign in to {name}");
    ui.status(&title, "Contacting the authorization server…");
    let open_url = {
        let guard = lock(state);
        guard
            .options
            .open_url
            .clone()
            .unwrap_or_else(|| Arc::new(open_browser))
    };
    let prompt = Arc::new(ManagerSignInPrompt {
        title: title.clone(),
        ui: Arc::clone(ui),
        authorization_url: Mutex::new(String::new()),
        open_url,
    });
    sign_in(state, server, prompt).await
}

/// The manager's sign-in prompt (upstream the `runAction` "signin" prompt):
/// shows the authorization URL, opens the browser, then asks for a pasted
/// redirect URL through the manager UI.
struct ManagerSignInPrompt {
    title: String,
    ui: Arc<dyn McpUi>,
    authorization_url: Mutex<String>,
    open_url: Arc<dyn Fn(&str) + Send + Sync>,
}

impl McpSignInPrompt for ManagerSignInPrompt {
    fn show_authorization_url(&self, url: &url::Url) {
        *self
            .authorization_url
            .lock()
            .expect("authorization url cannot be poisoned") = url.as_str().to_string();
        (self.open_url)(url.as_str());
    }

    fn prompt_for_redirect_url<'a>(
        &'a self,
        signal: Arc<AbortSignal>,
    ) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let authorization_url = self
                .authorization_url
                .lock()
                .expect("authorization url cannot be poisoned")
                .clone();
            let value = self
                .ui
                .redirect_url(&self.title, &authorization_url, signal)
                .await;
            self.ui.status(&self.title, "Connecting…");
            value
        })
    }
}

/// Run the manager loop until it returns (upstream `manage`).
async fn manage(state: &SharedState, ui: &Arc<dyn McpUi>, ctx: &ExtensionContext) {
    loop {
        let build: MenuBuilder = {
            let state = Arc::clone(state);
            Arc::new(move || servers_menu(&state))
        };
        let subscribe: Box<dyn Fn(MenuListener) -> MenuUnsubscribe + Send + Sync> = {
            let state = Arc::clone(state);
            Box::new(move |listener: MenuListener| {
                let mut guard = lock(&state);
                let id = guard.next_listener_id;
                guard.next_listener_id += 1;
                guard.listeners.push((id, Arc::downgrade(&listener)));
                let state = Arc::clone(&state);
                Arc::new(move || {
                    let mut guard = lock(&state);
                    guard.listeners.retain(|(existing, _)| *existing != id);
                }) as MenuUnsubscribe
            })
        };
        let name = ui.menu(build, Some(subscribe)).await;
        let Some(name) = name else {
            return;
        };
        loop {
            let build: MenuBuilder = {
                let state = Arc::clone(state);
                let name = name.clone();
                Arc::new(move || server_menu(&state, &name))
            };
            let action = ui.menu(build, None).await;
            let server_exists = {
                let guard = lock(state);
                find_server(&guard, &name).is_some()
            };
            let Some(action) = action else {
                break;
            };
            if !server_exists {
                break;
            }
            run_action(state, ui, ctx, &name, &action).await;
        }
    }
}

/// The manager UI for a command context: the port's dialog adapter over the
/// runner UI handle (see [`super::ui`] for the cropped TUI view disclosure).
fn manager_ui(ctx: &ExtensionCommandContext) -> super::ui::DialogMcpUi {
    // The UiHandle is not Clone; the hooks re-resolve it per call through the
    // command context.
    let ctx = ctx.clone();
    let select = {
        let ctx = ctx.clone();
        Arc::new(
            move |title: String, options: Vec<String>| -> BoxFuture<'static, Option<String>> {
                let ctx = ctx.clone();
                Box::pin(async move {
                    let Ok(ui) = ctx.ui() else { return None };
                    ui.select(&title, &options, &Default::default())
                        .await
                        .ok()
                        .flatten()
                })
            },
        )
    };
    let input = {
        let ctx = ctx.clone();
        Arc::new(
            move |title: String,
                  placeholder: Option<String>,
                  signal: Arc<AbortSignal>|
                  -> BoxFuture<'static, Option<String>> {
                let ctx = ctx.clone();
                Box::pin(async move {
                    let Ok(ui) = ctx.ui() else { return None };
                    ui.input(
                        &title,
                        placeholder.as_deref(),
                        &crate::coding_agent::extensions::types::ExtensionUiDialogOptions {
                            signal: Some(signal),
                            timeout: None,
                        },
                    )
                    .await
                    .ok()
                    .flatten()
                })
            },
        )
    };
    let notify = {
        let ctx = ctx.clone();
        Arc::new(move |message: &str| {
            let ctx = ctx.clone();
            if let Ok(ui) = ctx.ui() {
                ui.notify(message, Some("info"));
            }
        })
    };
    let status = {
        let ctx = ctx.clone();
        Arc::new(move |title: &str, message: &str| {
            let ctx = ctx.clone();
            if let Ok(ui) = ctx.ui() {
                ui.set_status("mcp", Some(&format!("{title}: {message}")));
            }
        })
    };
    super::ui::DialogMcpUi::new(super::ui::DialogMcpUiHooks {
        select,
        input,
        notify,
        status,
    })
}
