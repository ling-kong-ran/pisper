//! Replay of the MCP-extension oracle
//! (`tests/fixtures/mcp_extension_oracle/oracle/mcp_extension_oracle.json`),
//! captured from the verbatim upstream HEAD (`pi@2bbfcca43`, v0.99.1)
//! `extensions/mcp/{config,tools,resources,log}.ts` with their deterministic
//! dependency closure under node (type stripping); see the fixture's
//! `capture.mjs` for the exact scenarios and `manifest.json` for SHA-256
//! provenance.
//!
//! Known port divergences pinned around (disclosed in the module docs):
//! - `JSON.parse` failure text is engine-shaped; the `malformed_json` case
//!   compares only the `<path>: ` prefix (V8 `SyntaxError` message vs
//!   serde_json).
//! - The validated config's raw object is not carried; config payloads are
//!   compared through the typed accessors.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::config::{
    add_mcp_server_config, load_mcp_config, remove_mcp_server_config, update_mcp_server_config,
    LoadedMcpConfigOptions, McpConfigScope,
};
use super::log::{format_mcp_log_message, time_point::Zoned};
use super::resources::{
    create_mcp_resource_tool_definitions, McpResourceServer, McpResourceToolOptions,
};
use super::runtime::ServerState;
use super::tools::{
    convert_mcp_result, create_mcp_tool_definition, create_mcp_tool_name, to_tool_exposure,
    ConvertMcpResultOptions, McpToolDefinitionOptions,
};
use crate::coding_agent::core::mcp_servers::{get_mcp_tool_exposure, McpExposure, McpServerConfig};
use crate::mcp::protocol::content::CallToolResult;

use crate::mcp::client::McpRequestOptions;
use crate::mcp::protocol::types::{
    ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult, Resource,
    ResourceTemplate, Tool as McpTool,
};

fn oracle() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/fixtures/mcp_extension_oracle/oracle/mcp_extension_oracle.json"
    ))
    .expect("mcp extension oracle parses")
}

