//! Tests for the codemode extension (upstream `execute.ts` / `tool.ts`
//! seams): the nested-call pipeline through a stub `ctx.executeTool`, the
//! store-entry persistence, and the loadout presentation. The byte-pinned
//! description surface lives in `src/codemode/codemode_oracle_tests.rs`
//! (Part B of the oracle).

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::execute::{execute_codemode, read_codemode_store};
use super::tool::{
    create_codemode_tool_definition, prepare_codemode_loadout, CodemodeStoreEntryData,
    CodemodeToolOptions, CODEMODE_STORE_ENTRY_TYPE, CODEMODE_TOOL_NAME,
};
use crate::agent_core::types::AgentTool;
use crate::coding_agent::extensions::runner::ExtensionRunner;
use crate::coding_agent::extensions::types::{
    AbortSignal, ExtensionContextActions, NoopProviderRegistry, OrderedMap, ToolExposure,
    ToolLoadout, ToolNamespace,
};
use crate::coding_agent::extensions::ExtensionRuntime;

const CWD: &str = "/workspace";

fn placeholder_tool(name: &str, description: &str) -> AgentTool {
    AgentTool {
        name: name.to_string(),
        label: name.to_string(),
        description: description.to_string(),
        parameters: json!({
            "type": "object",
            "properties": { "value": { "type": "number" } },
            "required": ["value"]
        }),
        constrained_sampling: None,
        execute: Arc::new(|_, _, _, _| Box::pin(async { unreachable!() })),
        prepare_arguments: None,
        replay: None,
        execution_mode: None,
    }
}

fn shared_actions() -> Arc<crate::coding_agent::extensions::types::ExtensionActions> {
    Arc::new(crate::coding_agent::extensions::types::ExtensionActions {
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
        set_model: Arc::new(|_| {
            Ok(crate::coding_agent::extensions::types::CommandFuture::resolved(false))
        }),
        get_thinking_level: Arc::new(|| crate::coding_agent::extensions::types::ThinkingLevel::Off),
        set_thinking_level: Arc::new(|_| {}),
    })
}

fn context_actions() -> ExtensionContextActions {
    ExtensionContextActions {
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
    }
}

/// A runner whose tool context exposes `tools` and a stub `executeTool` that
/// echoes the arguments back as the nested result text.
fn runner_with_echo_tool(recorded: Arc<Mutex<Vec<Value>>>) -> ExtensionRunner {
    let runner = ExtensionRunner::new(
        Vec::new(),
        ExtensionRuntime::new(),
        CWD,
        Arc::new(()),
        Arc::new(NoopProviderRegistry),
    );
    let mut context_actions = context_actions();
    context_actions.get_callable_tools = Some(Arc::new(|| {
        vec![placeholder_tool("echo", "Returns the arguments unchanged.")]
    }));
    context_actions.execute_tool = Some(Arc::new(
        move |caller_id: String, name: String, args: Value, _options| {
            let recorded = Arc::clone(&recorded);
            Box::pin(async move {
                recorded.lock().unwrap().push(json!({
                    "callerId": caller_id,
                    "name": name,
                    "args": args,
                }));
                Ok(json!({
                    "toolCall": {
                        "type": "toolCall",
                        "id": format!("{caller_id}/0"),
                        "name": name,
                        "arguments": args,
                    },
                    "result": {
                        "content": [{ "type": "text", "text": args.to_string() }],
                        "details": {},
                    },
                    "isError": false,
                }))
            })
        },
    ));
    runner.bind_core(shared_actions(), Arc::new(context_actions), None);
    runner
}

