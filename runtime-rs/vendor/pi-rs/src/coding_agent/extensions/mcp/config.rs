//! Port of upstream `coding-agent/src/extensions/mcp/config.ts` (HEAD
//! `2bbfcca43`): MCP server configuration.
//!
//! Servers are read from `mcp.json` in the agent directory and, for trusted
//! projects, from `<project>/.pi/mcp.json`. Both use the `mcpServers` shape
//! shared by other MCP clients, so existing configurations can be copied
//! over. Project entries replace global entries with the same name.
//!
//! HTTP servers without an `Authorization` header use OAuth when they answer
//! 401 (sign in with `/mcp`). The top-level `autoEnableCodemode` (default
//! true) activates the codemode tool when a server with `codemode` exposure
//! connects; a project value overrides the global one.
//!
//! Disclosed seam: `JSON.parse` failure text is engine-shaped upstream (a V8
//! `SyntaxError` message). The port reports the `serde_json` message after
//! the same `<path>: ` prefix; everything else (the object-shape error, the
//! `autoEnableCodemode` type error, the per-server validation errors, entry
//! precedence and the file rewrites with the detected indentation) is
//! byte-equivalent.

use std::path::Path;

use serde_json::Value;

use crate::ai::types::ordered_map::OrderedMap;
use crate::coding_agent::core::mcp_servers::{
    validate_mcp_server_config, McpExposure, McpServerConfig,
};
use crate::coding_agent::core::{path_join, CONFIG_DIR_NAME};

pub use crate::coding_agent::core::mcp_servers::McpOAuthConfig;

/// Upstream `McpServerEntry`.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerEntry {
    pub name: String,
    pub config: McpServerConfig,
    /// Config file that defined the entry, or the path of the extension that
    /// registered it.
    pub source: String,
    /// The global or the project `mcp.json`, or `extension` for servers
    /// registered with `pi.registerMcpServer()`. Changes to extension servers
    /// are not saved.
    pub scope: Option<McpConfigScope>,
    /// Project `mcp.json` with an override of this global server's `enabled`,
    /// `exposure`, or `toolExposure` (v1.0.0).
    pub override_: Option<String>,
}

/// Upstream `"global" | "project" | "extension"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigScope {
    Global,
    Project,
    Extension,
}

impl McpConfigScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            McpConfigScope::Global => "global",
            McpConfigScope::Project => "project",
            McpConfigScope::Extension => "extension",
        }
    }
}

/// Upstream `LoadedMcpConfig`.
#[derive(Debug, Clone, Default)]
pub struct LoadedMcpConfig {
    pub servers: Vec<McpServerEntry>,
    /// Activate the codemode tool when `codemode` servers connect.
    /// `None` is upstream `undefined` (default true).
    pub auto_enable_codemode: Option<bool>,
    pub errors: Vec<String>,
    /// The project `mcp.json` when the project is trusted, where `/mcp` saves
    /// project overrides (v1.0.0). `None` is upstream `undefined`.
    pub project_config: Option<String>,
}

fn is_record(value: &Value) -> bool {
    value.is_object()
}

#[derive(Default)]
struct McpConfigState {
    servers: Vec<(String, McpServerEntry)>,
    auto_enable_codemode: Option<bool>,
    errors: Vec<String>,
}

impl McpConfigState {
    /// `servers.set(name, entry)` — replacement keeps the slot.
    fn set(&mut self, entry: McpServerEntry) {
        match self
            .servers
            .iter_mut()
            .find(|(name, _)| *name == entry.name)
        {
            Some(slot) => slot.1 = entry,
            None => self.servers.push((entry.name.clone(), entry)),
        }
    }
}