fn observed(name: &str) -> Value {
    oracle()["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scenario| scenario["name"] == name)
        .unwrap_or_else(|| panic!("scenario {name} missing from the mcp extension oracle"))
        ["observed"]
        .clone()
}

/// Replaces `mine` with `theirs` in serialized text, so recorded absolute
/// paths compare byte-for-byte.
fn substitute(text: String, mine: &str, theirs: &str) -> String {
    text.replace(mine, theirs)
}

// ============================================================================
// config_load
// ============================================================================

struct LoadCase {
    name: String,
    files: Vec<(&'static str, String)>,
    agent_dir: &'static str,
    cwd: &'static str,
    project_trusted: bool,
}

fn load_cases() -> Vec<LoadCase> {
    let json = |value: Value| serde_json::to_string_pretty(&value).unwrap_or_default();
    vec![
        LoadCase {
            name: "config_load_global_only".into(),
            files: vec![(
                "agent/mcp.json",
                json(json!({
                    "mcpServers": {
                        "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] },
                        "docs": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer ${DOCS_TOKEN}" } },
                        "sentry": { "url": "https://mcp.sentry.dev/mcp" },
                    },
                })),
            )],
            agent_dir: "agent",
            cwd: "project",
            project_trusted: false,
        },
        LoadCase {
            name: "config_load_project_override_trusted".into(),
            files: vec![
                (
                    "agent/mcp.json",
                    json(json!({
                        "mcpServers": {
                            "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] },
                            "docs": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer ${DOCS_TOKEN}" } },
                            "sentry": { "url": "https://mcp.sentry.dev/mcp" },
                        },
                    })),
                ),
                (
                    "project/.pi/mcp.json",
                    json(json!({
                        "mcpServers": {
                            "docs": { "url": "https://other.example.com/mcp" },
                            "local": { "command": "bun", "args": ["server.ts"], "exposure": "codemode-deferred" },
                        },
                        "autoEnableCodemode": false,
                    })),
                ),
            ],
            agent_dir: "agent",
            cwd: "project",
            project_trusted: true,
        },
        LoadCase {
            name: "config_load_project_ignored_untrusted".into(),
            files: vec![
                (
                    "agent/mcp.json",
                    json(json!({
                        "mcpServers": {
                            "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] },
                            "docs": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer ${DOCS_TOKEN}" } },
                            "sentry": { "url": "https://mcp.sentry.dev/mcp" },
                        },
                    })),
                ),
                (
                    "project/.pi/mcp.json",
                    json(json!({
                        "mcpServers": {
                            "docs": { "url": "https://other.example.com/mcp" },
                            "local": { "command": "bun", "args": ["server.ts"], "exposure": "codemode-deferred" },
                        },
                        "autoEnableCodemode": false,
                    })),
                ),
            ],
            agent_dir: "agent",
            cwd: "project",
            project_trusted: false,
        },
        LoadCase {
            name: "config_load_shapes".into(),
            files: vec![(
                "agent/mcp.json",
                json(json!({
                    "mcpServers": {
                        "off": { "command": "x", "enabled": false },
                        "shaped": {
                            "command": "x",
                            "exposure": "direct",
                            "toolExposure": { "se*": "deferred", "secret*": "hidden", "exact": "codemode" },
                            "timeout": 30,
                            "description": "  tools for tests  ",
                        },
                        "alias": { "url": "https://x", "exposure": "codemode-deferred", "type": "streamable-http" },
                    },
                    "autoEnableCodemode": true,
                })),
            )],
            agent_dir: "agent",
            cwd: "project",
            project_trusted: false,
        },
        LoadCase {
            name: "config_load_malformed_json".into(),
            files: vec![("agent/mcp.json", "{".into())],
            agent_dir: "agent",
            cwd: "project",
            project_trusted: false,
        },
        LoadCase {
            name: "config_load_mcpServers_number".into(),
            files: vec![("agent/mcp.json", json(json!({ "mcpServers": 5 })))],
            agent_dir: "agent",
            cwd: "project",
            project_trusted: false,
        },
        LoadCase {
            name: "config_load_top_level_array".into(),
            files: vec![("agent/mcp.json", "[]".into())],
            agent_dir: "agent",
            cwd: "project",
            project_trusted: false,
        },
        LoadCase {
            name: "config_load_autoEnableCodemode_string".into(),
            files: vec![(
                "agent/mcp.json",
                json(json!({ "mcpServers": {}, "autoEnableCodemode": "yes" })),
            )],
            agent_dir: "agent",
            cwd: "project",
            project_trusted: false,
        },
        LoadCase {
            name: "config_load_no_mcpServers_key".into(),
            files: vec![("agent/mcp.json", json(json!({ "mcpServers": {} })))],
            agent_dir: "agent",
            cwd: "project",
            project_trusted: false,
        },
        LoadCase {
            name: "config_load_validation_errors".into(),
            files: vec![(
                "agent/mcp.json",
                json(json!({
                    "mcpServers": {
                        "bad name": { "command": "x" },
                        "exposure": { "command": "x", "exposure": "nope" },
                        "toolExposure": { "command": "x", "toolExposure": 5 },
                        "toolEntry": { "command": "x", "toolExposure": { "t": "no" } },
                        "enabled": { "command": "x", "enabled": "yes" },
                        "timeout": { "command": "x", "timeout": 0 },
                        "legacySse": { "type": "sse", "url": "https://x" },
                        "badUrl": { "url": "ftp://x" },
                        "badHeaders": { "url": "https://x", "headers": 5 },
                        "badPort": { "url": "https://x", "oauth": { "callbackPort": 0 } },
                        "badCallbackUrl": { "url": "https://x", "oauth": { "callbackUrl": "https://x/cb" } },
                        "badArgs": { "command": "x", "args": [1] },
                        "badEnv": { "command": "x", "env": { "A": 1 } },
                        "noTransport": {},
                        "entryNumber": 5,
                    },
                })),
            )],
            agent_dir: "agent",
            cwd: "project",
            project_trusted: false,
        },
        LoadCase {
            name: "config_load_missing_files".into(),
            files: vec![],
            agent_dir: "absent-agent",
            cwd: "absent-project",
            project_trusted: true,
        },
    ]
}

/// Compare the typed config against the oracle's raw validated object.
fn assert_config_matches(oracle_config: &Value, mine: &McpServerConfig) {
    let object = oracle_config.as_object().expect("oracle config object");
    assert_eq!(
        mine.command().map(str::to_string),
        object
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_string),
        "command of {oracle_config}"
    );
    assert_eq!(
        mine.args()
            .map(|args| args.into_iter().map(str::to_string).collect::<Vec<_>>()),
        object.get("args").and_then(Value::as_array).map(|args| args
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>()),
        "args of {oracle_config}"
    );
    assert_eq!(
        mine.url().map(str::to_string),
        object
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string),
        "url of {oracle_config}"
    );
    let expected_record = |key: &str| -> Option<Vec<(String, String)>> {
        object.get(key).and_then(Value::as_object).map(|record| {
            record
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        value.as_str().expect("string value").to_string(),
                    )
                })
                .collect()
        })
    };
    assert_eq!(
        mine.string_record("headers"),
        expected_record("headers"),
        "headers of {oracle_config}"
    );
    assert_eq!(
        mine.string_record("env"),
        expected_record("env"),
        "env of {oracle_config}"
    );
    assert_eq!(
        mine.cwd().map(str::to_string),
        object
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string),
        "cwd of {oracle_config}"
    );
    assert_eq!(
        exposure_name(mine.exposure()),
        object
            .get("exposure")
            .and_then(Value::as_str)
            .unwrap_or("codemode"),
        "exposure of {oracle_config}"
    );
    assert_eq!(
        mine.timeout(),
        object.get("timeout").and_then(Value::as_f64),
        "timeout of {oracle_config}"
    );
    assert_eq!(
        mine.enabled(),
        object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        "enabled of {oracle_config}"
    );
    assert_eq!(
        mine.description().map(str::to_string),
        object
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        "description of {oracle_config}"
    );
    let mine_tools: Vec<(String, String)> = mine
        .tool_exposure()
        .iter()
        .map(|(tool, exposure)| (tool.clone(), exposure_name(*exposure).to_string()))
        .collect();
    let oracle_tools: Vec<(String, String)> = object
        .get("toolExposure")
        .and_then(Value::as_object)
        .map(|record| {
            record
                .iter()
                .map(|(tool, value)| (tool.clone(), value.as_str().expect("exposure").to_string()))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(mine_tools, oracle_tools, "toolExposure of {oracle_config}");
    // oauth: compare the parsed projection.
    let oracle_oauth = object.get("oauth").and_then(Value::as_object);
    let mine_oauth = mine.oauth();
    assert_eq!(
        mine_oauth.is_some(),
        oracle_oauth.is_some(),
        "oauth presence"
    );
    if let (Some(oauth), Some(expected)) = (mine_oauth, oracle_oauth) {
        let field = |key: &str| expected.get(key).cloned();
        assert_eq!(
            oauth.client_id.map(Value::String),
            field("clientId"),
            "oauth clientId"
        );
        assert_eq!(
            oauth.client_secret.map(Value::String),
            field("clientSecret"),
            "oauth clientSecret"
        );
        assert_eq!(
            oauth.callback_port.map(|port| json!(port)),
            field("callbackPort"),
            "oauth callbackPort"
        );
        assert_eq!(
            oauth.callback_url.map(Value::String),
            field("callbackUrl"),
            "oauth callbackUrl"
        );
        assert_eq!(
            oauth.scope.map(Value::String),
            field("scope"),
            "oauth scope"
        );
        assert_eq!(
            oauth.client_name.map(Value::String),
            field("clientName"),
            "oauth clientName"
        );
    }
}

fn exposure_name(exposure: McpExposure) -> &'static str {
    match exposure {
        McpExposure::Codemode => "codemode",
        McpExposure::Deferred => "deferred",
        McpExposure::Direct => "direct",
        McpExposure::Hidden => "hidden",
    }
}