#[test]
fn read_store_applies_entries_in_branch_order() {
    use crate::coding_agent::session_manager::{CustomEntry, SessionEntry};
    let entry = |set: Value, delete: Vec<&str>| {
        SessionEntry::Custom(CustomEntry {
            custom_type: CODEMODE_STORE_ENTRY_TYPE.to_string(),
            data: Some(json!({
                "set": set,
                "delete": delete,
            })),
            id: "e".to_string(),
            parent_id: None,
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
        })
    };
    let branch = vec![
        entry(json!({ "a": 1, "b": "two" }), vec![]),
        entry(json!({ "a": 2 }), vec![]),
        entry(json!({}), vec!["b"]),
        // Other custom types and malformed entries are skipped.
        SessionEntry::Custom(CustomEntry {
            custom_type: "other".to_string(),
            data: Some(json!({ "set": { "a": 9 }, "delete": [] })),
            id: "e2".to_string(),
            parent_id: None,
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
        }),
        SessionEntry::Custom(CustomEntry {
            custom_type: CODEMODE_STORE_ENTRY_TYPE.to_string(),
            data: Some(json!({ "set": "not-an-object", "delete": [] })),
            id: "e3".to_string(),
            parent_id: None,
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
        }),
    ];
    assert_eq!(
        read_codemode_store(&branch),
        [(String::from("a"), json!(2))].into_iter().collect()
    );
}

