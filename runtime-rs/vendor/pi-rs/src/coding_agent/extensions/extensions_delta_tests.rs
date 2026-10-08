//! Replay of the extensions-delta oracle
//! (`tests/fixtures/extensions_delta_oracle/oracle/extensions_delta_oracle.json`),
//! captured from the verbatim upstream HEAD (`pi@2bbfcca43`, v0.99.1)
//! `core/extensions/{types,runner,loader}.ts`, `core/mcp-servers.ts`,
//! `core/source-info.ts`, `extensions/tool-search/*`, and the pi-ai
//! transcript utils under node (type stripping); see the fixture's
//! manifest.json for SHA-256 provenance and `oracle/capture.mjs` for the
//! exact scenario definitions.
//!
//! Known port divergences pinned here (disclosed in the port report):
//! - `isToolSearchTool` compares parameter VALUES structurally; upstream
//!   compares object identity, so the `isTool_structuralClone` sub-check of
//!   `tool_search_schema` is `false` upstream and `true` here.
//! - Handler `error.stack` is always `None` in the port; the oracle pins JS
//!   stack presence only (`<js-stack:true>`), so stack fields are excluded
//!   from error comparisons.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::loader::{
    load_extension_from_factory, load_extensions, ExtensionRuntime, NullModuleLoader,
};
use super::runner::ExtensionRunner;
use super::tool_search::{
    create_tool_search_document, create_tool_search_extension, create_tool_search_tool_definition,
    tokenize, Bm25Ranker, ToolRanker, TOOL_SEARCH_DESCRIPTION,
};
use super::types::{self, ExecuteToolOptions, ToolExposure, ToolInfo, ToolNamespace};
use crate::coding_agent::core::event_bus::EventBusController;

fn oracle() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/extensions_delta_oracle/oracle/extensions_delta_oracle.json"
    ))
    .expect("extensions delta oracle parses")
}

fn expected(name: &str) -> Value {
    oracle()["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scenario| scenario["name"] == name)
        .unwrap_or_else(|| panic!("scenario {name} missing from extensions delta oracle"))
        ["observed"]
        .clone()
}

fn bus() -> crate::coding_agent::core::event_bus::EventBus {
    EventBusController::new().bus().clone()
}

const CWD: &str = "/workspace";

fn shared_actions() -> Arc<types::ExtensionActions> {
    Arc::new(types::ExtensionActions {
        send_message: Arc::new(|_, _| {}),
        send_user_message: Arc::new(|_, _| {}),
        append_entry: Arc::new(|_, _| {}),
        set_session_name: Arc::new(|_| {}),
        get_session_name: Arc::new(|| None),
        set_label: Arc::new(|_, _| {}),
        get_active_tools: Arc::new(Vec::new),
        get_all_tools: Arc::new(Vec::new),
        get_settings: Arc::new(|| json!({})),
        set_active_tools: Arc::new(|_| {}),
        refresh_tools: Arc::new(|| {}),
        get_commands: Arc::new(Vec::new),
        set_model: Arc::new(|_| Ok(types::CommandFuture::resolved(false))),
        get_thinking_level: Arc::new(|| types::ThinkingLevel::Off),
        set_thinking_level: Arc::new(|_| {}),
    })
}

fn context_actions() -> Arc<types::ExtensionContextActions> {
    Arc::new(types::ExtensionContextActions {
        get_model: Arc::new(|| None),
        get_scoped_models: Arc::new(Vec::new),
        is_idle: Arc::new(|| true),
        is_project_trusted: Arc::new(|| true),
        get_signal: Arc::new(|| None),
        abort: Arc::new(|| {}),
        has_pending_messages: Arc::new(|| false),
        shutdown: Arc::new(|| {}),
        get_context_usage: Arc::new(|| None),
        compact: Arc::new(|_| {}),
        get_system_prompt: Arc::new(String::new),
        get_system_prompt_options: None,
        execute_tool: None,
        get_callable_tools: None,
    })
}

fn make_runner(
    extensions: Vec<types::Extension>,
    runtime: ExtensionRuntime,
    provider_actions: Option<super::runner::ProviderActions>,
) -> ExtensionRunner {
    let runner = ExtensionRunner::new(
        extensions,
        runtime,
        CWD,
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    runner.bind_core(shared_actions(), context_actions(), provider_actions);
    runner
}

// ============================================================================
// Part 1 — tool_search
// ============================================================================

#[test]
fn tool_search_tokenize() {
    for case in expected("tool_search_tokenize").as_array().unwrap() {
        let tokens = tokenize(case["text"].as_str().unwrap());
        assert_eq!(
            json!(tokens),
            case["tokens"],
            "tokenize({:?})",
            case["text"]
        );
    }
}

#[test]
fn tool_search_documents() {
    let namespace = ToolNamespace {
        name: "mcp__docs".to_string(),
        description: Some("Documentation server\nsecond line".to_string()),
        instructions: Some("Use for doc lookups.".to_string()),
    };
    let schema = super::tool_search::tool_search_schema();
    let documents = [
        create_tool_search_document("issue_list", "List repository issues", &schema, None),
        create_tool_search_document(
            "deploy",
            "Deploy a service",
            &json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Deployment target" },
                    "variants": { "anyOf": [{ "type": "string", "description": "A variant" }] },
                    "tags": { "type": "array", "items": { "type": "string", "description": "Tag names" } },
                },
            }),
            None,
        ),
        create_tool_search_document(
            "search_docs",
            "Search documentation",
            &schema,
            Some(&namespace),
        ),
        create_tool_search_document("bare", "  ", &json!({ "type": "object" }), None),
    ];
    let observed: Vec<Value> = documents
        .iter()
        .map(|document| json!({ "name": document.name, "text": document.text }))
        .collect();
    assert_eq!(json!(observed), expected("tool_search_documents"));
}