#[test]
fn oracle_config_load() {
    for case in load_cases() {
        let base = std::env::temp_dir().join(format!(
            "mcp-ext-load-rust-{}-{}",
            case.name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        for (relative, text) in &case.files {
            let path = base.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            std::fs::create_dir_all(path.parent().expect("parent")).unwrap();
            std::fs::write(&path, text).unwrap();
        }
        let agent_dir = base.join(case.agent_dir.replace('/', std::path::MAIN_SEPARATOR_STR));
        let cwd = base.join(case.cwd.replace('/', std::path::MAIN_SEPARATOR_STR));
        let loaded = load_mcp_config(LoadedMcpConfigOptions {
            agent_dir: agent_dir.to_string_lossy().into_owned(),
            cwd: cwd.to_string_lossy().into_owned(),
            project_trusted: case.project_trusted,
        });

        let oracle = observed(&case.name);
        // Errors: byte compare after the temp-dir substitution — the capture
        // sanitizes its scenario temp dir to `<root>` (see `capture.mjs`), so
        // ours is substituted for it. The malformed-JSON message is engine
        // text (disclosed): compare only the prefix shape.
        let oracle_errors: Vec<String> = oracle["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().map(str::to_string))
            .collect::<Option<_>>()
            .expect("error strings");
        if !oracle_errors.is_empty() || !loaded.errors.is_empty() {
            if case.name == "config_load_malformed_json" {
                assert_eq!(loaded.errors.len(), 1, "one parse error");
                // `<path>: <engine message>` — the port swaps only the
                // message text, so the `<path>: ` prefix must match.
                let parse_path = agent_dir.join("mcp.json");
                assert!(
                    loaded.errors[0].starts_with(&format!("{}: ", parse_path.display())),
                    "parse error keeps the path prefix: {}",
                    loaded.errors[0]
                );
            } else {
                // The capture ran on Windows (`<root>\...`); compare
                // separator-agnostically.
                let mine: Vec<String> = loaded
                    .errors
                    .iter()
                    .map(|error| substitute(error.clone(), &base.to_string_lossy(), "<root>"))
                    .map(|error| error.replace('\\', "/"))
                    .collect();
                let oracle_normalized: Vec<String> = oracle_errors
                    .iter()
                    .map(|error| error.replace('\\', "/"))
                    .collect();
                assert_eq!(mine, oracle_normalized, "errors of {}", case.name);
            }
        }

        // Servers: names, scopes, sources (path-substituted), configs.
        let oracle_servers = oracle["servers"].as_array().unwrap();
        assert_eq!(
            loaded.servers.len(),
            oracle_servers.len(),
            "server count of {}",
            case.name
        );
        for (mine, oracle_server) in loaded.servers.iter().zip(oracle_servers) {
            assert_eq!(mine.name, oracle_server["name"], "name of {}", case.name);
            let oracle_scope = oracle_server["scope"].as_str();
            let mine_scope = mine.scope.map(|scope| match scope {
                McpConfigScope::Global => "global",
                McpConfigScope::Project => "project",
                McpConfigScope::Extension => "extension",
            });
            assert_eq!(mine_scope, oracle_scope, "scope of {}", case.name);
            let mine_source = substitute(mine.source.clone(), &base.to_string_lossy(), "<root>");
            // The capture ran on a Windows host (`<root>\...`); compare
            // separator-agnostically.
            assert_eq!(
                mine_source.replace('\\', "/"),
                oracle_server["source"]
                    .as_str()
                    .expect("source is a string")
                    .replace('\\', "/"),
                "source of {}",
                case.name
            );
            assert_config_matches(&oracle_server["config"], &mine.config);
        }
        assert_eq!(
            loaded.auto_enable_codemode,
            oracle["autoEnableCodemode"].as_bool(),
            "autoEnableCodemode of {}",
            case.name
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[test]
fn oracle_config_update() {
    let oracle = observed("config_update");
    let base = std::env::temp_dir().join(format!("mcp-ext-update-rust-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    // a) two-space file.
    let two_space = base.join("two.json");
    std::fs::write(
        &two_space,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "mcpServers": {
                    "fs": { "command": "npx", "exposure": "direct" },
                    "other": { "command": "y" },
                },
                "autoEnableCodemode": true,
            }))
            .unwrap()
        ),
    )
    .unwrap();
    update_mcp_server_config(
        &two_space.to_string_lossy(),
        "fs",
        super::config::McpServerConfigPatch {
            enabled: Some(false),
            exposure: None,
        },
        false,
    )
    .unwrap();
    update_mcp_server_config(
        &two_space.to_string_lossy(),
        "fs",
        super::config::McpServerConfigPatch {
            enabled: None,
            exposure: Some(McpExposure::Deferred),
        },
        false,
    )
    .unwrap();
    update_mcp_server_config(
        &two_space.to_string_lossy(),
        "other",
        super::config::McpServerConfigPatch {
            enabled: Some(true),
            exposure: Some(McpExposure::Codemode),
        },
        false,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&two_space).unwrap(),
        oracle["two_space"].as_str().unwrap(),
        "two-space rewrite"
    );

    // b) tab-indented file keeps its indentation.
    let tabbed = base.join("tab.json");
    std::fs::write(
        &tabbed,
        "{\n\t\"mcpServers\": {\n\t\t\"fs\": {\"command\": \"npx\"}\n\t}\n}\n",
    )
    .unwrap();
    update_mcp_server_config(
        &tabbed.to_string_lossy(),
        "fs",
        super::config::McpServerConfigPatch {
            enabled: Some(false),
            exposure: Some(McpExposure::Hidden),
        },
        false,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&tabbed).unwrap(),
        oracle["tab"].as_str().unwrap(),
        "tab rewrite"
    );

    // c) errors. The capture sanitizes its scenario temp dir to `<root>`;
    // substitute ours for it.
    let error = update_mcp_server_config(
        &two_space.to_string_lossy(),
        "missing",
        super::config::McpServerConfigPatch::default(),
        false,
    )
    .unwrap_err();
    let oracle_error = oracle["update_missing"].as_str().unwrap();
    assert_eq!(
        // The capture ran on Windows (`<root>\...`); compare separators
        // normalized to forward slashes.
        substitute(error, &base.to_string_lossy(), "<root>").replace('\\', "/"),
        oracle_error.replace('\\', "/")
    );
    let broken = base.join("broken.json");
    std::fs::write(&broken, "[]").unwrap();
    let error = update_mcp_server_config(
        &broken.to_string_lossy(),
        "fs",
        super::config::McpServerConfigPatch::default(),
        false,
    )
    .unwrap_err();
    let oracle_error = oracle["update_broken"].as_str().unwrap();
    assert_eq!(
        // The capture ran on Windows (`<root>\...`); compare separators
        // normalized to forward slashes.
        substitute(error, &base.to_string_lossy(), "<root>").replace('\\', "/"),
        oracle_error.replace('\\', "/")
    );

    // d) add (creates directories), replace, remove.
    let add_path = base.join("nested").join("deep").join("mcp.json");
    let config = crate::ai::types::ordered_map::OrderedMap::from_pairs([(
        "command".to_string(),
        Value::from("npx"),
    )]);
    let added_first = add_mcp_server_config(&add_path.to_string_lossy(), "fs", &config).unwrap();
    let config_bun = crate::ai::types::ordered_map::OrderedMap::from_pairs([(
        "command".to_string(),
        Value::from("bun"),
    )]);
    let added_second =
        add_mcp_server_config(&add_path.to_string_lossy(), "fs", &config_bun).unwrap();
    assert_eq!(
        json!({ "addedFirst": added_first, "addedSecond": added_second }),
        json!({ "addedFirst": oracle["add"]["addedFirst"], "addedSecond": oracle["add"]["addedSecond"] })
    );
    assert_eq!(
        std::fs::read_to_string(&add_path).unwrap(),
        oracle["add"]["text"].as_str().unwrap(),
        "add rewrite"
    );
    assert!(
        remove_mcp_server_config(&add_path.to_string_lossy(), "fs").unwrap(),
        "remove existing"
    );
    assert!(
        !remove_mcp_server_config(&add_path.to_string_lossy(), "fs").unwrap(),
        "remove missing"
    );
    assert!(
        !remove_mcp_server_config(base.join("nope.json").to_string_lossy().as_ref(), "fs").unwrap(),
        "remove absent file"
    );
    let _ = std::fs::remove_dir_all(&base);
}

// ============================================================================
// tool names / definitions
// ============================================================================

#[test]
fn oracle_tool_names() {
    let oracle = observed("tool_names");
    assert_eq!(
        create_mcp_tool_name("docs", "search", |_| false),
        oracle["basic"]
    );
    assert_eq!(
        create_mcp_tool_name("my.server-1", "a.b/c", |_| false),
        oracle["sanitized"]
    );
    assert_eq!(
        create_mcp_tool_name("s", "t\u{00e9}st", |_| false),
        oracle["unicode"]
    );
    assert_eq!(
        create_mcp_tool_name("s", "a.b", |name| name == "mcp__s__a_b"),
        oracle["taken"]
    );
    let long_tool = "t".repeat(200);
    let long_name = create_mcp_tool_name("s", &long_tool, |_| false);
    assert_eq!(
        json!({
            "length": long_name.len(),
            "suffix": long_name[long_name.len() - 9..],
            "expectedSuffix": oracle["long"]["expectedSuffix"],
        }),
        json!({
            "length": oracle["long"]["length"],
            "suffix": oracle["long"]["suffix"],
            "expectedSuffix": oracle["long"]["expectedSuffix"],
        })
    );
}

fn tool_from_json(value: &Value) -> McpTool {
    serde_json::from_value(value.clone()).expect("oracle tool shape")
}

#[test]
fn oracle_tool_definitions_real() {
    let oracle = observed("tool_definitions");
    let config_json = json!({
        "command": "x",
        "exposure": "direct",
        "toolExposure": { "hidden_override": "hidden", "no.*": "deferred" },
    });
    let ordered = crate::ai::types::ordered_map::OrderedMap::from_pairs(
        config_json
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>(),
    );
    let config =
        crate::coding_agent::core::mcp_servers::validate_mcp_server_config("docs", &ordered)
            .expect("config validates");
    let tools: Vec<Value> = vec![
        json!({
            "name": "search",
            "description": "  Search the docs.  ",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] },
        }),
        json!({
            "name": "no.schema",
            "inputSchema": { "properties": {} },
            "title": "Fallback title",
            "annotations": { "title": "Annotation title", "readOnlyHint": true, "destructiveHint": false, "otherHint": true },
        }),
        json!({
            "name": "full",
            "description": "Full tool",
            "inputSchema": { "type": "object" },
            "outputSchema": { "type": "object", "properties": { "answer": { "type": "string" } } },
        }),
        json!({
            "name": "hidden_override",
            "description": "Hidden by toolExposure",
            "inputSchema": { "type": "string" },
        }),
    ];
    let entries = oracle.as_array().unwrap();
    assert_eq!(entries.len(), tools.len());
    for (entry, tool_json) in entries.iter().zip(&tools) {
        let tool = tool_from_json(tool_json);
        let exposure = get_mcp_tool_exposure(&config, &tool.name);
        assert_eq!(json!(exposure_name(exposure)), entry["exposure"]);
        assert_eq!(
            json!(match to_tool_exposure(exposure) {
                crate::coding_agent::extensions::types::ToolExposure::Direct => "direct",
                crate::coding_agent::extensions::types::ToolExposure::Deferred => "deferred",
                crate::coding_agent::extensions::types::ToolExposure::Codemode => "codemode",
                crate::coding_agent::extensions::types::ToolExposure::Hidden => "hidden",
                crate::coding_agent::extensions::types::ToolExposure::ModelOnly => "model-only",
            }),
            entry["toToolExposure"]
        );
        let get_client: super::tools::GetClientHook = Arc::new(|| {
            Box::pin(async {
                Err("not called".to_string())
                    as Result<Arc<dyn super::tools::McpToolCaller>, String>
            })
        });
        let definition = create_mcp_tool_definition(McpToolDefinitionOptions {
            server: "docs".to_string(),
            tool,
            name: create_mcp_tool_name("docs", entry["tool"].as_str().unwrap(), |_| false),
            exposure,
            namespace: crate::coding_agent::extensions::types::ToolNamespace {
                name: "mcp__docs".to_string(),
                description: Some("Docs tools".to_string()),
                instructions: Some("Use search first".to_string()),
            },
            timeout_ms: 45000,
            get_client,
            readable_resources: Some(Arc::new(|| false)),
        });
        let mine = json!({
            "tool": entry["tool"],
            "exposure": exposure_name(exposure),
            "toToolExposure": entry["toToolExposure"],
            "declaration": {
                "name": definition.name,
                "label": definition.label,
                "description": definition.description,
                "parameters": definition.parameters,
                "outputSchema": definition.output_schema,
                "exposure": definition.exposure.as_str(),
                "namespace": definition.namespace,
                "annotations": definition.annotations,
            },
        });
        assert_eq!(mine, *entry, "declaration of {}", entry["tool"]["name"]);
    }
}

