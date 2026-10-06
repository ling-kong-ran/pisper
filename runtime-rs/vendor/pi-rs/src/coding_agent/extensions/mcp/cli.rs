//! Port of upstream `coding-agent/src/extensions/mcp/cli.ts` (HEAD
//! `2bbfcca43`): `pi mcp` — add, remove, and check MCP servers and sign in to
//! them outside a session. Agents run it through bash to configure servers,
//! verify an `mcp.json` they wrote, and start an OAuth sign-in; the user only
//! approves access in the browser. Running sessions pick up new credentials on
//! their next turn.
//!
//! Disclosed seams:
//! - **chalk styling**: the help text uses chalk `bold`/`dim` upstream; the
//!   port emits the plain text (identical when chalk's color detection is off,
//!   which is the oracle's and piped-output case).
//! - **stdin reading** for pasted redirect URLs uses Tokio's stdin line reader
//!   (upstream `node:readline/promises`); the prompt text and timeout
//!   cancellation are verbatim.

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::{json, Map, Value};

use crate::ai::types::ordered_map::OrderedMap;
use crate::coding_agent::core::mcp_servers::validate_mcp_server_config;
use crate::coding_agent::core::path_join;
use crate::coding_agent::core::trust_manager::ProjectTrustStore;
use crate::coding_agent::core::CONFIG_DIR_NAME;
use crate::coding_agent::extensions::mcp::config::{
    add_mcp_server_config, load_mcp_config, remove_mcp_server_config, LoadedMcpConfig,
    LoadedMcpConfigOptions, McpServerEntry,
};
use crate::coding_agent::extensions::mcp::oauth::{
    sign_in_mcp_server, McpOAuthCredentialStore, McpSignInError, McpSignInPrompt,
};
use crate::coding_agent::extensions::mcp::open_browser;
use crate::coding_agent::extensions::mcp::runtime::{
    create_default_transport, McpServerConnection, McpServerConnectionOptions, McpServerLog,
    ServerState,
};

pub const APP_NAME: &str = "pi";

fn bold(text: &str) -> String {
    text.to_string()
}

fn dim(text: &str) -> String {
    text.to_string()
}

fn help_text() -> String {
    format!(
        "{bold_usage}\n  {APP_NAME} mcp add <server> [options] -- <command> [args...]\n  {APP_NAME} mcp add <server> [options] --url <url>\n  {APP_NAME} mcp remove <server> [-l]\n  {APP_NAME} mcp list [--json]\n  {APP_NAME} mcp login <server> [--timeout <seconds>]\n  {APP_NAME} mcp logout <server>\n\nConfigure and check MCP servers and sign in to OAuth servers without starting a session.\nReads ~/{CONFIG_DIR_NAME}/agent/mcp.json and, in trusted projects, {CONFIG_DIR_NAME}/mcp.json.\n\nCommands:\n  add <server>            Add or replace a server in mcp.json\n  remove <server>         Remove a server from mcp.json\n  list                    Show state, tools, and errors (exits 1 on failure)\n  login <server>          Sign in through the browser\n  logout <server>         Delete the stored OAuth credentials\n\nOptions for add and remove:\n  -l, --local             Use {CONFIG_DIR_NAME}/mcp.json in the current project instead of the global file\n\nOptions for add:\n  --url <url>             Streamable HTTP server URL (instead of a command)\n  --env <KEY=VALUE>       Environment variable for a stdio server (repeatable)\n  --cwd <dir>             Working directory for a stdio server\n  --header <KEY=VALUE>    HTTP header (repeatable)\n  --bearer-token-env-var <NAME>\n                          Send \"Authorization: Bearer ${{NAME}}\"\n  --oauth-client-id <id>  Pre-registered OAuth client id\n  --oauth-client-secret <secret>\n                          OAuth client secret (may be ${{NAME}} or !command)\n  --oauth-callback-port <port>\n                          Fixed OAuth callback port\n  --oauth-client-name <name>\n                          Client name sent when registering with the OAuth server\n  --exposure <mode>       codemode (default), deferred, direct, or hidden\n  --description <text>    What the server offers, shown to the model with its tools\n\nOther options:\n  --json                  Print the list as JSON\n  --timeout <seconds>     How long login waits for the browser (default: 300)",
        bold_usage = bold("Usage:"),
    )
}