#[test]
fn tool_search_bm25() {
    use super::tool_search::ToolSearchDocument;
    let documents = vec![
        ToolSearchDocument {
            name: "issue_list".into(),
            text: "issue_list issue list list repository issues".into(),
        },
        ToolSearchDocument {
            name: "issue_close".into(),
            text: "issue_close issue close repository issue".into(),
        },
        ToolSearchDocument {
            name: "deploy_service".into(),
            text: "deploy_service deploy service deployment target".into(),
        },
        ToolSearchDocument {
            name: "search_docs".into(),
            text: "search_docs search documentation docs".into(),
        },
    ];
    let expected_bm25 = expected("tool_search_bm25");
    let to_json = |matches: &[super::tool_search::ToolSearchMatch]| {
        json!(matches
            .iter()
            .map(|m| json!({ "name": m.name, "score": m.score }))
            .collect::<Vec<_>>())
    };
    let rank = |query: &str, limit: usize, docs: &[ToolSearchDocument]| {
        to_json(&Bm25Ranker::new().rank(query, docs, limit))
    };
    assert_eq!(rank("issue", 10, &documents), expected_bm25["issue"]);
    assert_eq!(rank("issues", 10, &documents), expected_bm25["issues_stem"]);
    assert_eq!(
        rank("issue repository", 2, &documents),
        expected_bm25["limit2"]
    );
    assert_eq!(rank("issue", 0, &documents), expected_bm25["limit0"]);
    // Upstream -3; usize cannot be negative, rank(0) already covers the guard.
    assert_eq!(rank("issue", 0, &documents), expected_bm25["limitNegative"]);
    assert_eq!(rank("", 10, &documents), expected_bm25["emptyQuery"]);
    assert_eq!(rank("zzzqqq", 10, &documents), expected_bm25["noMatch"]);
    assert_eq!(rank("issue", 10, &[]), expected_bm25["emptyDocs"]);
    assert_eq!(
        rank("deploy service", 10, &documents[2..3]),
        expected_bm25["singleDoc"]
    );
    assert_eq!(
        rank(
            "shared",
            10,
            &[
                ToolSearchDocument {
                    name: "alpha".into(),
                    text: "shared term".into()
                },
                ToolSearchDocument {
                    name: "beta".into(),
                    text: "shared term".into()
                },
                ToolSearchDocument {
                    name: "gamma".into(),
                    text: "shared term".into()
                },
            ]
        ),
        expected_bm25["tieOrder"]
    );
    let custom = Bm25Ranker::with_parameters(2.0, 0.5).rank("issue", &documents, 10);
    assert_eq!(to_json(&custom), expected_bm25["customParams"]);
}

#[test]
fn tool_search_description() {
    // v1.0.0 replaced `createToolSearchDescription(sources)` with the constant
    // `TOOL_SEARCH_DESCRIPTION`; the captured oracle case predates it and the
    // per-source listing it pinned no longer exists (the description stays
    // stable while tools register).
    assert!(TOOL_SEARCH_DESCRIPTION.starts_with("# Tool discovery\n\n"));
    assert!(TOOL_SEARCH_DESCRIPTION.contains(
        "Some of the tools, such as tools of MCP servers, may not have been provided to you upfront",
    ));
    assert!(TOOL_SEARCH_DESCRIPTION.contains("always use `tool_search`."));
    assert!(!TOOL_SEARCH_DESCRIPTION.contains("None currently enabled."));
}

#[test]
fn tool_search_schema() {
    let expected_schema = expected("tool_search_schema");
    let schema = super::tool_search::tool_search_schema();
    assert_eq!(schema, expected_schema["schema"]);
    assert_eq!(
        super::tool_search::DEFAULT_TOOL_SEARCH_LIMIT,
        expected_schema["defaultLimit"]
    );
    assert_eq!(
        super::tool_search::TOOL_SEARCH_TOOL_NAME,
        expected_schema["toolName"]
    );
    assert!(super::tool_search::is_tool_search_tool(
        "tool_search",
        &schema
    ));
    // Disclosed divergence: a structural clone is a different JS object
    // upstream (`false`) but compares equal by value here.
    assert!(super::tool_search::is_tool_search_tool(
        "tool_search",
        &schema.clone()
    ));
    assert!(!expected_schema["isTool_structuralClone"].as_bool().unwrap());
    assert_eq!(
        super::tool_search::is_tool_search_tool("other", &schema),
        expected_schema["isTool_otherName"]
    );
}

struct FakeTools {
    all: Vec<ToolInfo>,
    active: Mutex<Vec<String>>,
    set_active_calls: Mutex<Vec<Vec<String>>>,
}

impl FakeTools {
    fn new(all: Vec<ToolInfo>, active: &[&str]) -> Self {
        Self {
            all,
            active: Mutex::new(active.iter().map(|name| name.to_string()).collect()),
            set_active_calls: Mutex::new(Vec::new()),
        }
    }
}

impl super::tool_search::ToolSearchTools for FakeTools {
    fn get_all_tools(&self) -> Vec<ToolInfo> {
        self.all.clone()
    }
    fn get_active_tools(&self) -> Vec<String> {
        self.active.lock().unwrap().clone()
    }
    fn set_active_tools(&self, tool_names: &[String]) {
        self.set_active_calls
            .lock()
            .unwrap()
            .push(tool_names.to_vec());
        *self.active.lock().unwrap() = tool_names.to_vec();
    }
}

fn fixture_tool_infos() -> Vec<ToolInfo> {
    let schema = super::tool_search::tool_search_schema();
    let info = |name: &str, description: &str, exposure: ToolExposure| ToolInfo {
        name: name.to_string(),
        description: description.to_string(),
        parameters: schema.clone(),
        prompt_guidelines: None,
        exposure,
        namespace: None,
        annotations: None,
        source_info: types::create_synthetic_source_info("builtin", "builtin", None, None, None),
    };
    vec![
        info("direct_active", "Already declared", ToolExposure::Direct),
        info(
            "cm_tool",
            "Codemode tool first line\nmore",
            ToolExposure::Codemode,
        ),
        info("def_tool", "Deferred tool", ToolExposure::Deferred),
        info("cm_active", "Codemode but active", ToolExposure::Codemode),
        info("hidden_tool", "Hidden", ToolExposure::Hidden),
        info("model_only", "Model only", ToolExposure::ModelOnly),
    ]
}

fn execute_tool_search(
    tools: Option<Arc<FakeTools>>,
    query: &str,
    limit: Option<Value>,
) -> Result<Value, String> {
    let options = match tools {
        Some(tools) => super::tool_search::ToolSearchToolOptions { tools: Some(tools) },
        None => super::tool_search::ToolSearchToolOptions::default(),
    };
    let definition = create_tool_search_tool_definition(options);
    let execute = definition.execute.expect("tool_search execute");
    let mut args = json!({ "query": query });
    if let Some(limit) = limit {
        args["limit"] = limit;
    }
    execute("call1", &args, None, None, &make_context())
}

fn make_context() -> types::ExtensionContext {
    // The tool_search execute closure ignores its context; a bare context
    // stands in.
    make_runner(Vec::new(), ExtensionRuntime::new(), None).create_context()
}

#[test]
fn tool_search_execute_load() {
    let tools = Arc::new(FakeTools::new(
        fixture_tool_infos(),
        &["direct_active", "cm_active"],
    ));
    let expected_load = expected("tool_search_execute_load");
    let result =
        execute_tool_search(Some(Arc::clone(&tools)), "deferred codemode tool", None).unwrap();
    assert_eq!(result, expected_load["result"]);
    assert_eq!(
        json!(tools.set_active_calls.lock().unwrap().clone()),
        expected_load["setActiveCalls"]
    );
}

#[test]
fn tool_search_execute_no_match() {
    let tools = Arc::new(FakeTools::new(fixture_tool_infos(), &["direct_active"]));
    let expected_no_match = expected("tool_search_execute_no_match");
    let result = execute_tool_search(Some(Arc::clone(&tools)), "zzzqqq nothing", None).unwrap();
    assert_eq!(result, expected_no_match["result"]);
    assert!(tools.set_active_calls.lock().unwrap().is_empty());
}