// ============================================================================
// result conversion
// ============================================================================

#[derive(Default, Clone)]
struct RecordingSaver {
    calls: Arc<Mutex<Vec<Value>>>,
    fail: bool,
}

impl RecordingSaver {
    fn saver(&self) -> super::tools::McpOutputSaver {
        let calls = Arc::clone(&self.calls);
        let fail = self.fail;
        Arc::new(move |data: super::tools::McpSaveData, extension: &str| {
            let calls = Arc::clone(&calls);
            let extension = extension.to_string();
            Box::pin(async move {
                if fail {
                    return Err("disk on fire".to_string());
                }
                // The capture's stub keeps `text` for string data only
                // (`typeof data === "string" ? data : null`); binary blobs
                // record `null`. `length` is JS `.length`: UTF-16 code units
                // of a string, byte count of a `Uint8Array`.
                let (length, text) = match data {
                    super::tools::McpSaveData::Text(text) => {
                        let length = text.encode_utf16().count();
                        (length, Some(text))
                    }
                    super::tools::McpSaveData::Bytes(bytes) => (bytes.len(), None),
                };
                let mut guard = calls.lock().expect("saver calls");
                let index = guard.len() + 1;
                guard.push(json!({
                    "extension": extension,
                    "length": length,
                    "text": text,
                }));
                Ok(format!("/tmp/fake-output-{index}{extension}"))
            })
        })
    }
}