fn read_config_file(path: &str, scope: McpConfigScope, state: &mut McpConfigState) {
    if !Path::new(path).exists() {
        return;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // `readFileSync` failures (permission races) would crash the session
        // start upstream; the port treats the file as unreadable instead.
        Err(_) => return,
    };
    let parsed: Value = match serde_json::from_str(&text) {
        Ok(parsed) => parsed,
        Err(error) => {
            state.errors.push(format!("{path}: {error}"));
            return;
        }
    };
    if !is_record(&parsed) {
        state.errors.push(format!(
            "{path}: expected an object with an \"mcpServers\" object"
        ));
        return;
    }
    if let Some(servers) = parsed.get("mcpServers") {
        if !is_record(servers) {
            state.errors.push(format!(
                "{path}: expected an object with an \"mcpServers\" object"
            ));
            return;
        }
    }
    match parsed.get("autoEnableCodemode") {
        Some(Value::Bool(value)) => state.auto_enable_codemode = Some(*value),
        // JSON has no `undefined`: a present-but-null key is `!== undefined`
        // upstream and fails the boolean check.
        Some(_) => state
            .errors
            .push(format!("{path}: autoEnableCodemode must be a boolean")),
        None => {}
    }
    let entries: Vec<(String, Value)> = parsed
        .get("mcpServers")
        .and_then(Value::as_object)
        .map(|servers| {
            servers
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    for (name, value) in entries {
        // Upstream `validateMcpServerConfig` rejects non-object entries before
        // the shape checks.
        if !is_record(&value) {
            state
                .errors
                .push(format!("{path}: server \"{name}\" must be an object"));
            continue;
        }
        // v1.0.0: a project entry without `command`, `url`, or `type`
        // overrides only `enabled`, `exposure`, and `toolExposure` of the
        // global server with the same name.
        if scope == McpConfigScope::Project && is_override(&value) {
            let base = state.servers.iter().find(|(existing, _)| *existing == name);
            let extra: Vec<&String> = value
                .as_object()
                .expect("record checked")
                .keys()
                .filter(|key| !OVERRIDE_KEYS.contains(&key.as_str()))
                .collect();
            let Some((_, base_entry)) = base else {
                state.errors.push(format!(
                    "{path}: server \"{name}\" needs \"command\" or \"url\", or a global server to override"
                ));
                continue;
            };
            if !extra.is_empty() {
                state.errors.push(format!(
                    "{path}: server \"{name}\": an override can only set {}",
                    OVERRIDE_KEYS.join(", ")
                ));
                continue;
            }
            let mut merged = base_entry.config.raw().clone();
            for (key, entry_value) in value.as_object().expect("record checked") {
                merged.insert(key.clone(), entry_value.clone());
            }
            match validate_mcp_server_config(&name, &merged) {
                Ok(config) => {
                    let mut entry = base_entry.clone();
                    entry.config = config;
                    entry.override_ = Some(path.to_string());
                    state.set(entry);
                }
                Err(error) => state.errors.push(format!("{path}: {error}")),
            }
            continue;
        }
        let raw = OrderedMap::from_pairs(
            value
                .as_object()
                .expect("record checked")
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>(),
        );
        match validate_mcp_server_config(&name, &raw) {
            Ok(config) => {
                // Names that differ only in `-` and `_` would share a
                // namespace.
                let clash = state.servers.iter().find(|(other, _)| {
                    other != &name
                        && crate::coding_agent::core::mcp_servers::mcp_namespace(other)
                            == crate::coding_agent::core::mcp_servers::mcp_namespace(&name)
                });
                if let Some((clash, _)) = clash {
                    state.errors.push(format!(
                        "{path}: server \"{name}\" conflicts with \"{clash}\""
                    ));
                    continue;
                }
                if scope == McpConfigScope::Project
                    && config.url().is_some()
                    && config.auth_provider().is_some()
                {
                    state.errors.push(format!(
                        "{path}: server \"{name}\": auth is only allowed in the global mcp.json"
                    ));
                    continue;
                }
                state.set(McpServerEntry {
                    name,
                    config,
                    source: path.to_string(),
                    scope: Some(scope),
                    override_: None,
                });
            }
            Err(error) => state.errors.push(format!("{path}: {error}")),
        }
    }
}

/// Override-only keys of a project entry (upstream `OVERRIDE_KEYS`).
const OVERRIDE_KEYS: [&str; 3] = ["enabled", "exposure", "toolExposure"];

/// Whether an entry overrides a server defined elsewhere instead of defining
/// one (upstream `isOverride`).
fn is_override(value: &Value) -> bool {
    let object = value.as_object();
    let Some(object) = object else {
        return false;
    };
    !object.contains_key("command") && !object.contains_key("url") && !object.contains_key("type")
}

/// Upstream `loadMcpConfig` options.
pub struct LoadedMcpConfigOptions {
    pub agent_dir: String,
    pub cwd: String,
    pub project_trusted: bool,
}

/// Load global and (when trusted) project MCP configuration. Disabled servers
/// are included with `enabled: false`, so they can be enabled again (upstream
/// `loadMcpConfig`).
pub fn load_mcp_config(options: LoadedMcpConfigOptions) -> LoadedMcpConfig {
    let mut state = McpConfigState::default();
    read_config_file(
        &path_join(&options.agent_dir, "mcp.json"),
        McpConfigScope::Global,
        &mut state,
    );
    let project_config = if options.project_trusted {
        Some(path_join(
            &options.cwd,
            &format!("{CONFIG_DIR_NAME}/mcp.json"),
        ))
    } else {
        None
    };
    if let Some(project_config) = &project_config {
        read_config_file(project_config, McpConfigScope::Project, &mut state);
    }
    LoadedMcpConfig {
        servers: state.servers.into_iter().map(|(_, entry)| entry).collect(),
        auto_enable_codemode: state.auto_enable_codemode,
        errors: state.errors,
        project_config,
    }
}

/// Settings `/mcp` changes. `enabled: true` and `exposure: "codemode"` are the
/// defaults and remove the key (upstream `McpServerConfigPatch`).
#[derive(Debug, Clone, Copy, Default)]
pub struct McpServerConfigPatch {
    pub enabled: Option<bool>,
    pub exposure: Option<McpExposure>,
}

fn exposure_str(exposure: McpExposure) -> &'static str {
    match exposure {
        McpExposure::Codemode => "codemode",
        McpExposure::Deferred => "deferred",
        McpExposure::Direct => "direct",
        McpExposure::Hidden => "hidden",
    }
}

/// The JSON name of an exposure (shared with the extension index).
pub(crate) fn exposure_json_name(exposure: McpExposure) -> &'static str {
    exposure_str(exposure)
}