#[test]
fn tool_search_execute_no_tools_option() {
    let result = execute_tool_search(None, "anything", None).unwrap();
    assert_eq!(
        result,
        expected("tool_search_execute_no_tools_option")["result"]
    );
}

#[test]
fn tool_search_execute_validation() {
    let expected_validation = expected("tool_search_execute_validation");
    let run = |query: &str, limit: Option<Value>| {
        execute_tool_search(
            Some(Arc::new(FakeTools::new(Vec::new(), &[]))),
            query,
            limit,
        )
        .unwrap_err()
    };
    assert_eq!(
        run("   ", None),
        expected_validation["emptyQuery"]["threw"].as_str().unwrap()
    );
    assert_eq!(
        run("query", Some(json!(0))),
        expected_validation["limitZero"]["threw"].as_str().unwrap()
    );
    assert_eq!(
        run("query", Some(json!(2.5))),
        expected_validation["limitFloat"]["threw"].as_str().unwrap()
    );
    assert_eq!(
        run("query", Some(json!(-1))),
        expected_validation["limitNegative"]["threw"]
            .as_str()
            .unwrap()
    );
}

#[test]
fn tool_search_execute_limit_one() {
    let tools = Arc::new(FakeTools::new(
        fixture_tool_infos(),
        &["direct_active", "cm_active"],
    ));
    let expected_limit = expected("tool_search_execute_limit_one");
    let result = execute_tool_search(
        Some(Arc::clone(&tools)),
        "deferred codemode tool",
        Some(json!(1)),
    )
    .unwrap();
    assert_eq!(result, expected_limit["result"]);
    assert_eq!(
        json!(tools.set_active_calls.lock().unwrap().clone()),
        expected_limit["setActiveCalls"]
    );
}

fn placeholder_tool(name: &str) -> crate::agent_core::types::AgentTool {
    crate::agent_core::types::AgentTool {
        name: name.to_string(),
        label: String::new(),
        description: String::new(),
        parameters: Value::Null,
        constrained_sampling: None,
        execute: Arc::new(|_, _, _, _| Box::pin(async { unreachable!() })),
        prepare_arguments: None,
        replay: None,
        execution_mode: None,
    }
}

// v1.0.0 removed `tool_search`'s `prepareLoadout` (the description no longer
// lists the searchable namespaces); the `tool_search_prepare_loadout` oracle
// case and its replay went with it.

#[test]
fn tool_search_extension_registration() {
    let expected_registration = expected("tool_search_extension_registration");
    let runtime = ExtensionRuntime::new();
    let extension = load_extension_from_factory(
        create_tool_search_extension(),
        CWD,
        bus(),
        &runtime,
        Some("builtin:tool-search"),
    )
    .unwrap();
    let registered = extension.tools.get("tool_search").expect("tool registered");
    let definition = &registered.definition;
    assert_eq!(extension.path, expected_registration["extensionPath"]);
    assert_eq!(
        serde_json::to_value(&extension.source_info).unwrap(),
        expected_registration["sourceInfo"]
    );
    assert_eq!(
        definition.default_active,
        expected_registration["defaultActive"].as_bool()
    );
    let observed = json!({
        "name": definition.name,
        "label": definition.label,
        "description": definition.description,
        "promptSnippet": definition.prompt_snippet,
        "parameters": definition.parameters,
        "exposure": definition.exposure.as_str(),
        "hasExecute": definition.execute.is_some(),
        "hasPrepareLoadout": definition.prepare_loadout.is_some(),
    });
    assert_eq!(observed, expected_registration["definition"]);
}

// ============================================================================
// Part 2 — runner/loader delta
// ============================================================================

fn config_map(pairs: Vec<(&str, Value)>) -> crate::ai::types::ordered_map::OrderedMap<Value> {
    crate::ai::types::ordered_map::OrderedMap::from_pairs(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    )
}

#[test]
fn register_mcp_server_surface() {
    let expected_surface = expected("register_mcp_server_surface");
    let runtime = ExtensionRuntime::new();
    let one_out: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let out = Arc::clone(&one_out);
        load_extension_from_factory(
            Arc::new(move |pi| {
                let valid = pi.register_mcp_server(
                    "jira",
                    &config_map(vec![
                        ("url", json!("https://mcp.example.com/jira")),
                        ("exposure", json!("codemode-deferred")),
                    ]),
                );
                let invalid =
                    pi.register_mcp_server("bad name!", &config_map(vec![("command", json!("x"))]));
                let after_register = pi.get_mcp_servers();
                out.lock().unwrap().push(match valid {
                    Ok(()) => json!("ok"),
                    Err(message) => json!(message),
                });
                out.lock().unwrap().push(match invalid {
                    Ok(()) => json!("ok"),
                    Err(message) => json!(message),
                });
                out.lock().unwrap().push(match after_register {
                    Ok(servers) => json!(servers.len()),
                    Err(message) => json!(message),
                });
                Ok(())
            }),
            CWD,
            bus(),
            &runtime,
            Some("<inline:one>"),
        )
        .unwrap();
    }
    let two_out: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let out = Arc::clone(&two_out);
        load_extension_from_factory(
            Arc::new(move |pi| {
                let conflict =
                    pi.register_mcp_server("jira", &config_map(vec![("command", json!("y"))]));
                let own_replace = (|| -> Result<Value, String> {
                    pi.register_mcp_server("own", &config_map(vec![("command", json!("a"))]))?;
                    pi.register_mcp_server("own", &config_map(vec![("command", json!("b"))]))?;
                    Ok(json!(pi
                        .get_mcp_servers()?
                        .iter()
                        .map(|server| server.name.clone())
                        .collect::<Vec<_>>()))
                })();
                let foreign_unregister = pi.unregister_mcp_server("jira");
                let after_foreign = json!(pi
                    .get_mcp_servers()?
                    .iter()
                    .map(|server| server.name.clone())
                    .collect::<Vec<_>>());
                let own_unregister = (|| -> Result<Value, String> {
                    pi.unregister_mcp_server("own")?;
                    Ok(json!(pi
                        .get_mcp_servers()?
                        .iter()
                        .map(|server| server.name.clone())
                        .collect::<Vec<_>>()))
                })();
                let push = |value: Value| out.lock().unwrap().push(value);
                push(match conflict {
                    Ok(()) => json!("ok"),
                    Err(message) => json!(message),
                });
                push(match own_replace {
                    Ok(value) => value,
                    Err(message) => json!(message),
                });
                push(match foreign_unregister {
                    Ok(()) => json!("ok"),
                    Err(message) => json!(message),
                });
                push(after_foreign);
                push(match own_unregister {
                    Ok(value) => value,
                    Err(message) => json!(message),
                });
                Ok(())
            }),
            CWD,
            bus(),
            &runtime,
            Some("<inline:two>"),
        )
        .unwrap();
    }
    // The JSON payload snapshots of the final registry (name order =
    // registration order).
    let servers = runtime.mcp_servers_list();
    let payloads = runtime.mcp_server_payloads();
    let final_list = super::loader::registered_mcp_servers_value(&servers, &payloads);
    let observed = json!({
        "one": {
            "valid": one_out.lock().unwrap()[0],
            "invalid": one_out.lock().unwrap()[1],
            "afterRegister": [],
        },
        "two": {
            "conflict": two_out.lock().unwrap()[0],
            "ownReplace": two_out.lock().unwrap()[1],
            "foreignUnregister": two_out.lock().unwrap()[2],
            "afterForeignUnregister": two_out.lock().unwrap()[3],
            "ownUnregister": two_out.lock().unwrap()[4],
        },
        "finalList": final_list,
    });
    assert_eq!(observed, expected_surface);
}