#[tokio::test]
async fn oracle_result_conversion() {
    let oracle = observed("result_conversion");
    let raw_cases = oracle["conversions"].as_array().unwrap();
    // Reconstruct the inputs per name (the oracle stores only outputs).
    let inputs = conversion_inputs();
    assert_eq!(inputs.len(), raw_cases.len(), "case coverage");
    let saver = RecordingSaver::default();
    let failing = RecordingSaver {
        fail: true,
        ..RecordingSaver::default()
    };
    for raw in raw_cases {
        let name = raw["name"].as_str().unwrap();
        let (_input_name, server, tool, result_json, use_failing, readable) = inputs
            .iter()
            .find(|(input_name, ..)| input_name == name)
            .unwrap_or_else(|| panic!("input for {name}"));
        let result: CallToolResult = serde_json::from_value({
            let mut object = result_json.clone();
            object
                .as_object_mut()
                .unwrap()
                .entry("content")
                .or_insert(json!([]));
            object
        })
        .expect("result shape");
        let options = ConvertMcpResultOptions {
            save_output: Some(if *use_failing {
                failing.saver()
            } else {
                saver.saver()
            }),
            readable_resources: *readable,
        };
        let value = convert_mcp_result(server, tool, &result, &options)
            .await
            .expect("conversion succeeds");
        assert_eq!(value, raw["value"], "conversion of {name}");
    }
    // The recorded saver calls must match the oracle's transcript.
    let mine_calls = saver.calls.lock().expect("calls").clone();
    let oracle_calls = oracle["saverCalls"].as_array().unwrap();
    assert_eq!(
        mine_calls.len(),
        oracle_calls.len(),
        "saver call count: mine {mine_calls:?} oracle {oracle_calls:?}"
    );
    for (mine, expected) in mine_calls.iter().zip(oracle_calls) {
        assert_eq!(mine["extension"], expected["extension"]);
        assert_eq!(mine["length"], expected["length"]);
        // Text calls compare byte-for-byte; binary calls carry null text.
        assert_eq!(mine["text"], expected["text"], "saver text of {mine}");
    }
}