/// Change one server's settings in the `mcp.json` that defines or overrides
/// it. With `override_missing`, a missing entry is added as an override.
/// Overrides keep default values, since they replace the global server's.
/// Other content is kept; the file is rewritten with its indentation
/// (upstream `updateMcpServerConfig`).
pub fn update_mcp_server_config(
    path: &str,
    name: &str,
    patch: McpServerConfigPatch,
    override_missing: bool,
) -> Result<(), String> {
    edit_mcp_servers(path, |mut servers, parsed| {
        let existing = servers.as_deref().and_then(|s| s.get(name)).cloned();
        let mut server: OrderedMap<Value> = match existing {
            Some(record) if is_record(&record) => OrderedMap::from_pairs(
                record
                    .as_object()
                    .expect("record checked")
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>(),
            ),
            Some(_) => return Err(format!("{path} does not define MCP server \"{name}\"")),
            None => {
                if !override_missing {
                    return Err(format!("{path} does not define MCP server \"{name}\""));
                }
                // With `override_missing`, a missing entry is added as an
                // override: in the `mcpServers` record when one exists, else
                // directly in the document (upstream
                // `parsed.mcpServers = { ...servers, [name]: {} }`).
                match &mut servers {
                    Some(map) => {
                        map.insert(name.to_string(), Value::Object(serde_json::Map::new()));
                    }
                    None => {
                        let mut fresh = serde_json::Map::new();
                        fresh.insert(name.to_string(), Value::Object(serde_json::Map::new()));
                        parsed.insert("mcpServers".to_string(), Value::Object(fresh));
                    }
                }
                OrderedMap::new()
            }
        };
        // An override replaces the global server's values, so its defaults
        // are kept instead of deleted.
        let keep_defaults = is_override(&Value::Object(to_json_map(&server)));
        if let Some(enabled) = patch.enabled {
            if enabled && !keep_defaults {
                server = ordered_map_without(server, "enabled");
            } else {
                server.insert("enabled", Value::Bool(enabled));
            }
        }
        if let Some(exposure) = patch.exposure {
            if exposure == McpExposure::Codemode && !keep_defaults {
                server = ordered_map_without(server, "exposure");
            } else {
                server.insert("exposure", Value::from(exposure_str(exposure)));
            }
        }
        let patched = Value::Object(to_json_map(&server));
        match &mut servers {
            Some(map) => {
                map.insert(name.to_string(), patched);
            }
            None => {
                if let Some(mcp_servers) =
                    parsed.get_mut("mcpServers").and_then(Value::as_object_mut)
                {
                    mcp_servers.insert(name.to_string(), patched);
                }
            }
        }
        Ok(true)
    })
}