fn help_hint() -> String {
    dim(&format!("Use \"{APP_NAME} mcp --help\" for usage."))
}

const DEFAULT_LOGIN_TIMEOUT_SECONDS: f64 = 300.0;

/// Upstream `McpCommandOptions`.
#[derive(Clone)]
pub struct McpCommandOptions {
    pub cwd: String,
    pub agent_dir: String,
    /// Defaults to `mcp-auth.json` in the agent directory.
    pub credentials: Option<Arc<McpOAuthCredentialStore>>,
    /// Defaults to the platform browser.
    pub open_url: Option<OpenUrlHook>,
    /// Defaults to console output.
    pub log: Option<OutputHook>,
    pub error: Option<OutputHook>,
}

/// Upstream `log?`/`error?` console sinks.
pub type OpenUrlHook = Arc<dyn Fn(&str) + Send + Sync>;
pub type OutputHook = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
struct ServerReport {
    name: String,
    scope: String,
    source: String,
    enabled: bool,
    exposure: String,
    transport: String,
    state: String,
    tools: Vec<String>,
    /// Tools whose exposure differs from the server's, from `toolExposure`.
    tool_exposure: Map<String, Value>,
    resources: Option<f64>,
    resource_templates: Option<f64>,
    error: Option<String>,
}

impl ServerReport {
    fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("name".into(), Value::from(self.name.clone()));
        map.insert("scope".into(), Value::from(self.scope.clone()));
        map.insert("source".into(), Value::from(self.source.clone()));
        map.insert("enabled".into(), Value::Bool(self.enabled));
        map.insert("exposure".into(), Value::from(self.exposure.clone()));
        map.insert("transport".into(), Value::from(self.transport.clone()));
        map.insert("state".into(), Value::from(self.state.clone()));
        map.insert(
            "tools".into(),
            Value::Array(self.tools.iter().cloned().map(Value::from).collect()),
        );
        if !self.tool_exposure.is_empty() {
            map.insert(
                "toolExposure".into(),
                Value::Object(self.tool_exposure.clone()),
            );
        }
        if let Some(resources) = self.resources {
            map.insert("resources".into(), json!(resources));
        }
        if let Some(templates) = self.resource_templates {
            map.insert("resourceTemplates".into(), json!(templates));
        }
        if let Some(error) = &self.error {
            map.insert("error".into(), Value::from(error.clone()));
        }
        Value::Object(map)
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

fn create_connection(
    entry: McpServerEntry,
    options: &McpCommandOptions,
    credentials: Arc<McpOAuthCredentialStore>,
) -> Arc<McpServerConnection> {
    let log = Arc::new(McpServerLog::new(path_join(&options.agent_dir, "mcp.log")));
    McpServerConnection::new(McpServerConnectionOptions {
        entry,
        cwd: options.cwd.clone(),
        create_transport: Arc::new(create_default_transport),
        credentials,
        provider_token: None,
        on_tools: Arc::new(|_| {}),
        on_change: None,
        log: Some(log),
    })
}

/// Short spellings of options. `-l`/`--local` match `pi install`.
fn option_alias(argument: &str) -> &str {
    match argument {
        "-l" => "--local",
        other => other,
    }
}

#[derive(Default)]
struct ParsedOptions {
    positional: Vec<String>,
    values: HashMap<String, Option<String>>,
    /// Values of `list` options, in order.
    lists: HashMap<String, Vec<String>>,
}

impl ParsedOptions {
    fn has(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).and_then(|value| value.as_deref())
    }
}