/// The conversion inputs of the capture script, verbatim.
fn conversion_inputs() -> Vec<(String, String, String, Value, bool, bool)> {
    vec![
        (
            "plain_text".into(),
            "docs".into(),
            "search".into(),
            json!({ "content": [{ "type": "text", "text": "hello" }] }),
            false,
            false,
        ),
        (
            "multi_block_join".into(),
            "docs".into(),
            "search".into(),
            json!({ "content": [{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }] }),
            false,
            false,
        ),
        (
            "image".into(),
            "docs".into(),
            "pic".into(),
            json!({ "content": [{ "type": "image", "data": "Zm9v", "mimeType": "image/png" }] }),
            false,
            false,
        ),
        (
            "audio".into(),
            "docs".into(),
            "say".into(),
            json!({ "content": [{ "type": "audio", "data": "Zm9v", "mimeType": "audio/wav" }] }),
            false,
            false,
        ),
        (
            "resource_link".into(),
            "docs".into(),
            "link".into(),
            json!({ "content": [{ "type": "resource_link", "uri": "file:///a.txt", "name": "a.txt", "title": "A", "mimeType": "text/plain", "size": 2048, "description": "A file" }] }),
            false,
            true,
        ),
        (
            "resource_link_unreadable".into(),
            "docs".into(),
            "link".into(),
            json!({ "content": [{ "type": "resource_link", "uri": "file:///a.txt", "name": "a.txt" }] }),
            false,
            false,
        ),
        (
            "embedded_text_resource".into(),
            "docs".into(),
            "read".into(),
            json!({ "content": [{ "type": "resource", "resource": { "uri": "file:///a.txt", "mimeType": "text/plain", "text": "contents" } }] }),
            false,
            false,
        ),
        (
            "embedded_json_blob".into(),
            "docs".into(),
            "read".into(),
            json!({ "content": [{ "type": "resource", "resource": { "uri": "https://x/a.json", "mimeType": "application/json", "blob": "eyJrIjoxfQ==" } }] }),
            false,
            false,
        ),
        (
            "embedded_binary_blob".into(),
            "docs".into(),
            "read".into(),
            json!({ "content": [{ "type": "resource", "resource": { "uri": "https://x/a.bin", "mimeType": "application/octet-stream", "blob": "AAECAw==" } }] }),
            false,
            false,
        ),
        (
            "embedded_binary_save_fails".into(),
            "docs".into(),
            "read".into(),
            json!({ "content": [{ "type": "resource", "resource": { "uri": "https://x/a.bin", "blob": "CQ==" } }] }),
            true,
            false,
        ),
        (
            "structured_only".into(),
            "docs".into(),
            "structured".into(),
            json!({ "structuredContent": { "answer": 42 } }),
            false,
            false,
        ),
        (
            "is_error_no_text".into(),
            "docs".into(),
            "boom".into(),
            json!({ "content": [], "structuredContent": { "code": 7 }, "isError": true }),
            false,
            false,
        ),
        (
            "is_error_with_text".into(),
            "docs".into(),
            "boom".into(),
            json!({ "content": [{ "type": "text", "text": "explicit failure" }], "isError": true }),
            false,
            false,
        ),
        (
            "_meta_dropped".into(),
            "docs".into(),
            "meta".into(),
            json!({ "content": [{ "type": "text", "text": "x" }], "_meta": { "hidden": true } }),
            false,
            false,
        ),
        (
            "empty".into(),
            "docs".into(),
            "empty".into(),
            json!({}),
            false,
            false,
        ),
        (
            "truncated".into(),
            "docs".into(),
            "big".into(),
            json!({ "content": [{ "type": "text", "text": "a".repeat(30000) }, { "type": "image", "data": "Zm9v", "mimeType": "image/png" }] }),
            false,
            false,
        ),
        (
            "truncate_save_fails".into(),
            "docs".into(),
            "big".into(),
            json!({ "content": [{ "type": "text", "text": "b".repeat(30000) }] }),
            true,
            false,
        ),
    ]
}

// ============================================================================
// resources tools
// ============================================================================

struct FakeServer {
    name: String,
    fail_page: bool,
    fail_all: bool,
    fail_read: bool,
    resources: Vec<Resource>,
    templates: Vec<ResourceTemplate>,
    next_cursor: Option<String>,
    contents: Vec<serde_json::Map<String, Value>>,
}

impl McpResourceServer for FakeServer {
    fn name(&self) -> &str {
        &self.name
    }

    fn timeout_ms(&self) -> u64 {
        1234
    }