#[tokio::test]
async fn mcp_servers_change_with_handler() {
    let expected_change = expected("mcp_servers_change_with_handler");
    let emitted: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime = ExtensionRuntime::new();
    let sink = Arc::clone(&emitted);
    let handler = load_extension_from_factory(
        Arc::new(move |pi| {
            let sink = Arc::clone(&sink);
            pi.on(
                "mcp_servers_change",
                types::sync_handler(move |event, _ctx| {
                    sink.lock().unwrap().push(event.clone());
                    Ok(None)
                }),
            )?;
            pi.register_mcp_server("loaded", &config_map(vec![("command", json!("serve"))]))?;
            Ok(())
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline:handler>"),
    )
    .unwrap();
    let runner = make_runner(vec![handler], runtime.clone(), None);
    load_extension_from_factory(
        Arc::new(move |pi| {
            pi.register_mcp_server(
                "live",
                &config_map(vec![("url", json!("https://mcp.example.com/live"))]),
            )
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline:handler2>"),
    )
    .unwrap();
    // `void this.emit(...)` — a spawned task in the port.
    for _ in 0..100 {
        if !emitted.lock().unwrap().is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        json!({ "emitted": json!(*emitted.lock().unwrap()) }),
        expected_change
    );
    // bind_core above instantiated the sink; silence the unused warning path.
    assert!(runner.has_handlers("mcp_servers_change"));
}

#[test]
fn mcp_servers_unhandled_report() {
    let errors: Arc<Mutex<Vec<types::ExtensionError>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime = ExtensionRuntime::new();
    let runner = make_runner(Vec::new(), runtime.clone(), None);
    {
        let errors = Arc::clone(&errors);
        runner.on_error(Arc::new(move |error| {
            errors.lock().unwrap().push(error.clone())
        }));
    }
    load_extension_from_factory(
        Arc::new(|pi| pi.register_mcp_server("alpha", &config_map(vec![("command", json!("a"))]))),
        CWD,
        bus(),
        &runtime,
        Some("<inline:nohandler-same>"),
    )
    .unwrap();
    load_extension_from_factory(
        Arc::new(|pi| {
            // The same extension replaces its own "alpha"; a different
            // extension's registration would conflict.
            pi.register_mcp_server("alpha", &config_map(vec![("command", json!("a2"))]))
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline:nohandler-same>"),
    )
    .unwrap();
    load_extension_from_factory(
        Arc::new(|pi| {
            pi.register_mcp_server("beta", &config_map(vec![("command", json!("b"))]))?;
            pi.unregister_mcp_server("alpha")
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline:nohandler-same>"),
    )
    .unwrap();
    let observed: Vec<Value> = errors
        .lock()
        .unwrap()
        .iter()
        .map(|error| {
            json!({
                "extensionPath": error.extension_path,
                "event": error.event,
                "error": error.error,
            })
        })
        .collect();
    assert_eq!(
        json!({ "errors": observed }),
        json!({ "errors": expected("mcp_servers_unhandled_report")["errors"] })
    );
}

#[test]
fn get_settings() {
    let expected_settings = expected("get_settings");
    // Pre-bind: the throwing stub message.
    let pre_bind_message: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    {
        let holder = Arc::clone(&pre_bind_message);
        let pre_bind_runtime = ExtensionRuntime::new();
        load_extension_from_factory(
            Arc::new(move |pi| {
                let message = match pi.get_settings() {
                    Ok(_) => "ok".to_string(),
                    Err(error) => error,
                };
                *holder.lock().unwrap() = Some(message);
                Ok(())
            }),
            CWD,
            bus(),
            &pre_bind_runtime,
            Some("<inline:prebind>"),
        )
        .unwrap();
    }
    // Post-bind: the action's value (the factory itself runs pre-bind, so the
    // post-bind read goes through the runtime).
    let settings = json!({ "defaultProvider": "anthropic", "compaction": { "enabled": true } });
    let mut actions = shared_actions();
    let shared = Arc::get_mut(&mut actions).expect("fresh actions");
    shared.get_settings = {
        let settings = settings.clone();
        Arc::new(move || settings.clone())
    };
    let runtime = ExtensionRuntime::new();
    let runner = ExtensionRunner::new(
        Vec::new(),
        runtime,
        CWD,
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    runner.bind_core(actions, context_actions(), None);
    assert_eq!(
        json!({
            "preBind": pre_bind_message.lock().unwrap().clone(),
            "inFactory": pre_bind_message.lock().unwrap().clone(),
            "postBind": runner.runtime().get_settings().unwrap(),
            "runtimeGetSettingsAfterBind": runner.runtime().get_settings().unwrap(),
        }),
        expected_settings
    );
}

#[test]
fn register_command_validation() {
    let expected_validation = expected("register_command_validation");
    let empty_name: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let valid: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    {
        let empty = Arc::clone(&empty_name);
        let valid_out = Arc::clone(&valid);
        let runtime = ExtensionRuntime::new();
        load_extension_from_factory(
            Arc::new(move |pi| {
                *empty.lock().unwrap() = Some(
                    match pi.register_command("", None, Arc::new(|_, _| Ok(None))) {
                        Ok(()) => "ok".to_string(),
                        Err(message) => message,
                    },
                );
                // The "must define handler()" throw is unrepresentable here:
                // the port's CommandHandler is not optional (disclosed).
                *valid_out.lock().unwrap() = Some(
                    match pi.register_command(
                        "ok",
                        Some("d".to_string()),
                        Arc::new(|_, _| Ok(None)),
                    ) {
                        Ok(()) => "ok".to_string(),
                        Err(message) => message,
                    },
                );
                Ok(())
            }),
            CWD,
            bus(),
            &runtime,
            Some("<inline>"),
        )
        .unwrap();
    }
    assert_eq!(
        empty_name.lock().unwrap().clone().unwrap(),
        expected_validation["emptyName"].as_str().unwrap()
    );
    assert_eq!(valid.lock().unwrap().clone().unwrap(), "ok");
}

fn turn_end_base_event() -> Value {
    json!({
        "type": "turn_end",
        "turnIndex": 0,
        "message": { "role": "assistant", "content": "answer", "timestamp": 1 },
        "toolResults": [],
        "messageEntryId": "entry-message",
        "toolResultEntryIds": [],
        "outcome": "completed",
    })
}

fn boundary_preview(label: &str, entries: &[Value]) -> Value {
    json!({
        "contextEntries": entries
            .iter()
            .map(|entry| json!({ "id": entry.get("customType").cloned().unwrap_or(Value::Null) }))
            .collect::<Vec<_>>(),
        "contextMessages": [],
        "llmMessages": [],
        "pendingMessages": [],
        "canContinue": true,
        "label": label,
    })
}

#[tokio::test]
async fn emit_boundary_chain() {
    let expected_chain = expected("emit_boundary_chain");
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime = ExtensionRuntime::new();
    let sink = Arc::clone(&seen);
    let handler_ext = load_extension_from_factory(
        Arc::new(move |pi| {
            let sink_a = Arc::clone(&sink);
            pi.on(
                "turn_end",
                types::sync_handler(move |event, _ctx| {
                    sink_a.lock().unwrap().push(json!({
                        "handler": "A",
                        "entries": event["entries"],
                        "continue": event["continue"],
                        "context": event["context"],
                        "outcome": event["outcome"],
                        "turnIndex": event["turnIndex"],
                        "messageEntryId": event["messageEntryId"],
                        "ctxIsObject": true,
                    }));
                    let mut entries = event["entries"].as_array().unwrap().clone();
                    entries.push(json!({ "type": "custom", "customType": "note", "data": { "from": "A" } }));
                    Ok(Some(types::HandlerResult::Json(json!({
                        "entries": entries,
                        "continue": true,
                    }))))
                }),
            )?;
            let sink_b = Arc::clone(&sink);
            pi.on(
                "turn_end",
                types::sync_handler(move |event, _ctx| {
                    sink_b.lock().unwrap().push(json!({
                        "handler": "B",
                        "entries": event["entries"],
                        "continue": event["continue"],
                        "context": event["context"],
                    }));
                    let mut entries = event["entries"].as_array().unwrap().clone();
                    entries.push(json!({ "type": "custom_message", "customType": "b", "content": "hi", "display": true }));
                    Ok(Some(types::HandlerResult::Json(json!({
                        "entries": entries,
                    }))))
                }),
            )?;
            Ok(())
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline>"),
    )
    .unwrap();
    let runner = make_runner(vec![handler_ext], runtime, None);
    let build_calls = Mutex::new(0u32);
    let result = runner
        .emit_boundary(turn_end_base_event(), &|entries: &[Value]| {
            let mut calls = build_calls.lock().unwrap();
            *calls += 1;
            let label = format!("build-{calls}");
            drop(calls);
            Ok(boundary_preview(&label, entries))
        })
        .await
        .unwrap();
    let observed = json!({
        "seen": json!(*seen.lock().unwrap()),
        "result": {
            "entries": result.entries,
            "continue": result.r#continue,
            "context": result.context,
            "valid": result.valid,
        },
    });
    assert_eq!(observed, expected_chain);
}

#[tokio::test]
async fn emit_boundary_invalid() {
    let expected_invalid = expected("emit_boundary_invalid");
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime = ExtensionRuntime::new();
    let sink = Arc::clone(&order);
    let handler_ext = load_extension_from_factory(
        Arc::new(move |pi| {
            let sink_a = Arc::clone(&sink);
            pi.on(
                "turn_end",
                types::sync_handler(move |event, _ctx| {
                    sink_a.lock().unwrap().push("A".to_string());
                    let mut entries = event["entries"].as_array().unwrap().clone();
                    entries.push(json!({ "type": "custom", "customType": "ok" }));
                    Ok(Some(types::HandlerResult::Json(
                        json!({ "entries": entries }),
                    )))
                }),
            )?;
            let sink_b = Arc::clone(&sink);
            pi.on(
                "turn_end",
                types::sync_handler(move |event, _ctx| {
                    sink_b.lock().unwrap().push("B".to_string());
                    let mut entries = event["entries"].as_array().unwrap().clone();
                    entries.push(json!({ "type": "bogus" }));
                    Ok(Some(types::HandlerResult::Json(
                        json!({ "entries": entries }),
                    )))
                }),
            )?;
            let sink_c = Arc::clone(&sink);
            pi.on(
                "turn_end",
                types::sync_handler(move |_event, _ctx| {
                    sink_c.lock().unwrap().push("C".to_string());
                    Ok(Some(types::HandlerResult::Json(
                        json!({ "continue": true }),
                    )))
                }),
            )?;
            Ok(())
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline>"),
    )
    .unwrap();
    let runner = make_runner(vec![handler_ext], runtime, None);
    let build_calls = Mutex::new(0u32);
    let result = runner
        .emit_boundary(turn_end_base_event(), &|entries: &[Value]| {
            let mut calls = build_calls.lock().unwrap();
            *calls += 1;
            let label = format!("build2-{calls}");
            drop(calls);
            if entries.iter().any(|entry| entry["type"] == "bogus") {
                return Err("unsupported draft type".to_string());
            }
            Ok(boundary_preview(&label, entries))
        })
        .await
        .unwrap();
    let observed = json!({
        "order": json!(*order.lock().unwrap()),
        "result": {
            "entries": result.entries,
            "continue": result.r#continue,
            "context": result.context,
            "valid": result.valid,
        },
    });
    assert_eq!(observed, expected_invalid);
}

#[tokio::test]
async fn emit_boundary_agent_before_settle() {
    let expected_settle = expected("emit_boundary_agent_before_settle");
    let order: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime = ExtensionRuntime::new();
    let sink = Arc::clone(&order);
    let handler_ext = load_extension_from_factory(
        Arc::new(move |pi| {
            let sink_settle = Arc::clone(&sink);
            pi.on(
                "agent_before_settle",
                types::sync_handler(move |event, _ctx| {
                    sink_settle
                        .lock()
                        .unwrap()
                        .push(json!({ "outcome": event["outcome"], "type": event["type"] }));
                    Ok(Some(types::HandlerResult::Json(
                        json!({ "continue": true }),
                    )))
                }),
            )?;
            Ok(())
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline>"),
    )
    .unwrap();
    let runner = make_runner(vec![handler_ext], runtime, None);
    let result = runner
        .emit_boundary(
            json!({
                "type": "agent_before_settle",
                "turnIndex": 0,
                "message": { "role": "assistant", "content": "answer", "timestamp": 1 },
                "toolResults": [],
                "messageEntryId": "entry-message",
                "toolResultEntryIds": [],
                "outcome": "completed",
            }),
            &|entries: &[Value]| Ok(boundary_preview("settle", entries)),
        )
        .await
        .unwrap();
    let observed = json!({
        "order": json!(*order.lock().unwrap()),
        "result": {
            "entries": result.entries,
            "continue": result.r#continue,
            "context": result.context,
            "valid": result.valid,
        },
    });
    assert_eq!(observed, expected_settle);
}

#[tokio::test]
async fn emit_cache_warming_decision() {
    let expected_warming = expected("emit_cache_warming_decision");
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime = ExtensionRuntime::new();
    let sink = Arc::clone(&seen);
    let handler_ext = load_extension_from_factory(
        Arc::new(move |pi| {
            let sink_warm = Arc::clone(&sink);
            pi.on(
                "cache_warming_decision",
                types::sync_handler(move |event, _ctx| {
                    sink_warm.lock().unwrap().push(event.clone());
                    Ok(Some(types::HandlerResult::Json(
                        json!({ "action": "stop" }),
                    )))
                }),
            )?;
            pi.on(
                "cache_warming_decision",
                types::sync_handler(|_event, _ctx| Err("classifier down".to_string())),
            )?;
            pi.on(
                "cache_warming_decision",
                types::sync_handler(|_event, _ctx| {
                    Ok(Some(types::HandlerResult::Json(
                        json!({ "action": "warm" }),
                    )))
                }),
            )?;
            Ok(())
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline>"),
    )
    .unwrap();
    let runner = make_runner(vec![handler_ext], runtime, None);
    let errors: Arc<Mutex<Vec<types::ExtensionError>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let errors = Arc::clone(&errors);
        runner.on_error(Arc::new(move |error| {
            errors.lock().unwrap().push(error.clone())
        }));
    }
    let action = runner
        .emit_cache_warming_decision(&json!({
            "type": "cache_warming_decision",
            "warmCost": 0.001,
            "missCost": 0.2,
            "continuationProbability": 0.5,
            "action": "warm",
        }))
        .await;
    let observed_errors: Vec<Value> = errors
        .lock()
        .unwrap()
        .iter()
        .map(|error| {
            json!({
                "extensionPath": error.extension_path,
                "event": error.event,
                "error": error.error,
            })
        })
        .collect();
    // Upstream attaches a JS stack; the port's stack stays None (disclosed),
    // so the oracle's presence marker is dropped before comparing.
    let mut expected_without_stack = expected_warming;
    if let Some(entries) = expected_without_stack["errors"].as_array_mut() {
        for entry in entries {
            entry.as_object_mut().unwrap().remove("stack");
        }
    }
    assert_eq!(
        json!({ "seen": json!(*seen.lock().unwrap()), "action": action, "errors": observed_errors }),
        expected_without_stack
    );
}

fn transcript_json() -> Vec<Value> {
    vec![
        json!({ "role": "system", "content": "BASE", "timestamp": 5, "sections": { "s1": "v1" }, "toolsAdded": [{ "name": "read" }] }),
        json!({ "role": "user", "content": "hello", "timestamp": 6 }),
        json!({ "role": "system", "content": "extra", "timestamp": 7 }),
        json!({ "role": "assistant", "content": "hi", "timestamp": 8 }),
    ]
}

#[tokio::test]
async fn emit_context_two_phase() {
    let expected_context = expected("emit_context_two_phase");
    let seen: Arc<Mutex<(Vec<Value>, Vec<Value>)>> = Arc::new(Mutex::new((Vec::new(), Vec::new())));
    let runtime = ExtensionRuntime::new();
    let sink = Arc::clone(&seen);
    let handler_ext = load_extension_from_factory(
        Arc::new(move |pi| {
            let sink_ctx_a = Arc::clone(&sink);
            pi.on(
                "context",
                types::sync_handler(move |event, _ctx| {
                    let mut state = sink_ctx_a.lock().unwrap();
                    state.0.push(event["messages"].clone());
                    // Replace an element in place (upstream identity
                    // semantics; the port treats the value change as a
                    // replacement and reattaches the folded system head).
                    let mut messages = event["messages"].as_array().unwrap().clone();
                    if let Some(first) = messages.first_mut() {
                        first["content"] = json!("hello!");
                    }
                    event["messages"] = Value::Array(messages);
                    Ok(None)
                }),
            )?;
            let sink_ctx_b = Arc::clone(&sink);
            pi.on(
                "context",
                types::sync_handler(move |event, _ctx| {
                    let mut state = sink_ctx_b.lock().unwrap();
                    state.0.push(event["messages"].clone());
                    let kept: Vec<Value> = event["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|message| message["role"] != "assistant")
                        .cloned()
                        .collect();
                    Ok(Some(types::HandlerResult::Json(
                        json!({ "messages": kept }),
                    )))
                }),
            )?;
            let sink_cws = Arc::clone(&sink);
            pi.on(
                "context_with_system",
                types::sync_handler(move |event, _ctx| {
                    let mut state = sink_cws.lock().unwrap();
                    state.1.push(event["messages"].clone());
                    let kept: Vec<Value> = event["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|message| message["role"] != "system")
                        .cloned()
                        .collect();
                    Ok(Some(types::HandlerResult::Json(
                        json!({ "messages": kept }),
                    )))
                }),
            )?;
            Ok(())
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline>"),
    )
    .unwrap();
    let runner = make_runner(vec![handler_ext], runtime, None);
    let errors: Arc<Mutex<Vec<types::ExtensionError>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let errors = Arc::clone(&errors);
        runner.on_error(Arc::new(move |error| {
            errors.lock().unwrap().push(error.clone())
        }));
    }
    let result = runner.emit_context(&transcript_json()).await;
    let (context_seen, with_system_seen) = {
        let state = seen.lock().unwrap();
        (state.0.clone(), state.1.clone())
    };
    let observed_errors: Vec<Value> = errors
        .lock()
        .unwrap()
        .iter()
        .map(|error| {
            json!({
                "extensionPath": error.extension_path,
                "event": error.event,
                "error": error.error,
            })
        })
        .collect();
    assert_eq!(
        json!({
            "seen": { "context": context_seen, "contextWithSystem": with_system_seen },
            "result": result,
            "errors": observed_errors,
        }),
        expected_context
    );
}

#[tokio::test]
async fn emit_context_unchanged() {
    let expected_unchanged = expected("emit_context_unchanged");
    let context_handlers = Arc::new(Mutex::new(0u32));
    let with_system: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime = ExtensionRuntime::new();
    let handler_count = Arc::clone(&context_handlers);
    let sink = Arc::clone(&with_system);
    let handler_ext = load_extension_from_factory(
        Arc::new(move |pi| {
            let handler_count = Arc::clone(&handler_count);
            pi.on(
                "context",
                types::sync_handler(move |_event, _ctx| {
                    *handler_count.lock().unwrap() += 1;
                    Ok(None)
                }),
            )?;
            let sink_cws2 = Arc::clone(&sink);
            pi.on(
                "context_with_system",
                types::sync_handler(move |event, _ctx| {
                    sink_cws2.lock().unwrap().push(event["messages"].clone());
                    Ok(None)
                }),
            )?;
            Ok(())
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline>"),
    )
    .unwrap();
    let runner = make_runner(vec![handler_ext], runtime, None);
    let result = runner
        .emit_context(&[
            json!({ "role": "system", "content": "only", "timestamp": 1 }),
            json!({ "role": "user", "content": "u", "timestamp": 2 }),
        ])
        .await;
    assert_eq!(
        json!({
            "phases": {
                "contextHandlers": *context_handlers.lock().unwrap(),
                "contextWithSystem": json!(*with_system.lock().unwrap()),
            },
            "result": result,
        }),
        expected_unchanged
    );
}

#[tokio::test]
async fn emit_context_no_system() {
    let expected_no_system = expected("emit_context_no_system");
    let runtime = ExtensionRuntime::new();
    let handler_ext = load_extension_from_factory(
        Arc::new(|pi| {
            pi.on(
                "context",
                types::sync_handler(|event, _ctx| {
                    let mut messages = event["messages"].as_array().unwrap().clone();
                    messages.reverse();
                    Ok(Some(types::HandlerResult::Json(
                        json!({ "messages": messages }),
                    )))
                }),
            )?;
            Ok(())
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline>"),
    )
    .unwrap();
    let runner = make_runner(vec![handler_ext], runtime, None);
    let result = runner
        .emit_context(&[
            json!({ "role": "user", "content": "a", "timestamp": 1 }),
            json!({ "role": "assistant", "content": "b", "timestamp": 2 }),
        ])
        .await;
    assert_eq!(json!(result), expected_no_system["result"]);
}

#[tokio::test]
async fn create_tool_context() {
    let expected_context = expected("create_tool_context");
    let forward_calls: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime = ExtensionRuntime::new();
    let runner = ExtensionRunner::new(
        Vec::new(),
        runtime,
        CWD,
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    let calls_for_bind = Arc::clone(&forward_calls);
    let marker_signal = Arc::new(types::AbortSignal::new());
    let marker_for_closure = Arc::clone(&marker_signal);
    let mut context_actions_value = (*context_actions()).clone();
    context_actions_value.get_callable_tools = Some(Arc::new(|| {
        vec![
            placeholder_tool("callable_one"),
            placeholder_tool("callable_two"),
        ]
    }));
    context_actions_value.execute_tool = Some(Arc::new(
        move |caller_id: String, name: String, args: Value, options: ExecuteToolOptions| {
            let marker = Arc::clone(&marker_for_closure);
            let calls_for_bind = Arc::clone(&calls_for_bind);
            Box::pin(async move {
                calls_for_bind.lock().unwrap().push(json!({
                    "callerId": caller_id,
                    "name": name,
                    "args": args,
                    "hasSignal": options.signal.is_some(),
                    "sameSignal": options
                        .signal
                        .as_ref()
                        .map(|signal| Arc::ptr_eq(signal, &marker))
                        .unwrap_or(false),
                }));
                Ok(json!({
                    "toolCall": { "type": "toolCall", "id": format!("{caller_id}/0"), "name": name, "arguments": args },
                    "result": { "content": [{ "type": "text", "text": "nested ok" }], "details": {} },
                    "isError": false,
                }))
            })
        },
    ));
    runner.bind_core(shared_actions(), Arc::new(context_actions_value), None);
    let ctx = runner.create_tool_context("parent1", Some(Arc::clone(&marker_signal)));
    let nested = ctx
        .execute_tool(
            "mcp__docs__search",
            &json!({ "q": "rust" }),
            ExecuteToolOptions::default(),
        )
        .await
        .unwrap();
    ctx.execute_tool("other", &json!({}), ExecuteToolOptions::default())
        .await
        .unwrap();
    let tools: Vec<Value> = ctx
        .callable_tools()
        .iter()
        .map(|tool| json!({ "name": tool.name }))
        .collect();
    let observed = json!({
        "forwardCalls": json!(*forward_calls.lock().unwrap()),
        "nested": nested,
        "tools": tools,
        "toolsLiveGetter": ctx.callable_tools().len(),
    });
    assert_eq!(observed, expected_context);
    // The default nested-call signal is the calling tool's signal, verbatim.
    assert!(!marker_signal.is_aborted());
}

#[tokio::test]
async fn create_tool_context_fallback() {
    let expected_fallback = expected("create_tool_context_fallback");
    let runtime = ExtensionRuntime::new();
    let runner = make_runner(Vec::new(), runtime, None);
    let ctx = runner.create_tool_context("parent2", None);
    let result = ctx
        .execute_tool("target", &json!({ "a": 1 }), ExecuteToolOptions::default())
        .await
        .unwrap();
    let tools: Vec<Value> = ctx
        .callable_tools()
        .iter()
        .map(|tool| json!({ "name": tool.name }))
        .collect();
    assert_eq!(
        json!({ "result": result, "tools": tools }),
        expected_fallback
    );
}

fn virtual_model_route() -> types::ExtensionVirtualModelRouteFn {
    Arc::new(|request: Value, _ctx: types::ExtensionContext| {
        Box::pin(async move {
            Ok(json!({
                "model": request["model"],
                "thinkingLevel": request["thinkingLevel"],
                "ctxOk": true,
            }))
        })
    })
}

/// Clone a virtual-model definition JSON and pin the route function's
/// presence (functions serialize as `"<fn>"` in the oracle).
fn with_route_marker(definition: &Value) -> Value {
    let mut object = definition.as_object().cloned().unwrap_or_default();
    object.insert("route".to_string(), json!("<fn>"));
    Value::Object(object)
}

#[test]
fn virtual_models_flush() {
    let expected_flush = expected("virtual_models_flush");
    let runtime = ExtensionRuntime::new();
    {
        let runtime = runtime.clone();
        load_extension_from_factory(
            Arc::new(move |pi| {
                pi.register_virtual_model(
                    json!({
                        "provider": "llama.cpp",
                        "id": "auto",
                        "name": "Auto",
                        "thinkingLevels": ["off", "medium"],
                        "contextWindow": 8192,
                    }),
                    virtual_model_route(),
                )?;
                pi.register_virtual_model(
                    json!({ "provider": "openai", "id": "router", "name": "Router" }),
                    Arc::new(|_request, _ctx| Box::pin(async { Ok(json!({})) })),
                )?;
                pi.unregister_virtual_model("openai", "router")?;
                pi.unregister_virtual_model("openai", "unknown")
            }),
            CWD,
            bus(),
            &runtime,
            Some("<inline:vm>"),
        )
        .unwrap();
    }
    let pending: Vec<Value> = runtime
        .pending_virtual_model_registrations()
        .iter()
        .map(|registration| {
            json!({
                "definition": with_route_marker(&registration.definition.definition),
                "extensionPath": registration.definition.extension_path,
            })
        })
        .collect();
    // Pre-bind wrapped route: rejects with the createContext throw.
    let pre_bind_route = runtime.pending_virtual_model_registrations()[0]
        .definition
        .route
        .clone()(json!({ "model": { "id": "x" } }));
    let pre_bind_message = futures::executor::block_on(pre_bind_route).unwrap_err();

    let registered: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let unregistered: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let routed_contexts: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let registered_sink = Arc::clone(&registered);
        let unregistered_sink = Arc::clone(&unregistered);
        let routed_sink = Arc::clone(&routed_contexts);
        let provider_actions = super::runner::ProviderActions {
            register_virtual_model: Some(Arc::new(move |handle| {
                registered_sink
                    .lock()
                    .unwrap()
                    .push(with_route_marker(&handle.definition));
                let route = Arc::clone(&handle.route);
                let routed = route(
                    json!({ "model": { "id": "physical" }, "thinkingLevel": "off", "reason": "direct" }),
                );
                match futures::executor::block_on(routed) {
                    Ok(result) => routed_sink
                        .lock()
                        .unwrap()
                        .push(json!({ "result": result })),
                    Err(message) => routed_sink
                        .lock()
                        .unwrap()
                        .push(json!({ "threw": message })),
                }
                Ok(())
            })),
            unregister_virtual_model: Some(Arc::new(move |provider, id| {
                unregistered_sink
                    .lock()
                    .unwrap()
                    .push(json!([provider, id]));
                Ok(())
            })),
            ..Default::default()
        };
        make_runner(Vec::new(), runtime, Some(provider_actions));
    }
    assert_eq!(
        json!({
            "pending": pending,
            "preBindRoute": pre_bind_message,
            "registered": json!(*registered.lock().unwrap()),
            "unregistered": json!(*unregistered.lock().unwrap()),
            "routedContexts": json!(*routed_contexts.lock().unwrap()),
        }),
        expected_flush
    );
}

#[test]
fn virtual_models_flush_error() {
    let expected_error = expected("virtual_models_flush_error");
    let runtime = ExtensionRuntime::new();
    load_extension_from_factory(
        Arc::new(|pi| {
            pi.register_virtual_model(
                json!({ "provider": "p", "id": "bad", "name": "Bad" }),
                Arc::new(|_request, _ctx| Box::pin(async { Ok(json!({})) })),
            )
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline:vmbad>"),
    )
    .unwrap();
    let runner = ExtensionRunner::new(
        Vec::new(),
        runtime,
        CWD,
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    let errors: Arc<Mutex<Vec<types::ExtensionError>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = Arc::clone(&errors);
        runner.on_error(Arc::new(move |error| {
            sink.lock().unwrap().push(error.clone())
        }));
    }
    runner.bind_core(
        shared_actions(),
        context_actions(),
        Some(super::runner::ProviderActions {
            register_virtual_model: Some(Arc::new(|_handle| {
                Err("duplicate virtual model".to_string())
            })),
            ..Default::default()
        }),
    );
    let observed: Vec<Value> = errors
        .lock()
        .unwrap()
        .iter()
        .map(|error| {
            json!({
                "extensionPath": error.extension_path,
                "event": error.event,
                "error": error.error,
                "stack": (),
            })
        })
        .collect();
    let mut expected_observed = expected_error["errors"].clone();
    for entry in expected_observed.as_array_mut().unwrap() {
        entry["stack"] = json!(());
    }
    assert_eq!(
        json!({ "errors": observed }),
        json!({ "errors": expected_observed })
    );
}

#[test]
fn virtual_models_registry_fallback() {
    let expected_fallback = expected("virtual_models_registry_fallback");
    let registry_calls: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    struct CapturingRegistry(Arc<Mutex<Vec<Value>>>);
    impl types::ProviderRegistryHandle for CapturingRegistry {
        fn register_virtual_model(&self, definition: &Value) -> Result<(), String> {
            self.0.lock().unwrap().push(with_route_marker(definition));
            Ok(())
        }
        fn unregister_virtual_model(&self, provider: &str, id: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push(json!(["unregister", provider, id]));
            Ok(())
        }
    }
    let runtime = ExtensionRuntime::new();
    load_extension_from_factory(
        Arc::new(|pi| {
            pi.register_virtual_model(
                json!({ "provider": "p", "id": "m", "name": "M" }),
                Arc::new(|_request, _ctx| Box::pin(async { Ok(json!({})) })),
            )
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline:vmreg>"),
    )
    .unwrap();
    {
        let sink = Arc::clone(&registry_calls);
        let runner = ExtensionRunner::new(
            Vec::new(),
            runtime.clone(),
            CWD,
            Arc::new(()),
            Arc::new(CapturingRegistry(sink)),
        );
        runner.bind_core(shared_actions(), context_actions(), None);
    }
    load_extension_from_factory(
        Arc::new(|pi| {
            pi.register_virtual_model(
                json!({ "provider": "q", "id": "live", "name": "Live" }),
                Arc::new(|_request, _ctx| Box::pin(async { Ok(json!({})) })),
            )?;
            pi.unregister_virtual_model("q", "live")
        }),
        CWD,
        bus(),
        &runtime,
        Some("<inline:vmlive>"),
    )
    .unwrap();
    assert_eq!(
        json!(registry_calls.lock().unwrap().clone()),
        expected_fallback["registryCalls"]
    );
}

#[test]
fn synthetic_source_info() {
    let expected_info = expected("synthetic_source_info");
    let runtime = ExtensionRuntime::new();
    let builtin = load_extension_from_factory(
        Arc::new(|_pi| Ok(())),
        CWD,
        bus(),
        &runtime,
        Some("builtin:tool-search"),
    )
    .unwrap();
    let inline = load_extension_from_factory(
        Arc::new(|_pi| Ok(())),
        CWD,
        bus(),
        &runtime,
        Some("<inline:named>"),
    )
    .unwrap();
    // Windows separators on the capture host; the port replays the same
    // relative shape with the host separator (the normalize step below maps
    // the capture's `<root>\file-ext.ts` onto either).
    let file_path = format!("{CWD}{}file-ext.ts", std::path::MAIN_SEPARATOR_STR);
    let local = load_extension_from_factory(
        Arc::new(|_pi| Ok(())),
        CWD,
        bus(),
        &runtime,
        Some(&file_path),
    )
    .unwrap();
    let result = load_extensions(&[], CWD, Some(bus()), Some(runtime), &NullModuleLoader);
    // The capture's loader result had no warnings (key omitted by
    // JSON.stringify on undefined); warnings are covered by dedicated tests.
    let observed = json!({
        "builtin": serde_json::to_value(&builtin.source_info).unwrap(),
        "inline": serde_json::to_value(&inline.source_info).unwrap(),
        "local": serde_json::to_value(&local.source_info).unwrap(),
    });
    assert!(result.warnings.is_empty());
    // The capture ran on a Windows host: "<root>" holds the tmp root with
    // native separators. The port replay uses "/workspace".
    let mut expected_observed = expected_info;
    let normalize = |value: &mut Value| {
        fn walk(value: &mut Value) {
            match value {
                Value::String(text) => {
                    *text = text
                        .replace("<root>\\", CWD)
                        .replace("<root>", CWD)
                        .replace('\\', "/");
                }
                Value::Array(items) => items.iter_mut().for_each(walk),
                Value::Object(entries) => entries.values_mut().for_each(walk),
                _ => {}
            }
        }
        walk(value);
    };
    normalize(&mut expected_observed);
    let mut observed_normalized = observed;
    normalize(&mut observed_normalized);
    assert_eq!(observed_normalized, expected_observed);
}