/// Parse `--name value` options; returns None and reports unknown ones. `--`
/// ends the options, as does reaching `max_positionals` positional arguments:
/// the remaining arguments are positional, so a command's own options
/// (`add <server> <command> --flag`) are passed through (upstream
/// `parseOptions`).
fn parse_options(
    args: &[String],
    known: &HashMap<&str, OptionKind>,
    error: &dyn Fn(&str),
    max_positionals: usize,
) -> Option<ParsedOptions> {
    let mut parsed = ParsedOptions::default();
    let mut index = 0usize;
    while index < args.len() {
        let raw = option_alias(&args[index]).to_string();
        if raw == "--" || parsed.positional.len() >= max_positionals {
            let start = if raw == "--" { index + 1 } else { index };
            parsed.positional.extend(args[start..].iter().cloned());
            break;
        }
        if !raw.starts_with("--") {
            parsed.positional.push(raw);
            index += 1;
            continue;
        }
        let name = raw.trim_start_matches("--").to_string();
        let Some(kind) = known.get(name.as_str()) else {
            error(&format!("Unknown option {raw}.\n{}", help_hint()));
            return None;
        };
        match kind {
            OptionKind::Flag => {
                parsed.values.insert(name, None);
                index += 1;
            }
            OptionKind::Value | OptionKind::List => {
                index += 1;
                let value = args.get(index);
                let Some(value) = value else {
                    error(&format!("{raw} needs a value."));
                    return None;
                };
                if *kind == OptionKind::List {
                    parsed.lists.entry(name).or_default().push(value.clone());
                } else {
                    parsed.values.insert(name, Some(value.clone()));
                }
                index += 1;
            }
        }
    }
    Some(parsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionKind {
    Flag,
    Value,
    List,
}

fn known_options<'a>(entries: &'a [(&'a str, OptionKind)]) -> HashMap<&'a str, OptionKind> {
    entries.iter().copied().collect()
}

/// Run `pi mcp <args>` and return the exit code (upstream `runMcpCommand`).
pub async fn run_mcp_command(args: &[String], options: &McpCommandOptions) -> i32 {
    let log: Arc<dyn Fn(&str) + Send + Sync> = options
        .log
        .clone()
        .unwrap_or_else(|| Arc::new(|line: &str| println!("{line}")));
    let error: Arc<dyn Fn(&str) + Send + Sync> = options
        .error
        .clone()
        .unwrap_or_else(|| Arc::new(|line: &str| eprintln!("{line}")));
    let log_ref: &dyn Fn(&str) = log.as_ref();
    let error_ref: &dyn Fn(&str) = error.as_ref();
    let Some(command) = args.first() else {
        log_ref(&help_text());
        return 0;
    };
    if command == "help" || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        log_ref(&help_text());
        return 0;
    }

    let project_config = path_join(&options.cwd, &format!("{CONFIG_DIR_NAME}/mcp.json"));
    match command.as_str() {
        "add" => {
            return add(&args[1..], &project_config, options, log_ref, error_ref);
        }
        "remove" => {
            return remove(&args[1..], &project_config, options, log_ref, error_ref);
        }
        _ => {}
    }
    let project_trusted = ProjectTrustStore::new(&options.agent_dir)
        .ok()
        .and_then(|store| store.get(&options.cwd).ok())
        .flatten()
        == Some(true);
    let loaded = load_mcp_config(LoadedMcpConfigOptions {
        agent_dir: options.agent_dir.clone(),
        cwd: options.cwd.clone(),
        project_trusted,
    });
    let untrusted_note = if !project_trusted && std::path::Path::new(&project_config).exists() {
        Some(format!(
            "{project_config} is ignored because the project is not trusted. Start {APP_NAME} in the project to trust it."
        ))
    } else {
        None
    };
    let credentials = options
        .credentials
        .clone()
        .unwrap_or_else(|| Arc::new(McpOAuthCredentialStore::new()));

    match command.as_str() {
        "list" => {
            let known = known_options(&[("json", OptionKind::Flag)]);
            let Some(parsed) = parse_options(&args[1..], &known, error_ref, usize::MAX) else {
                return 1;
            };
            if !parsed.positional.is_empty() {
                error_ref(&format!(
                    "Usage: {APP_NAME} mcp list [--json]\n{}",
                    help_hint()
                ));
                return 1;
            }
            list(
                &loaded,
                parsed.has("json"),
                untrusted_note,
                options,
                credentials,
                log_ref,
            )
            .await
        }
        "login" | "logout" => {
            let known = if command == "login" {
                known_options(&[("timeout", OptionKind::Value)])
            } else {
                known_options(&[])
            };
            let Some(parsed) = parse_options(&args[1..], &known, error_ref, usize::MAX) else {
                return 1;
            };
            let positional = &parsed.positional;
            let invalid = positional.is_empty() || positional.len() > 1;
            if invalid {
                error_ref(&format!(
                    "Usage: {APP_NAME} mcp {command} <server>\n{}",
                    help_hint()
                ));
                return 1;
            }
            let name = &positional[0];
            let Some(entry) = loaded
                .servers
                .iter()
                .find(|server| &server.name == name)
                .cloned()
            else {
                let configured = loaded
                    .servers
                    .iter()
                    .map(|server| server.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                error_ref(&format!(
                    "No MCP server named \"{name}\".{} Configured: {}.",
                    untrusted_note
                        .as_ref()
                        .map(|note| format!(" {note}"))
                        .unwrap_or_default(),
                    if configured.is_empty() {
                        "none"
                    } else {
                        configured.as_str()
                    },
                ));
                return 1;
            };
            let connection = create_connection(entry.clone(), options, Arc::clone(&credentials));
            let Some(url) = connection.oauth_url() else {
                error_ref(&format!(
                    "MCP server \"{name}\" does not use OAuth. Only HTTP servers without an Authorization header do."
                ));
                return 1;
            };
            if command == "logout" {
                let removed = credentials.remove(name, &url).await;
                let message = if removed {
                    format!("Signed out of MCP server \"{name}\".")
                } else {
                    format!("No stored credentials for MCP server \"{name}\".")
                };
                log_ref(&message);
                return 0;
            }
            // Upstream `Number(value ?? default)`: an unparseable value is
            // NaN and fails the finite check.
            let timeout = match parsed.value("timeout") {
                Some(value) => value.parse::<f64>().unwrap_or(f64::NAN),
                None => DEFAULT_LOGIN_TIMEOUT_SECONDS,
            };
            if !timeout.is_finite() || timeout <= 0.0 {
                error_ref("--timeout must be a positive number of seconds.");
                return 1;
            }
            let result = login(
                &entry,
                &connection,
                &url,
                timeout * 1000.0,
                options,
                &credentials,
                &log,
                &error,
            )
            .await;
            let _ = connection.close().await;
            result
        }
        other => {
            error_ref(&format!(
                "Unknown mcp command \"{other}\".\n{}",
                help_hint()
            ));
            1
        }
    }
}

/// Parse `KEY=VALUE` pairs of a repeatable option into a record (upstream
/// `parsePairs`). `None` reports a malformed pair.
fn parse_pairs(
    option: &str,
    pairs: Option<&Vec<String>>,
    error: &dyn Fn(&str),
) -> Option<Vec<(String, String)>> {
    let Some(pairs) = pairs else {
        return Some(Vec::new());
    };
    let mut record: Vec<(String, String)> = Vec::new();
    for pair in pairs {
        let separator = pair.find('=');
        let Some(separator) = separator else {
            error(&format!("--{option} expects KEY=VALUE, got \"{pair}\"."));
            return None;
        };
        if separator == 0 {
            error(&format!("--{option} expects KEY=VALUE, got \"{pair}\"."));
            return None;
        }
        record.push((
            pair[..separator].to_string(),
            pair[separator + 1..].to_string(),
        ));
    }
    Some(record)
}

fn add(
    args: &[String],
    project_config: &str,
    options: &McpCommandOptions,
    log: &dyn Fn(&str),
    error: &dyn Fn(&str),
) -> i32 {
    let usage = format!(
        "Usage: {APP_NAME} mcp add <server> [options] (--url <url> | -- <command> [args...])\n{}",
        help_hint()
    );
    let known = known_options(&[
        ("local", OptionKind::Flag),
        ("url", OptionKind::Value),
        ("env", OptionKind::List),
        ("cwd", OptionKind::Value),
        ("header", OptionKind::List),
        ("bearer-token-env-var", OptionKind::Value),
        ("oauth-client-id", OptionKind::Value),
        ("oauth-client-secret", OptionKind::Value),
        ("oauth-callback-port", OptionKind::Value),
        ("oauth-client-name", OptionKind::Value),
        ("exposure", OptionKind::Value),
        ("description", OptionKind::Value),
    ]);
    let Some(parsed) = parse_options(args, &known, error, 2) else {
        return 1;
    };
    let positional = &parsed.positional;
    let Some(name) = positional.first().cloned() else {
        error(&usage);
        return 1;
    };
    let command_parts = &positional[1..];
    let url = parsed.value("url").map(str::to_string);
    if url.is_some() == command_parts.is_empty() {
        error(&usage);
        return 1;
    }
    let value = |option: &str| parsed.value(option).map(str::to_string);
    let exposure = value("exposure");
    let http_only = [
        "header",
        "bearer-token-env-var",
        "oauth-client-id",
        "oauth-client-secret",
        "oauth-callback-port",
        "oauth-client-name",
    ];
    let stdio_only = ["env", "cwd"];
    let relevant: &[&str] = if url.is_none() {
        &http_only
    } else {
        &stdio_only
    };
    let misplaced = relevant
        .iter()
        .find(|option| parsed.has(option) || parsed.lists.contains_key(**option));
    if let Some(misplaced) = misplaced {
        error(&format!(
            "--{misplaced} only applies to {}.",
            if url.is_none() {
                "HTTP servers (--url)"
            } else {
                "stdio servers"
            }
        ));
        return 1;
    }
    let mut config: Map<String, Value> = Map::new();
    if let Some(url) = &url {
        let Some(headers_pairs) = parse_pairs("header", parsed.lists.get("header"), error) else {
            return 1;
        };
        let mut headers: Map<String, Value> = headers_pairs
            .into_iter()
            .map(|(key, value)| (key, Value::from(value)))
            .collect();
        if let Some(bearer) = value("bearer-token-env-var") {
            headers.insert(
                "Authorization".into(),
                Value::from(format!("Bearer ${{{bearer}}}")),
            );
        }
        let mut oauth = Map::new();
        if let Some(client_id) = value("oauth-client-id") {
            oauth.insert("clientId".into(), Value::from(client_id));
        }
        if let Some(client_secret) = value("oauth-client-secret") {
            oauth.insert("clientSecret".into(), Value::from(client_secret));
        }
        if let Some(port) = value("oauth-callback-port") {
            // Upstream `Number(port)`: NaN drops the key under
            // JSON.stringify; integers stay integers.
            match port.parse::<f64>() {
                Ok(port) if port.is_finite() => {
                    if port.fract() == 0.0 {
                        oauth.insert("callbackPort".into(), json!(port as u64));
                    } else {
                        oauth.insert("callbackPort".into(), json!(port));
                    }
                }
                _ => {}
            }
        }
        if let Some(client_name) = value("oauth-client-name") {
            oauth.insert("clientName".into(), Value::from(client_name));
        }
        config.insert("url".into(), Value::from(url.clone()));
        if !headers.is_empty() {
            config.insert("headers".into(), Value::Object(headers));
        }
        if !oauth.is_empty() {
            config.insert("oauth".into(), Value::Object(oauth));
        }
    } else {
        let Some(env_pairs) = parse_pairs("env", parsed.lists.get("env"), error) else {
            return 1;
        };
        let mut executable_parts = command_parts.iter();
        let Some(executable) = executable_parts.next() else {
            error(&usage);
            return 1;
        };
        let command_args: Vec<String> = executable_parts.cloned().collect();
        let env: Map<String, Value> = env_pairs
            .into_iter()
            .map(|(key, value)| (key, Value::from(value)))
            .collect();
        config.insert("command".into(), Value::from(executable.clone()));
        if !command_args.is_empty() {
            config.insert(
                "args".into(),
                Value::Array(command_args.into_iter().map(Value::from).collect()),
            );
        }
        if !env.is_empty() {
            config.insert("env".into(), Value::Object(env));
        }
        if let Some(cwd) = value("cwd") {
            config.insert("cwd".into(), Value::from(cwd));
        }
    }
    if let Some(exposure) = exposure {
        config.insert("exposure".into(), Value::from(exposure));
    }
    if let Some(description) = value("description") {
        config.insert("description".into(), Value::from(description));
    }
    let ordered = OrderedMap::from_pairs(config.clone());
    let validated = match validate_mcp_server_config(&name, &ordered) {
        Ok(validated) => validated,
        Err(message) => {
            error(&message);
            return 1;
        }
    };
    let project = parsed.has("local");
    let path = if project {
        project_config.to_string()
    } else {
        path_join(&options.agent_dir, "mcp.json")
    };
    let scope = if project { "project" } else { "global" };
    let replaced = match add_mcp_server_config(&path, &name, &ordered) {
        Ok(replaced) => replaced,
        Err(add_error) => {
            error(&format!("Could not update {path}: {add_error}"));
            return 1;
        }
    };
    log(&format!(
        "{} {scope} MCP server \"{name}\" in {path}.",
        if replaced { "Replaced" } else { "Added" }
    ));
    let trusted = ProjectTrustStore::new(&options.agent_dir)
        .ok()
        .and_then(|store| store.get(&options.cwd).ok())
        .flatten()
        == Some(true);
    if project && !trusted {
        log(&format!(
            "The project is not trusted, so {path} is ignored until you start {APP_NAME} in the project and trust it."
        ));
    }
    // HTTP servers without an Authorization header may use OAuth.
    let may_need_sign_in = validated.url().is_some()
        && !validated
            .string_record("headers")
            .unwrap_or_default()
            .iter()
            .any(|(header, _)| header.to_lowercase() == "authorization");
    log(&format!(
        "Check it with: {APP_NAME} mcp list{}",
        if may_need_sign_in {
            format!(". If it requires sign-in: {APP_NAME} mcp login {name}")
        } else {
            String::new()
        }
    ));
    0
}

fn remove(
    args: &[String],
    project_config: &str,
    options: &McpCommandOptions,
    log: &dyn Fn(&str),
    error: &dyn Fn(&str),
) -> i32 {
    let known = known_options(&[("local", OptionKind::Flag)]);
    let Some(parsed) = parse_options(args, &known, error, usize::MAX) else {
        return 1;
    };
    let positional = &parsed.positional;
    if positional.is_empty() || positional.len() > 1 {
        error(&format!(
            "Usage: {APP_NAME} mcp remove <server> [-l]\n{}",
            help_hint()
        ));
        return 1;
    }
    let name = &positional[0];
    let project = parsed.has("local");
    let global_config = path_join(&options.agent_dir, "mcp.json");
    let path = if project {
        project_config.to_string()
    } else {
        global_config
    };
    let scope = if project { "project" } else { "global" };
    let removed = match remove_mcp_server_config(&path, name) {
        Ok(removed) => removed,
        Err(remove_error) => {
            error(&format!("Could not update {path}: {remove_error}"));
            return 1;
        }
    };
    if removed {
        log(&format!(
            "Removed {scope} MCP server \"{name}\" from {path}."
        ));
        return 0;
    }
    let other = load_mcp_config(LoadedMcpConfigOptions {
        agent_dir: options.agent_dir.clone(),
        cwd: options.cwd.clone(),
        project_trusted: true,
    })
    .servers
    .into_iter()
    .find(|server| &server.name == name && server.scope.map(|scope| scope.as_str()) != Some(scope));
    let other_note = other.map(|other| {
        format!(
            " It is defined in {}{}.",
            other.source,
            if other.scope.as_ref().map(|scope| scope.as_str()) == Some("project") {
                "; use --local"
            } else {
                "; omit --local"
            }
        )
    });
    error(&format!(
        "No {scope} MCP server named \"{name}\" in {path}.{}",
        other_note.unwrap_or_default()
    ));
    1
}

#[allow(clippy::too_many_arguments)]
async fn list(
    loaded: &LoadedMcpConfig,
    json: bool,
    untrusted_note: Option<String>,
    options: &McpCommandOptions,
    credentials: Arc<McpOAuthCredentialStore>,
    log: &dyn Fn(&str),
) -> i32 {
    let mut reports: Vec<ServerReport> = Vec::new();
    for entry in &loaded.servers {
        let mut report = ServerReport {
            name: entry.name.clone(),
            scope: entry
                .scope
                .map(|scope| scope.as_str().to_string())
                .unwrap_or_else(|| "global".to_string()),
            source: entry.source.clone(),
            enabled: entry.config.enabled(),
            exposure: crate::coding_agent::extensions::mcp::config::exposure_json_name(
                entry.config.exposure(),
            )
            .to_string(),
            transport: describe_transport(entry),
            state: "disabled".to_string(),
            tools: Vec::new(),
            tool_exposure: Map::new(),
            resources: None,
            resource_templates: None,
            error: None,
        };
        if !report.enabled {
            reports.push(report);
            continue;
        }
        let connection = create_connection(entry.clone(), options, Arc::clone(&credentials));
        // The connection records the state and error.
        let _ = connection.get_client().await;
        report.state = connection.state().as_str().to_string();
        let tools = connection.tools();
        report.tools = tools.iter().map(|tool| tool.name.clone()).collect();
        let mut overrides: Vec<(String, String)> = Vec::new();
        for tool in &tools {
            let exposure = crate::coding_agent::core::mcp_servers::get_mcp_tool_exposure(
                &entry.config,
                &tool.name,
            );
            if crate::coding_agent::extensions::mcp::config::exposure_json_name(exposure)
                != report.exposure
            {
                overrides.push((
                    tool.name.clone(),
                    crate::coding_agent::extensions::mcp::config::exposure_json_name(exposure)
                        .to_string(),
                ));
            }
        }
        for (tool, exposure) in overrides {
            report.tool_exposure.insert(tool, Value::from(exposure));
        }
        if connection.has_resources() {
            report.resources = Some(connection.resources().len() as f64);
            report.resource_templates = Some(connection.resource_templates().len() as f64);
        }
        if connection.state() != ServerState::Connected {
            report.error = connection.error();
        }
        let _ = connection.close().await;
        reports.push(report);
    }
    let failed = !loaded.errors.is_empty()
        || reports
            .iter()
            .any(|report| report.enabled && report.state != "connected");

    if json {
        let mut payload = Map::new();
        payload.insert(
            "servers".into(),
            Value::Array(reports.iter().map(|report| report.to_json()).collect()),
        );
        payload.insert(
            "errors".into(),
            Value::Array(loaded.errors.iter().cloned().map(Value::from).collect()),
        );
        if let Some(note) = &untrusted_note {
            payload.insert("note".into(), Value::from(note.clone()));
        }
        log(&serde_json::to_string_pretty(&Value::Object(payload)).unwrap_or_default());
        return if failed { 1 } else { 0 };
    }
    if reports.is_empty() && loaded.errors.is_empty() {
        log(&format!(
            "No MCP servers configured. Add them to {} or .pi/mcp.json.",
            path_join(&options.agent_dir, "mcp.json")
        ));
    }
    for report in &reports {
        let state = if report.state == "connected" {
            format!(
                "connected, {} tool{}",
                report.tools.len(),
                if report.tools.len() == 1 { "" } else { "s" }
            )
        } else if report.state == "needs-auth" {
            "needs sign-in".to_string()
        } else {
            report.state.clone()
        };
        log(&format!(
            "{}: {} ({}, {})",
            report.name, state, report.exposure, report.scope
        ));
        log(&format!("  {}", report.transport));
        if report.state == "needs-auth" {
            log(&format!(
                "  sign in with: {APP_NAME} mcp login {}",
                report.name
            ));
        }
        if !report.tools.is_empty() {
            let tools = report
                .tools
                .iter()
                .map(
                    |tool| match report.tool_exposure.get(tool).and_then(Value::as_str) {
                        Some(exposure) => format!("{tool} [{exposure}]"),
                        None => tool.clone(),
                    },
                )
                .collect::<Vec<_>>()
                .join(", ");
            log(&format!("  tools: {tools}"));
        }
        if let Some(resources) = report.resources {
            log(&format!(
                "  resources: {}, URI templates: {}",
                resources,
                report.resource_templates.unwrap_or(0.0)
            ));
        }
        if let Some(error_text) = &report.error {
            log(&format!(
                "  {}",
                error_text.split('\n').collect::<Vec<_>>().join("\n  ")
            ));
        }
    }
    for config_error in &loaded.errors {
        log(&format!("config error: {config_error}"));
    }
    if let Some(note) = &untrusted_note {
        log(note);
    }
    if failed {
        1
    } else {
        0
    }
}

#[allow(clippy::too_many_arguments)]
async fn login(
    entry: &McpServerEntry,
    connection: &Arc<McpServerConnection>,
    url: &str,
    timeout_ms: f64,
    options: &McpCommandOptions,
    credentials: &Arc<McpOAuthCredentialStore>,
    log: &Arc<dyn Fn(&str) + Send + Sync>,
    error: &Arc<dyn Fn(&str) + Send + Sync>,
) -> i32 {
    let name = &entry.name;
    // Connecting first answers whether a sign-in is needed and records the
    // server's challenge.
    match connection.get_client().await {
        Ok(_) => {
            log(&format!(
                "Already signed in to MCP server \"{name}\" ({} tools).",
                connection.tools().len()
            ));
            return 0;
        }
        Err(_) => {
            if connection.state() != ServerState::NeedsAuth {
                error(&format!(
                    "MCP server \"{name}\" failed to connect: {}",
                    connection
                        .error()
                        .unwrap_or_else(|| "unknown error".to_string())
                ));
                return 1;
            }
        }
    }

    let open_url: Arc<dyn Fn(&str) + Send + Sync> = options
        .open_url
        .clone()
        .unwrap_or_else(|| Arc::new(open_browser));
    let interactive = is_stdin_tty() && options.open_url.is_none();
    let prompt = Arc::new(CliSignInPrompt {
        name: name.clone(),
        timeout_ms,
        interactive,
        open_url,
        log: Arc::clone(log),
        error: Arc::clone(error),
    });
    let result = sign_in_mcp_server(
        url,
        credentials.for_server(name, url),
        connection.oauth_settings(),
        connection.challenge(),
        prompt,
    )
    .await;
    if let Err(sign_in_error) = result {
        error(&match sign_in_error {
            McpSignInError::Cancelled => format!(
                "Sign-in to MCP server \"{name}\" was cancelled or not completed within {} seconds.",
                (timeout_ms / 1000.0).round()
            ),
            McpSignInError::Failed(message) => {
                format!("Sign-in to MCP server \"{name}\" failed: {message}")
            }
        });
        return 1;
    }
    connection.clear_challenge();
    if let Err(connect_error) = connection.reconnect().await {
        error(&format!("Signed in, but {connect_error}"));
        return 1;
    }
    log(&format!(
        "Signed in to MCP server \"{name}\" ({} tools).",
        connection.tools().len()
    ));
    0
}

fn is_stdin_tty() -> bool {
    // `process.stdin.isTTY` stand-in.
    #[cfg(unix)]
    {
        unsafe { libc::isatty(0) == 1 }
    }
    #[cfg(windows)]
    {
        std::env::var_os("PI_MCP_FORCE_INTERACTIVE").is_some()
    }
}

/// The pasted redirect URL in a terminal; otherwise only the browser callback
/// can finish the sign-in. Resolves to None (cancelling the sign-in) after
/// `timeout_ms`, or when the callback arrived (upstream `waitForRedirectUrl`).
async fn wait_for_redirect_url(
    signal: Arc<crate::coding_agent::extensions::types::AbortSignal>,
    timeout_ms: f64,
    interactive: bool,
) -> Option<String> {
    let controller = Arc::new(crate::coding_agent::extensions::types::AbortSignal::new());
    {
        let controller = Arc::clone(&controller);
        signal.on_abort(Arc::new(move || controller.abort()));
    }
    // Upstream `setTimeout(abort, timeoutMs)` + `clearTimeout` in `finally`.
    let timer_controller = Arc::clone(&controller);
    let timer = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(timeout_ms as u64)).await;
        timer_controller.abort();
    });
    let result = async {
        if !interactive {
            controller.cancelled().await;
            return None;
        }
        eprint!("If the browser cannot reach this machine, paste the URL it was redirected to: ");
        let line = tokio::select! {
            line = read_stdin_line() => line,
            _ = controller.cancelled() => return None,
        };
        line
    }
    .await;
    timer.abort();
    result
}

async fn read_stdin_line() -> Option<String> {
    use tokio::io::AsyncBufReadExt;
    let stdin = tokio::io::stdin();
    let mut lines = tokio::io::BufReader::new(stdin).lines();
    lines.next_line().await.ok().flatten()
}

/// Upstream `promptForRedirectUrl` of the CLI sign-in.
struct CliSignInPrompt {
    name: String,
    timeout_ms: f64,
    interactive: bool,
    open_url: Arc<dyn Fn(&str) + Send + Sync>,
    log: Arc<dyn Fn(&str) + Send + Sync>,
    #[allow(dead_code)]
    error: Arc<dyn Fn(&str) + Send + Sync>,
}

impl McpSignInPrompt for CliSignInPrompt {
    fn show_authorization_url(&self, url: &url::Url) {
        (self.log)(&format!(
            "Sign in to MCP server \"{}\" in your browser:\n{}",
            self.name,
            url.as_str()
        ));
        (self.open_url)(url.as_str());
    }

    fn prompt_for_redirect_url<'a>(
        &'a self,
        signal: Arc<crate::coding_agent::extensions::types::AbortSignal>,
    ) -> BoxFuture<'a, Option<String>> {
        Box::pin(
            async move { wait_for_redirect_url(signal, self.timeout_ms, self.interactive).await },
        )
    }
}