#[tokio::test]
async fn execute_runs_script_with_nested_tool_call() {
    let recorded: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let runner = runner_with_echo_tool(Arc::clone(&recorded));
    let ctx = runner.create_tool_context("call-1", None);
    let result = execute_codemode(
        "call-1",
        &json!({ "code": "const r = await tools.echo({ value: 7 });\nreturn r;" }),
        None,
        None,
        &ctx,
        CodemodeToolOptions::default(),
    )
    .await
    .expect("script executes");

    // One nested call through the pipeline, with the call's id.
    let calls = recorded.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["callerId"], json!("call-1"));
    assert_eq!(calls[0]["name"], json!("echo"));
    drop(calls);

    let content = result["content"].as_array().unwrap();
    let header = content[0]["text"].as_str().unwrap();
    assert!(
        header.starts_with("Script completed\nWall time "),
        "{header}"
    );
    assert!(header.ends_with(" seconds\nOutput:\n"), "{header}");
    // The returned value is appended like text() (a string, as-is).
    assert_eq!(
        content[1]["text"],
        json!(r#"{"value":7}"#),
        "the echoed text resolves the nested promise"
    );
    let details = &result["details"];
    let calls = details["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["id"], json!("call-1/0"));
    assert_eq!(calls[0]["name"], json!("echo"));
    assert_eq!(calls[0]["status"], json!("ok"));
    assert_eq!(calls[0]["args"], json!(r#"{"value":7}"#));
    assert!(result.get("isError").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn execute_reports_script_failure_with_partial_output() {
    let recorded: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let runner = runner_with_echo_tool(recorded);
    let ctx = runner.create_tool_context("call-2", None);
    let result = execute_codemode(
        "call-2",
        &json!({ "code": "text(\"before\");\nawait tools.echo({ value: 1 });\nthrow new Error('boom');" }),
        None,
        None,
        &ctx,
        CodemodeToolOptions::default(),
    )
    .await
    .expect("script executes");

    assert_eq!(result["isError"], json!(true));
    let content = result["content"].as_array().unwrap();
    assert!(content[0]["text"]
        .as_str()
        .unwrap()
        .starts_with("Script failed\nWall time "));
    // The partial output precedes the error block.
    assert_eq!(content[1]["text"], json!("before"));
    let error_text = content[2]["text"].as_str().unwrap();
    assert!(
        error_text.starts_with("Script error:\nError: boom\n    at "),
        "{error_text}"
    );
    // The calls summary covers every nested call made before the failure.
    assert!(
        error_text.contains("Tool calls made before the failure (they are not undone): echo (ok)"),
        "{error_text}"
    );
}

#[tokio::test]
async fn execute_persists_store_writes_via_append_entry() {
    let recorded: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let runner = runner_with_echo_tool(recorded);
    let ctx = runner.create_tool_context("call-3", None);
    let entries: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&entries);
    let options = CodemodeToolOptions {
        append_entry: Some(Arc::new(
            move |custom_type: &str, data: &CodemodeStoreEntryData| {
                sink.lock().unwrap().push((
                    custom_type.to_string(),
                    json!({ "set": data.set, "delete": data.delete }),
                ));
            },
        )),
        ..Default::default()
    };
    let result = execute_codemode(
        "call-3",
        &json!({ "code": "store('k', { n: 1 });\nstore('gone', undefined);\nreturn 'done';" }),
        None,
        None,
        &ctx,
        options,
    )
    .await
    .expect("script executes");
    let written = entries.lock().unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].0, CODEMODE_STORE_ENTRY_TYPE);
    assert_eq!(
        written[0].1,
        json!({ "set": { "k": { "n": 1 } }, "delete": ["gone"] })
    );
    // `return 'done'` is appended like text().
    let content = result["content"].as_array().unwrap();
    assert_eq!(content[1]["text"], json!("done"));
}

#[test]
fn prepare_loadout_modes_and_descriptions() {
    let options = CodemodeToolOptions::default();
    let echo = placeholder_tool("echo", "Returns the arguments unchanged.");
    let mut exposures = OrderedMap::new();
    exposures.set("echo", ToolExposure::Direct);
    exposures.set(CODEMODE_TOOL_NAME, ToolExposure::ModelOnly);
    let namespaces = OrderedMap::new();
    let loadout = ToolLoadout::new(
        vec![
            echo.clone(),
            placeholder_tool(CODEMODE_TOOL_NAME, "placeholder"),
        ],
        vec![
            echo.clone(),
            placeholder_tool(CODEMODE_TOOL_NAME, "placeholder"),
        ],
        vec![
            echo.clone(),
            placeholder_tool(CODEMODE_TOOL_NAME, "placeholder"),
        ],
        exposures,
        namespaces,
    );

    let changes = prepare_codemode_loadout(&loadout, &options)
        .descriptions
        .expect("descriptions");
    // Mode `on`: the declared callable tool gets its sample appended, and the
    // codemode description replaces its own.
    assert!(
        changes["echo"].contains("### codemode tool declaration")
            || changes["echo"].contains("```ts")
    );
    let codemode_description = changes[CODEMODE_TOOL_NAME].clone();
    assert!(
        codemode_description.starts_with("Run JavaScript code to orchestrate/compose tool calls")
    );
    // Mode `on` lists only non-direct callable tools: `echo` is direct, so
    // the description has no nested-tools section at all (upstream returns
    // before the tool listing when nothing is listed).
    assert!(!codemode_description.contains("Nested tools:"));
    assert!(!codemode_description.contains("### `echo`"));
}

#[test]
fn prepare_loadout_mode_only_hides_direct_declarations() {
    let options = CodemodeToolOptions {
        get_mode: Some(Arc::new(|| super::tool::CodemodeMode::Only)),
        ..Default::default()
    };
    let echo = placeholder_tool("echo", "Returns the arguments unchanged.");
    let mut exposures = OrderedMap::new();
    exposures.set("echo", ToolExposure::Direct);
    let namespaces = OrderedMap::new();
    let loadout = ToolLoadout::new(
        vec![echo.clone()],
        vec![echo.clone()],
        vec![echo],
        exposures,
        namespaces,
    );
    let changes = prepare_codemode_loadout(&loadout, &options);
    // Mode `only`: every callable tool is listed, and direct tools'
    // declarations are hidden from requests.
    let description = &changes.descriptions.expect("descriptions")[CODEMODE_TOOL_NAME];
    assert!(description.contains("### `echo`"));
    assert_eq!(
        changes.hidden_declarations.expect("hidden").as_slice(),
        ["echo"]
    );
}

#[test]
fn tool_definition_shape() {
    let definition = create_codemode_tool_definition(CodemodeToolOptions::default());
    assert_eq!(definition.name, CODEMODE_TOOL_NAME);
    assert_eq!(definition.label, CODEMODE_TOOL_NAME);
    assert_eq!(definition.exposure, ToolExposure::ModelOnly);
    assert_eq!(
        definition.parameters,
        json!({
            "type": "object",
            "required": ["code"],
            "properties": {
                "code": {
                    "type": "string",
                    "description": "Raw JavaScript source. Top-level await and return work. May start with a `// @options: {\"max_output_tokens\": 1000}` line."
                }
            }
        })
    );
    assert_eq!(
        definition.prompt_snippet.as_deref(),
        Some(
            "Run JavaScript that calls other tools (chains, loops, Promise.all, filtering large results)"
        )
    );
    assert_eq!(
        definition.constrained_sampling,
        Some(json!({
            "type": "grammar",
            "variants": { "openai_lark": crate::codemode::CODEMODE_SOURCE_GRAMMAR }
        }))
    );
    // The placeholder description is replaced by prepareLoadout once active.
    assert_eq!(definition.description, super::tool::DESCRIPTION_INTRO);
}

#[tokio::test]
async fn execute_nested_error_is_recorded_and_rejected() {
    let recorded: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let runner = ExtensionRunner::new(
        Vec::new(),
        ExtensionRuntime::new(),
        CWD,
        Arc::new(()),
        Arc::new(NoopProviderRegistry),
    );
    let mut context_actions = context_actions();
    context_actions.get_callable_tools = Some(Arc::new(|| {
        vec![placeholder_tool("failing", "Always fails.")]
    }));
    let calls_for_stub = Arc::clone(&recorded);
    context_actions.execute_tool = Some(Arc::new(
        move |_caller_id: String, name: String, args: Value, _options| {
            let recorded = Arc::clone(&calls_for_stub);
            Box::pin(async move {
                recorded.lock().unwrap().push(json!({ "name": name }));
                Ok(json!({
                    "toolCall": { "type": "toolCall", "id": "nested-1", "name": name, "arguments": args },
                    "result": { "content": [{ "type": "text", "text": "tool exploded" }], "details": {} },
                    "isError": true,
                }))
            })
        },
    ));
    runner.bind_core(shared_actions(), Arc::new(context_actions), None);
    let ctx = runner.create_tool_context("call-4", Some(Arc::new(AbortSignal::new())));

    // The failed nested call rejects into the script; the caught message is
    // the tool's error text.
    let result = execute_codemode(
        "call-4",
        &json!({
            "code": "try { await tools.failing({ value: 1 }); return 'unreachable'; } catch (error) { return error.message; }"
        }),
        None,
        None,
        &ctx,
        CodemodeToolOptions::default(),
    )
    .await
    .expect("script executes");
    let content = result["content"].as_array().unwrap();
    assert_eq!(content[1]["text"], json!("tool exploded"));

    // An uncaught nested failure fails the script with the tool's text.
    let result = execute_codemode(
        "call-4",
        &json!({ "code": "await tools.failing({ value: 2 });" }),
        None,
        None,
        &ctx,
        CodemodeToolOptions::default(),
    )
    .await
    .expect("script executes");
    assert_eq!(result["isError"], json!(true));
    let content = result["content"].as_array().unwrap();
    let error_text = content.last().unwrap()["text"].as_str().unwrap();
    assert!(error_text.contains("tool exploded"), "{error_text}");
    assert!(
        error_text.contains("failing (error)"),
        "nested call recorded as error: {error_text}"
    );
}

#[test]
fn store_namespace_fixture_shape() {
    // The loadout namespace lookup drives describeNamespace; the shape is
    // pinned here at the seam.
    let namespace = ToolNamespace {
        name: "mcp__docs".to_string(),
        description: Some("Documentation tools".to_string()),
        instructions: None,
    };
    assert_eq!(namespace.name, "mcp__docs");
}