/// `delete server[key]` for the insertion-ordered map.
fn ordered_map_without(mut map: OrderedMap<Value>, key: &str) -> OrderedMap<Value> {
    let rebuilt: Vec<(String, Value)> = map
        .iter()
        .filter(|(existing, _)| existing.as_str() != key)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    map.clear();
    for (key, value) in rebuilt {
        map.insert(key, value);
    }
    map
}

fn to_json_map(map: &OrderedMap<Value>) -> serde_json::Map<String, Value> {
    map.iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect()
}

/// Add a server to an `mcp.json`, creating the file when missing. An existing
/// entry with the same name is replaced. Returns true when an entry was
/// replaced (upstream `addMcpServerConfig`).
pub fn add_mcp_server_config(
    path: &str,
    name: &str,
    config: &OrderedMap<Value>,
) -> Result<bool, String> {
    let mut replaced = false;
    edit_mcp_servers(path, |servers, parsed| {
        // Mutate the `mcpServers` record in place (upstream
        // `parsed.mcpServers[name] = config`); when absent, insert a fresh
        // record into the document.
        match servers {
            Some(servers) => {
                replaced = servers.contains_key(name);
                servers.insert(name.to_string(), Value::Object(to_json_map(config)));
            }
            None => {
                let mut target = serde_json::Map::new();
                target.insert(name.to_string(), Value::Object(to_json_map(config)));
                parsed.insert("mcpServers".to_string(), Value::Object(target));
            }
        }
        Ok(true)
    })?;
    Ok(replaced)
}

/// Remove a server from an `mcp.json`. Returns false when the file does not
/// define it (upstream `removeMcpServerConfig`).
pub fn remove_mcp_server_config(path: &str, name: &str) -> Result<bool, String> {
    if !Path::new(path).exists() {
        return Ok(false);
    }
    let mut removed = false;
    edit_mcp_servers(path, |servers, _parsed| match servers {
        Some(servers) if servers.contains_key(name) => {
            servers.remove(name);
            removed = true;
            Ok(true)
        }
        _ => Ok(false),
    })?;
    Ok(removed)
}