    fn resources_page(
        &self,
        cursor: Option<String>,
        _options: McpRequestOptions,
    ) -> futures::future::BoxFuture<'static, Result<ListResourcesResult, String>> {
        let items = if cursor.is_some() {
            self.resources[1..].to_vec()
        } else {
            self.resources.clone()
        };
        let next_cursor = if cursor.is_some() {
            None
        } else {
            self.next_cursor.clone()
        };
        let fail = self.fail_page;
        let name = self.name.clone();
        Box::pin(async move {
            if fail {
                return Err(format!("page failed: {name}"));
            }
            Ok(ListResourcesResult {
                resources: items,
                next_cursor,
                meta: None,
            })
        })
    }

    fn resource_templates_page(
        &self,
        cursor: Option<String>,
        _options: McpRequestOptions,
    ) -> futures::future::BoxFuture<'static, Result<ListResourceTemplatesResult, String>> {
        let items = if cursor.is_some() {
            self.templates[1..].to_vec()
        } else {
            self.templates.clone()
        };
        let next_cursor = if cursor.is_some() {
            None
        } else {
            self.next_cursor.clone()
        };
        let fail = self.fail_page;
        let name = self.name.clone();
        Box::pin(async move {
            if fail {
                return Err(format!("templates failed: {name}"));
            }
            Ok(ListResourceTemplatesResult {
                resource_templates: items,
                next_cursor,
                meta: None,
            })
        })
    }

    fn all_resources(
        &self,
        _options: McpRequestOptions,
    ) -> futures::future::BoxFuture<'static, Result<Vec<Resource>, String>> {
        let items = self.resources.clone();
        let fail = self.fail_all;
        let name = self.name.clone();
        Box::pin(async move {
            if fail {
                Err(format!("all failed: {name}"))
            } else {
                Ok(items)
            }
        })
    }

    fn all_resource_templates(
        &self,
        _options: McpRequestOptions,
    ) -> futures::future::BoxFuture<'static, Result<Vec<ResourceTemplate>, String>> {
        let items = self.templates.clone();
        let fail = self.fail_all;
        let name = self.name.clone();
        Box::pin(async move {
            if fail {
                Err(format!("all templates failed: {name}"))
            } else {
                Ok(items)
            }
        })
    }

    fn read_resource(
        &self,
        uri: &str,
        _options: McpRequestOptions,
    ) -> futures::future::BoxFuture<'static, Result<ReadResourceResult, String>> {
        let fail = self.fail_read;
        let name = self.name.clone();
        let contents = if uri == "file:///empty" {
            Vec::new()
        } else if self.contents.is_empty() {
            vec![serde_json::Map::from_iter([
                ("uri".to_string(), json!(uri)),
                ("mimeType".to_string(), json!("text/plain")),
                ("text".to_string(), json!(format!("content of {uri}"))),
            ])]
        } else {
            self.contents.clone()
        };
        Box::pin(async move {
            if fail {
                return Err(format!("read failed: {name}"));
            }
            Ok(ReadResourceResult {
                contents,
                meta: None,
                extra: Default::default(),
            })
        })
    }
}

fn fake_servers() -> Vec<Arc<dyn McpResourceServer>> {
    let alpha_resources: Vec<Resource> = [
        json!({ "uri": "ui://app/main", "name": "app", "mimeType": "text/html" }),
        json!({ "uri": "file:///a.txt", "name": "a.txt", "title": "A", "description": "File a", "mimeType": "text/plain", "size": 3, "_meta": { "hidden": true }, "icons": [{ "src": "x" }] }),
        json!({ "uri": "file:///b.html", "name": "b.html", "mimeType": "application/xhtml+xml; profile=mcp-app" }),
        json!({ "uri": "file:///c.md", "name": "c.md", "mimeType": "text/markdown" }),
    ]
    .into_iter()
    .map(|value| serde_json::from_value(value).expect("resource"))
    .collect();
    let alpha_templates: Vec<ResourceTemplate> = [
        json!({ "uriTemplate": "file:///{path}", "name": "path", "_meta": { "x": 1 } }),
        json!({ "uriTemplate": "ui://app/{view}", "name": "view" }),
    ]
    .into_iter()
    .map(|value| serde_json::from_value(value).expect("template"))
    .collect();
    let contents: Vec<serde_json::Map<String, Value>> = [
        json!({ "uri": "file:///a.txt", "mimeType": "text/plain", "text": "first" }),
        json!({ "uri": "file:///b.txt", "mimeType": "text/plain", "text": "second" }),
    ]
    .into_iter()
    .map(|value| value.as_object().expect("object").clone())
    .collect();
    vec![
        Arc::new(FakeServer {
            name: "alpha".to_string(),
            fail_page: false,
            fail_all: false,
            fail_read: false,
            resources: alpha_resources,
            templates: alpha_templates,
            next_cursor: Some("page-2".to_string()),
            contents,
        }),
        Arc::new(FakeServer {
            name: "Beta".to_string(),
            fail_page: true,
            fail_all: true,
            fail_read: true,
            resources: vec![
                serde_json::from_value(json!({ "uri": "file:///z.bin", "name": "z.bin", "mimeType": "application/octet-stream" }))
                    .expect("resource"),
            ],
            templates: Vec::new(),
            next_cursor: None,
            contents: Vec::new(),
        }),
    ]
}

fn make_execute_context() -> crate::coding_agent::extensions::types::ExtensionContext {
    use crate::coding_agent::extensions::loader::{ExtensionRuntime, NullModuleLoader};
    use crate::coding_agent::extensions::runner::ExtensionRunner;
    let runner = ExtensionRunner::new(
        Vec::new(),
        ExtensionRuntime::new(),
        "/workspace",
        Arc::new(NullModuleLoader),
        Arc::new(crate::coding_agent::extensions::types::NoopProviderRegistry),
    );
    runner.create_context()
}