/// Read an `mcp.json` (an empty config when missing), let `edit` change its
/// `mcpServers`, and write it back with its indentation when `edit` returns
/// true. Other content is kept (upstream `editMcpServers`). `edit` receives
/// the `mcpServers` record (`None` when absent) and the parsed document; a
/// write happens only when it returns `Ok(true)`.
fn edit_mcp_servers(
    path: &str,
    edit: impl FnOnce(
        Option<&mut serde_json::Map<String, Value>>,
        &mut serde_json::Map<String, Value>,
    ) -> Result<bool, String>,
) -> Result<(), String> {
    let text = if Path::new(path).exists() {
        Some(std::fs::read_to_string(path).map_err(|error| error.to_string())?)
    } else {
        None
    };
    let parsed: Value = match &text {
        None => Value::Object(serde_json::Map::new()),
        Some(text) => serde_json::from_str(text).map_err(|error| error.to_string())?,
    };
    if !is_record(&parsed)
        || parsed
            .get("mcpServers")
            .is_some_and(|servers| !is_record(servers))
    {
        return Err(format!(
            "{path}: expected an object with an \"mcpServers\" object"
        ));
    }
    let mut parsed_map: serde_json::Map<String, Value> = parsed
        .as_object()
        .expect("record checked")
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let mut servers: Option<serde_json::Map<String, Value>> = match parsed_map.get("mcpServers") {
        Some(servers) if is_record(servers) => Some(
            servers
                .as_object()
                .expect("record checked")
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
        _ => None,
    };
    let changed = edit(servers.as_mut(), &mut parsed_map)?;
    if !changed {
        return Ok(());
    }
    if let Some(servers) = servers {
        // Upstream mutates `parsed.mcpServers` in place; reinserting the
        // (possibly mutated) record is the same observable document. An
        // absent record stays absent unless `edit` inserted it.
        parsed_map.insert("mcpServers".to_string(), Value::Object(servers));
    }
    let indent = text
        .as_deref()
        .and_then(detect_indent)
        .unwrap_or_else(|| "  ".to_string());
    if let Some(parent) = Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(
        path,
        format!("{}\n", stringify_indent(&parsed_map, &indent)),
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// `/^([ \t]+)\S/m` — the indent of the first line that starts with spaces or
/// tabs followed by a non-whitespace character.
fn detect_indent(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start_matches([' ', '\t']);
        let leading = line.len() - trimmed.len();
        if leading > 0 && trimmed.starts_with(|c: char| !c.is_whitespace()) {
            return Some(line[..leading].to_string());
        }
    }
    None
}

/// `JSON.stringify(value, null, indent)`: the document with the detected
/// indent unit; empty objects and arrays collapse.
pub fn stringify_indent(value: &serde_json::Map<String, Value>, indent: &str) -> String {
    let mut out = String::new();
    write_value(&Value::Object(value.clone()), indent, 0, &mut out);
    out
}

fn write_value(value: &Value, indent: &str, depth: usize, out: &mut String) {
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (index, (key, value)) in map.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                for _ in 0..depth + 1 {
                    out.push_str(indent);
                }
                out.push_str(&serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string()));
                out.push_str(": ");
                write_value(value, indent, depth + 1, out);
            }
            out.push('\n');
            for _ in 0..depth {
                out.push_str(indent);
            }
            out.push('}');
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                for _ in 0..depth + 1 {
                    out.push_str(indent);
                }
                write_value(item, indent, depth + 1, out);
            }
            out.push('\n');
            for _ in 0..depth {
                out.push_str(indent);
            }
            out.push(']');
        }
        other => {
            let text = serde_json::to_string(other).unwrap_or_else(|_| "null".to_string());
            out.push_str(&text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(dir: &Path, text: &str) -> String {
        let path = path_join(&dir.to_string_lossy(), "mcp.json");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn temp_dir(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("mcp-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    #[test]
    fn loads_global_and_project_entries() {
        let agent = temp_dir("global");
        let project = temp_dir("project");
        write_config(
            Path::new(&agent),
            r#"{"mcpServers":{"fs":{"command":"npx"}}}"#,
        );
        // The project config lives at `<cwd>/.pi/mcp.json`.
        let project_pi = Path::new(&project).join(".pi");
        std::fs::create_dir_all(&project_pi).unwrap();
        write_config(
            &project_pi,
            r#"{"mcpServers":{"fs":{"command":"bun"},"docs":{"url":"https://x"}},"autoEnableCodemode":false}"#,
        );
        let loaded = load_mcp_config(LoadedMcpConfigOptions {
            agent_dir: agent.clone(),
            cwd: project.clone(),
            project_trusted: true,
        });
        assert!(loaded.errors.is_empty());
        assert_eq!(loaded.auto_enable_codemode, Some(false));
        // Project entry replaces the global one; order follows first mention.
        let names: Vec<&str> = loaded.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["fs", "docs"]);
        assert_eq!(loaded.servers[0].scope, Some(McpConfigScope::Project));
        assert_eq!(loaded.servers[1].scope, Some(McpConfigScope::Project));
        // Untrusted project: only the global entry.
        let loaded = load_mcp_config(LoadedMcpConfigOptions {
            agent_dir: agent,
            cwd: project,
            project_trusted: false,
        });
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].scope, Some(McpConfigScope::Global));
    }

    #[test]
    fn collects_shape_and_validation_errors() {
        let dir = temp_dir("errors");
        let path = write_config(
            Path::new(&dir),
            r#"{"mcpServers":{"bad":{"exposure":"nope","command":"x"}},"autoEnableCodemode":"yes"}"#,
        );
        let loaded = load_mcp_config(LoadedMcpConfigOptions {
            agent_dir: dir.clone(),
            cwd: dir.clone(),
            project_trusted: false,
        });
        assert_eq!(
            loaded.errors,
            vec![
                format!("{path}: autoEnableCodemode must be a boolean"),
                format!(
                    "{path}: server \"bad\": exposure must be one of \"codemode\", \"deferred\", \"direct\", \"hidden\""
                ),
            ]
        );
        let path2 = write_config(Path::new(&dir), r#"{"mcpServers": 5}"#);
        let loaded = load_mcp_config(LoadedMcpConfigOptions {
            agent_dir: dir.clone(),
            cwd: dir.clone(),
            project_trusted: false,
        });
        assert_eq!(
            loaded.errors,
            vec![format!(
                "{path2}: expected an object with an \"mcpServers\" object"
            )]
        );
        let path3 = write_config(Path::new(&dir), r#"{"mcpServers":{"fs":5}}"#);
        let loaded = load_mcp_config(LoadedMcpConfigOptions {
            agent_dir: dir.clone(),
            cwd: dir,
            project_trusted: false,
        });
        assert_eq!(
            loaded.errors,
            vec![format!("{path3}: server \"fs\" must be an object")]
        );
    }

    #[test]
    fn update_rewrites_with_detected_indent() {
        let dir = temp_dir("update");
        let path = write_config(
            Path::new(&dir),
            "{\n\t\"mcpServers\": {\n\t\t\"fs\": {\"command\": \"npx\"}\n\t},\n\t\"autoEnableCodemode\": true\n}\n",
        );
        update_mcp_server_config(
            &path,
            "fs",
            McpServerConfigPatch {
                enabled: Some(false),
                exposure: Some(McpExposure::Direct),
            },
            false,
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "{\n\t\"mcpServers\": {\n\t\t\"fs\": {\n\t\t\t\"command\": \"npx\",\n\t\t\t\"enabled\": false,\n\t\t\t\"exposure\": \"direct\"\n\t\t}\n\t},\n\t\"autoEnableCodemode\": true\n}\n"
        );
        // Defaults remove the keys.
        update_mcp_server_config(
            &path,
            "fs",
            McpServerConfigPatch {
                enabled: Some(true),
                exposure: Some(McpExposure::Codemode),
            },
            false,
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"fs\": {\n\t\t\t\"command\": \"npx\"\n\t\t}"));
        let error =
            update_mcp_server_config(&path, "other", McpServerConfigPatch::default(), false)
                .unwrap_err();
        assert_eq!(
            error,
            format!("{path} does not define MCP server \"other\"")
        );
    }

    #[test]
    fn add_creates_and_replaces() {
        let dir = temp_dir("add");
        let path = path_join(&path_join(&dir, "nested"), "mcp.json");
        let config = OrderedMap::from_pairs([("command".to_string(), Value::from("npx"))]);
        let replaced = add_mcp_server_config(&path, "fs", &config).unwrap();
        assert!(!replaced);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "{\n  \"mcpServers\": {\n    \"fs\": {\n      \"command\": \"npx\"\n    }\n  }\n}\n"
        );
        let replaced = add_mcp_server_config(&path, "fs", &config).unwrap();
        assert!(replaced);
        let removed = remove_mcp_server_config(&path, "fs").unwrap();
        assert!(removed);
        let removed = remove_mcp_server_config(&path, "fs").unwrap();
        assert!(!removed);
    }
}