#[tokio::test]
async fn oracle_resources_tools() {
    let oracle = observed("resources_tools");
    let servers = fake_servers();
    let definitions = create_mcp_resource_tool_definitions(McpResourceToolOptions {
        exposure: McpExposure::Direct,
        servers: Arc::new(move || {
            servers
                .iter()
                .map(|server| Arc::clone(server) as Arc<dyn McpResourceServer>)
                .collect()
        }),
    });
    let mine_definitions: Vec<Value> = definitions
        .iter()
        .map(|definition| {
            json!({
                "name": definition.name,
                "label": definition.label,
                "description": definition.description,
                "parameters": definition.parameters,
                "outputSchema": definition.output_schema,
                "exposure": definition.exposure.as_str(),
                "annotations": definition.annotations,
            })
        })
        .collect();
    assert_eq!(
        mine_definitions,
        oracle["definitions"]
            .as_array()
            .expect("definitions array")
            .as_slice(),
        "definitions"
    );

    let execute = |definition: &crate::coding_agent::extensions::types::ToolDefinition,
                   params: Value| {
        let definition = definition.clone();
        let ctx = make_execute_context();
        async move {
            match definition.execute_async.as_ref().expect("execute")(
                "call-1".to_string(),
                params,
                None,
                None,
                ctx,
            )
            .await
            {
                Ok(result) => {
                    let mut value = result;
                    // Normalize the shape to the oracle's projection.
                    let object = value.as_object_mut().expect("object");
                    let is_error = object.get("isError").cloned().unwrap_or(Value::Null);
                    json!({
                        "ok": true,
                        "content": object.get("content").cloned().unwrap_or(Value::Null),
                        "details": object.get("details").cloned().unwrap_or(Value::Null),
                        "structuredContent": object.get("structuredContent").cloned().unwrap_or(Value::Null),
                        "isError": is_error,
                    })
                }
                Err(error) => json!({ "ok": false, "error": error }),
            }
        }
    };
    // Note: the fake list call re-resolves the servers per invocation.
    let servers = fake_servers();
    let definitions = create_mcp_resource_tool_definitions(McpResourceToolOptions {
        exposure: McpExposure::Direct,
        servers: Arc::new(move || {
            servers
                .iter()
                .map(|server| Arc::clone(server) as Arc<dyn McpResourceServer>)
                .collect()
        }),
    });
    let [list_resources, list_templates, read_resource] =
        [&definitions[0], &definitions[1], &definitions[2]];
    let cases: Vec<(
        &str,
        &crate::coding_agent::extensions::types::ToolDefinition,
        Value,
    )> = vec![
        ("list_all", list_resources, json!({})),
        ("list_one", list_resources, json!({ "server": "alpha" })),
        (
            "list_one_page2",
            list_resources,
            json!({ "server": "alpha", "cursor": "page-2" }),
        ),
        (
            "list_unknown_server",
            list_resources,
            json!({ "server": "gamma" }),
        ),
        (
            "list_cursor_without_server",
            list_resources,
            json!({ "cursor": "page-2" }),
        ),
        ("list_bad_param", list_resources, json!({ "server": 5 })),
        ("templates_all", list_templates, json!({})),
        (
            "templates_one",
            list_templates,
            json!({ "server": "alpha" }),
        ),
        (
            "read_multi",
            read_resource,
            json!({ "server": "alpha", "uri": "file:///a.txt" }),
        ),
        (
            "read_single",
            read_resource,
            json!({ "server": "alpha", "uri": "file:///only" }),
        ),
        (
            "read_empty",
            read_resource,
            json!({ "server": "alpha", "uri": "file:///empty" }),
        ),
        ("read_missing_args", read_resource, json!({})),
        (
            "read_unknown_server",
            read_resource,
            json!({ "server": "gamma", "uri": "x" }),
        ),
        (
            "read_failed",
            read_resource,
            json!({ "server": "Beta", "uri": "x" }),
        ),
    ];
    for (name, definition, params) in cases {
        let mine = execute(definition, params).await;
        let expected = &oracle[name];
        assert_eq!(mine, *expected, "resources case {name}");
    }
}

// ============================================================================
// log format
// ============================================================================

#[test]
fn oracle_log_format() {
    let oracle = observed("log_format");
    let now = Zoned::from_epoch_ms(1758240000000);
    let format = |params: Value| format_mcp_log_message("docs", &params, &now);
    assert_eq!(
        format(json!({ "level": "debug", "logger": "db", "data": "ready" })),
        oracle["plain"]
    );
    assert_eq!(format(json!({ "data": { "a": 1 } })), oracle["defaults"]);
    assert_eq!(format(json!({ "level": "warn" })), oracle["no_data"]);
    assert_eq!(format(json!("raw string")), oracle["non_record"]);
    assert_eq!(format(json!(42)), oracle["number_data"]);
    assert_eq!(format(json!({ "data": "a\nb\r\nc" })), oracle["newlines"]);
    assert_eq!(
        format(json!({ "logger": "", "data": "x" })),
        oracle["empty_logger"]
    );
}

// ============================================================================
// truncate middle + format size
// ============================================================================

#[test]
fn oracle_truncate_middle() {
    let oracle = observed("truncate_middle");
    let truncation_json = |result: &super::tools::MiddleTruncation| {
        json!({
            "content": result.content,
            "truncated": result.truncated,
            "removedChars": result.removed_chars,
            "totalBytes": result.total_bytes,
            "totalLines": result.total_lines,
        })
    };
    assert_eq!(
        truncation_json(&super::tools::truncate_middle("hello world", 100)),
        oracle["short"]
    );
    assert_eq!(
        truncation_json(&super::tools::truncate_middle("hello world", 11)),
        oracle["exact"]
    );
    assert_eq!(
        truncation_json(&super::tools::truncate_middle("hello world", 5)),
        oracle["ascii"]
    );
    assert_eq!(
        truncation_json(&super::tools::truncate_middle(
            "h\u{00e9}llo w\u{00f6}rld \u{2014} \u{00fc}n\u{00ef}code \u{2713}\u{2713}",
            10
        )),
        oracle["multibyte"]
    );
    assert_eq!(
        truncation_json(&super::tools::truncate_middle("a\nb\nc\n", 4)),
        oracle["lines"]
    );
    let sizes: Vec<Value> = [0u64, 512, 1023, 1024, 1025, 1048575, 1048576, 2097152]
        .iter()
        .map(|bytes| {
            json!(crate::agent_core::harness::utils::truncate::format_size(
                *bytes
            ))
        })
        .collect();
    assert_eq!(Value::Array(sizes), oracle["sizes"]);
}

// ============================================================================
// Server state strings (used by status formatting; pinned inline)
// ============================================================================

#[test]
fn server_state_strings() {
    assert_eq!(ServerState::Connecting.as_str(), "connecting");
    assert_eq!(ServerState::Connected.as_str(), "connected");
    assert_eq!(ServerState::Disconnected.as_str(), "disconnected");
    assert_eq!(ServerState::NeedsAuth.as_str(), "needs-auth");
    assert_eq!(ServerState::Failed.as_str(), "failed");
    assert_eq!(ServerState::Closed.as_str(), "closed");
}
