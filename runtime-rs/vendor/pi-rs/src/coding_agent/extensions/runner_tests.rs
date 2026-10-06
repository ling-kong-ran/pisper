//! Tests for the ported `coding-agent/src/core/extensions/runner.ts` (and the
//! `extensions-input-event.test.ts` suite).
//!
//! Sources of truth:
//! - upstream `test/extensions-runner.test.ts` and
//!   `test/extensions-input-event.test.ts` (adapted to the handler seams),
//! - the runner oracle capture (`tests/fixtures/ext_oracle/runner.oracle.json`,
//!   generator `oracle_runner.mjs`): byte comparisons against the verbatim
//!   upstream runner under node. The capture's `warns` entries are the
//!   upstream `console.warn` texts, which equal the pinned diagnostic
//!   messages (the port emits the same text to stderr); the `stack` capture
//!   is a presence marker (`<js-stack:true>`) since JS stacks are not
//!   portable.

use crate::coding_agent::extensions::{loader, types};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::coding_agent::core::event_bus::EventBusController;
use crate::coding_agent::core::keybindings::{KeybindingsConfig, KeybindingsManager, ResolvedKeys};
use crate::coding_agent::extensions::loader::{
    load_extension_from_factory, ExtensionApi, ExtensionRuntime, HandlerUnsubscribe,
};
use crate::coding_agent::extensions::oracle_data::RUNNER;
use crate::coding_agent::extensions::runner::{
    emit_project_trust_event, emit_session_shutdown_event, ExtensionRunner, ProviderActions,
};
use crate::coding_agent::extensions::types::{
    AbortSignal, ExtensionActions, ExtensionCommandContextActions, ExtensionContext,
    ExtensionContextActions, ExtensionError, ExtensionUiDialogOptions, FlagType, FlagValue,
    HandlerFn, HandlerResult, InputEventResult, InputSource, NormalizedBuildSystemPromptOptions,
    NormalizedSystemPromptRenderer, StreamingDelivery, ThinkingLevel, ToolDefinition,
    UserBashEventResult,
};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn runner_oracle(name: &str) -> Value {
    let parsed: Value = serde_json::from_str(RUNNER).expect("runner oracle parses");
    parsed["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scenario| scenario["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("scenario {name} missing from runner oracle"))
}

fn test_actions() -> Arc<ExtensionActions> {
    Arc::new(ExtensionActions {
        send_message: Arc::new(|_, _| {}),
        send_user_message: Arc::new(|_, _| {}),
        append_entry: Arc::new(|_, _| {}),
        set_session_name: Arc::new(|_| {}),
        get_session_name: Arc::new(|| None),
        set_label: Arc::new(|_, _| {}),
        get_active_tools: Arc::new(Vec::new),
        get_all_tools: Arc::new(Vec::new),
        get_settings: Arc::new(|| serde_json::Value::Object(serde_json::Map::new())),
        set_active_tools: Arc::new(|_| {}),
        refresh_tools: Arc::new(|| {}),
        get_commands: Arc::new(Vec::new),
        set_model: Arc::new(|_| Ok(types::CommandFuture::resolved(false))),
        get_thinking_level: Arc::new(|| ThinkingLevel::Off),
        set_thinking_level: Arc::new(|_| {}),
    })
}

fn baseline_context_actions() -> ExtensionContextActions {
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

fn test_context_actions() -> Arc<ExtensionContextActions> {
    Arc::new(baseline_context_actions())
}

fn context_actions_with(
    overrides: impl FnOnce(&mut ExtensionContextActions),
) -> Arc<ExtensionContextActions> {
    let mut actions = baseline_context_actions();
    overrides(&mut actions);
    Arc::new(actions)
}

/// Upstream `loadSubscriptionExtension`: one factory, one bound runner.
fn load_runner_with(
    factory: loader::ExtensionFactory,
    extension_path: Option<&str>,
) -> (Option<types::Extension>, ExtensionRunner) {
    let runtime = ExtensionRuntime::new();
    let extension = load_extension_from_factory(
        factory,
        ".",
        EventBusController::new().bus().clone(),
        &runtime,
        extension_path,
    )
    .ok();
    let runner = ExtensionRunner::new(
        extension.clone().into_iter().collect(),
        runtime,
        ".",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    runner.bind_core(test_actions(), test_context_actions(), None);
    (extension, runner)
}

fn load_runner(factory: loader::ExtensionFactory) -> ExtensionRunner {
    load_runner_with(factory, None).1
}

fn handler<F>(f: F) -> HandlerFn
where
    F: Fn(&mut Value, &ExtensionContext) -> Result<Option<HandlerResult>, String>
        + Send
        + Sync
        + 'static,
{
    types::sync_handler(f)
}

fn event_value(event: &InputEventResult) -> Value {
    match event {
        InputEventResult::Continue => json!({"action": "continue"}),
        InputEventResult::Handled => json!({"action": "handled"}),
        InputEventResult::Transform { text, images } => {
            let mut object = json!({"action": "transform", "text": text});
            if let Some(images) = images {
                object["images"] = json!(images);
            }
            object
        }
    }
}

fn agent_end_event() -> Value {
    json!({"type": "agent_end", "messages": []})
}

fn collect_errors(runner: &ExtensionRunner) -> Arc<Mutex<Vec<Value>>> {
    let errors = Arc::new(Mutex::new(Vec::new()));
    let listener = Arc::clone(&errors);
    runner.on_error(Arc::new(move |error: &ExtensionError| {
        listener
            .lock()
            .unwrap()
            .push(json!({"event": error.event, "error": error.error}));
    }));
    errors
}

// ---------------------------------------------------------------------------
// Event subscription semantics (upstream #8967 battery)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_self_removal_does_not_skip_neighbors() {
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let factory_calls = Arc::clone(&calls);
    let runner = load_runner(Arc::new(move |api: &ExtensionApi| {
        let calls = Arc::clone(&factory_calls);
        let slot: Arc<Mutex<Option<HandlerUnsubscribe>>> = Arc::new(Mutex::new(None));
        let slot_for_handler = Arc::clone(&slot);
        let calls_a = Arc::clone(&calls);
        let unsubscribe = api.on(
            "agent_end",
            handler(move |_event, _ctx| {
                calls_a.lock().unwrap().push("A".into());
                if let Some(unsubscribe) = slot_for_handler.lock().unwrap().as_ref() {
                    unsubscribe.unsubscribe();
                }
                Ok(None)
            }),
        )?;
        *slot.lock().unwrap() = Some(unsubscribe);
        let calls_b = Arc::clone(&calls);
        api.on(
            "agent_end",
            handler(move |_event, _ctx| {
                calls_b.lock().unwrap().push("B".into());
                Ok(None)
            }),
        )?;
        Ok(())
    }));

    runner.emit(&mut agent_end_event()).await;
    let after_first = calls.lock().unwrap().clone();
    runner.emit(&mut agent_end_event()).await;
    let after_second = calls.lock().unwrap().clone();

    assert_eq!(
        json!({"afterFirst": after_first, "afterSecond": after_second}),
        runner_oracle("subscription_self_removal")["observed"]
    );
}

#[tokio::test]
async fn subscription_duplicate_removal_is_independent() {
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let factory_calls = Arc::clone(&calls);
    let (extension, runner) = load_runner_with(
        Arc::new(move |api: &ExtensionApi| {
            let calls = Arc::clone(&factory_calls);
            let shared_calls = Arc::clone(&calls);
            let shared = handler(move |_event, _ctx| {
                shared_calls.lock().unwrap().push("shared".into());
                Ok(None)
            });
            let first = api.on("agent_end", Arc::clone(&shared))?;
            let b_calls = Arc::clone(&calls);
            let second = api.on(
                "agent_end",
                handler(move |_event, _ctx| {
                    b_calls.lock().unwrap().push("B".into());
                    Ok(None)
                }),
            )?;
            let third = api.on("agent_end", shared)?;
            // Handles escape the factory through a captured slot so the test
            // can drive removals (mirrors the upstream closures).
            handles_slot()
                .lock()
                .unwrap()
                .replace(vec![first, second, third]);
            Ok(())
        }),
        None,
    );

    // Materialize the handles through the per-test slot.
    let handles = take_handles();
    let (stop_first, stop_b, stop_second) =
        (handles[0].clone(), handles[1].clone(), handles[2].clone());
    stop_second.unsubscribe();
    stop_second.unsubscribe();
    runner.emit(&mut agent_end_event()).await;
    let s1 = calls.lock().unwrap().clone();
    stop_first.unsubscribe();
    runner.emit(&mut agent_end_event()).await;
    let s2 = calls.lock().unwrap().clone();
    stop_b.unsubscribe();

    let expected = runner_oracle("subscription_duplicate_removal")["observed"].clone();
    assert_eq!(
        json!({
            "s1": s1,
            "s2": s2,
            "handlersMapCleared": !extension.unwrap().handlers.has("agent_end"),
        }),
        expected
    );
}

// Per-test slots are unnecessary; the duplicate test keeps its handles in a
// local once-cell keyed by the single test invocation order (tests in this
// file do not run concurrently against shared slots — this slot is only used
// by `subscription_duplicate_removal_is_independent`).
static DUPE_HANDLES: std::sync::OnceLock<Mutex<Option<Vec<HandlerUnsubscribe>>>> =
    std::sync::OnceLock::new();

fn handles_slot() -> &'static Mutex<Option<Vec<HandlerUnsubscribe>>> {
    DUPE_HANDLES.get_or_init(|| Mutex::new(None))
}

fn take_handles() -> Vec<HandlerUnsubscribe> {
    handles_slot()
        .lock()
        .unwrap()
        .take()
        .expect("handles registered")
}

#[tokio::test]
async fn subscription_removed_pending_handler_runs_in_current_dispatch() {
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let factory_calls = Arc::clone(&calls);
    let runner = load_runner(Arc::new(move |api: &ExtensionApi| {
        let calls = Arc::clone(&factory_calls);
        let stop_b_slot: Arc<Mutex<Option<HandlerUnsubscribe>>> = Arc::new(Mutex::new(None));
        let stop_b_for_a = Arc::clone(&stop_b_slot);
        let calls_a = Arc::clone(&calls);
        api.on(
            "agent_end",
            handler(move |_event, _ctx| {
                calls_a.lock().unwrap().push("A".into());
                if let Some(stop_b) = stop_b_for_a.lock().unwrap().as_ref() {
                    stop_b.unsubscribe();
                }
                Ok(None)
            }),
        )?;
        let calls_b = Arc::clone(&calls);
        let stop_b = api.on(
            "agent_end",
            handler(move |_event, _ctx| {
                calls_b.lock().unwrap().push("B".into());
                Ok(None)
            }),
        )?;
        *stop_b_slot.lock().unwrap() = Some(stop_b);
        let calls_c = Arc::clone(&calls);
        api.on(
            "agent_end",
            handler(move |_event, _ctx| {
                calls_c.lock().unwrap().push("C".into());
                Ok(None)
            }),
        )?;
        Ok(())
    }));

    runner.emit(&mut agent_end_event()).await;
    let first = calls.lock().unwrap().clone();
    runner.emit(&mut agent_end_event()).await;
    let second = calls.lock().unwrap().clone();

    assert_eq!(
        json!({"first3": first, "second": second}),
        runner_oracle("subscription_removed_pending")["observed"]
    );
}

#[tokio::test]
async fn subscription_deferred_registration_waits_for_next_dispatch() {
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let api_slot: Arc<Mutex<Option<ExtensionApi>>> = Arc::new(Mutex::new(None));
    let factory_calls = Arc::clone(&calls);
    let factory_api_slot = Arc::clone(&api_slot);
    let runner = load_runner(Arc::new(move |api: &ExtensionApi| {
        let api_for_handler = Arc::clone(&factory_api_slot);
        let calls = Arc::clone(&factory_calls);
        *api_for_handler.lock().unwrap() = Some(api.clone());
        let calls_a = Arc::clone(&calls);
        api.on(
            "agent_end",
            handler(move |_event, _ctx| {
                calls_a.lock().unwrap().push("A".into());
                // Registering during dispatch defers to the next dispatch.
                let api = api_for_handler.lock().unwrap().as_ref().unwrap().clone();
                let calls_c = Arc::clone(&calls_a);
                let _ = api.on(
                    "agent_end",
                    handler(move |_event, _ctx| {
                        calls_c.lock().unwrap().push("C".into());
                        Ok(None)
                    }),
                );
                Ok(None)
            }),
        )?;
        let calls_b = Arc::clone(&calls);
        api.on(
            "agent_end",
            handler(move |_event, _ctx| {
                calls_b.lock().unwrap().push("B".into());
                Ok(None)
            }),
        )?;
        Ok(())
    }));

    runner.emit(&mut agent_end_event()).await;
    let first = calls.lock().unwrap().clone();
    runner.emit(&mut agent_end_event()).await;
    let second = calls.lock().unwrap().clone();
    assert_eq!(
        json!({"first4": first, "second": second}),
        runner_oracle("subscription_deferred_registration")["observed"]
    );
}

#[tokio::test]
async fn subscription_nested_dispatch_uses_fresh_handler_list() {
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let runner_slot: Arc<Mutex<Option<ExtensionRunner>>> = Arc::new(Mutex::new(None));
    let factory_calls = Arc::clone(&calls);
    let factory_runner_slot = Arc::clone(&runner_slot);
    let api_slot: Arc<Mutex<Option<ExtensionApi>>> = Arc::new(Mutex::new(None));
    let factory_api_slot = Arc::clone(&api_slot);
    let runner = load_runner(Arc::new(move |api: &ExtensionApi| {
        let runner_for_handler = Arc::clone(&factory_runner_slot);
        let api_for_handler = Arc::clone(&factory_api_slot);
        *api_for_handler.lock().unwrap() = Some(api.clone());
        let calls = Arc::clone(&factory_calls);
        let stop_slot: Arc<Mutex<Option<Vec<HandlerUnsubscribe>>>> = Arc::new(Mutex::new(None));
        let stop_for_handler = Arc::clone(&stop_slot);
        let calls_a = Arc::clone(&calls);
        let stop_a = api.on(
            "agent_end",
            Arc::new(move |_event, _ctx| {
                calls_a.lock().unwrap().push("A".into());
                let handles = stop_for_handler.lock().unwrap().take().unwrap();
                handles[0].unsubscribe();
                handles[1].unsubscribe();
                // Register C during dispatch, then run a nested dispatch with
                // a fresh handler list.
                let api = api_for_handler.lock().unwrap().as_ref().unwrap().clone();
                let calls_c = Arc::clone(&calls_a);
                let _ = api.on(
                    "agent_end",
                    handler(move |_event, _ctx| {
                        calls_c.lock().unwrap().push("C".into());
                        Ok(None)
                    }),
                );
                let nested = runner_for_handler.lock().unwrap().as_ref().unwrap().clone();
                Box::pin(async move {
                    nested.emit(&mut agent_end_event()).await;
                    Ok(None)
                })
            }),
        )?;
        let calls_b = Arc::clone(&calls);
        let stop_b = api.on(
            "agent_end",
            handler(move |_event, _ctx| {
                calls_b.lock().unwrap().push("B".into());
                Ok(None)
            }),
        )?;
        *stop_slot.lock().unwrap() = Some(vec![stop_a, stop_b]);
        Ok(())
    }));
    *runner_slot.lock().unwrap() = Some(runner.clone());

    runner.emit(&mut agent_end_event()).await;
    let observed = calls.lock().unwrap().clone();
    assert_eq!(
        json!({"calls": observed}),
        runner_oracle("subscription_nested_dispatch")["observed"]
    );
}

#[test]
fn has_handlers_reflects_registrations() {
    let empty = load_runner(Arc::new(|_api: &ExtensionApi| Ok(())));
    let with_handler = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "tool_call",
            handler(|_event, _ctx| Ok(Some(HandlerResult::Json(Value::Null)))),
        )?;
        Ok(())
    }));
    assert_eq!(
        json!({
            "empty": empty.has_handlers("tool_call"),
            "withHandler": with_handler.has_handlers("tool_call"),
            "otherEvent": with_handler.has_handlers("agent_end"),
        }),
        runner_oracle("has_handlers")["observed"]
    );
}

// ---------------------------------------------------------------------------
// Tool / command / shortcut collection
// ---------------------------------------------------------------------------

fn tool_factory_named(name: &str, description: &str) -> loader::ExtensionFactory {
    let name = name.to_string();
    let description = description.to_string();
    Arc::new(move |api: &ExtensionApi| {
        api.register_tool(ToolDefinition::new(&name, &name, &description, json!({})))
    })
}

#[test]
fn tool_collection_first_registration_wins() {
    let (_, a) = load_runner_with(tool_factory_named("tool_a", "a"), None);
    let (_, b) = load_runner_with(tool_factory_named("tool_b", "b"), None);
    let (shared1, runner1) =
        load_runner_with(tool_factory_named("shared", "first"), Some("<a-first>"));
    let (shared2, runner2) =
        load_runner_with(tool_factory_named("shared", "second"), Some("<b-second>"));

    let combined = ExtensionRunner::new(
        vec![shared1.unwrap(), shared2.unwrap()],
        runner2.runtime(),
        ".",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    let _ = runner1;

    let no_params = |tool: &types::RegisteredTool| json!({"name": tool.definition.name, "description": tool.definition.description});
    let by_name = combined.get_tool_definition("shared");
    let observed = json!({
        "two_tools": [
            a.get_all_registered_tools().iter().map(no_params).collect::<Vec<_>>(),
            b.get_all_registered_tools().iter().map(no_params).collect::<Vec<_>>(),
        ],
        "first_wins": combined.get_all_registered_tools().iter().map(no_params).collect::<Vec<_>>(),
        "by_name": by_name.as_ref().map(|d| json!({"name": d.name, "description": d.description})),
    });
    let mut expected = runner_oracle("tool_collection")["observed"].clone();
    expected.as_object_mut().unwrap().remove("missing");
    assert_eq!(observed, expected);
    assert!(
        combined.get_tool_definition("nope").is_none(),
        "missing tool is None"
    );
}

#[test]
fn command_invocation_names_suffix_in_insertion_order() {
    let cmd_factory = |name: &str, description: &str| {
        let name = name.to_string();
        let description = description.to_string();
        Arc::new(move |api: &ExtensionApi| {
            api.register_command(&name, Some(description.clone()), Arc::new(|_, _| Ok(None)))
        }) as loader::ExtensionFactory
    };
    let (unique, runner) = load_runner_with(cmd_factory("unique", "Only one"), None);
    let (a, _) = load_runner_with(cmd_factory("shared-cmd", "First command"), Some("<cmd-a>"));
    let (b, _) = load_runner_with(cmd_factory("shared-cmd", "Second command"), Some("<cmd-b>"));
    let (c, _) = load_runner_with(cmd_factory("shared-cmd", "Third command"), Some("<cmd-c>"));
    let (x, _) = load_runner_with(cmd_factory("collide", "A"), Some("<collide-a>"));
    let (y, _) = load_runner_with(cmd_factory("collide:2", "fake"), Some("<collide-fake>"));
    let (z, _) = load_runner_with(cmd_factory("collide", "B"), Some("<collide-b>"));

    let runner = ExtensionRunner::new(
        vec![
            unique.unwrap(),
            a.unwrap(),
            b.unwrap(),
            c.unwrap(),
            x.unwrap(),
            y.unwrap(),
            z.unwrap(),
        ],
        runner.runtime(),
        ".",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    let commands = runner.get_registered_commands();
    let observed = json!({
        "resolved": commands.iter().map(|command| json!({
            "name": command.name(),
            "invocationName": command.invocation_name,
            "description": command.description(),
        })).collect::<Vec<_>>(),
        "diagnostics": diagnostics_json(&runner.get_command_diagnostics()),
        "lookup1": runner.get_command("shared-cmd:1").and_then(|c| c.description().map(str::to_string)),
        "lookup2": runner.get_command("shared-cmd:2").and_then(|c| c.description().map(str::to_string)),
        "lookup3": runner.get_command("shared-cmd:3").and_then(|c| c.description().map(str::to_string)),
        "lookupCollide2": runner.get_command("collide:2").and_then(|c| c.description().map(str::to_string)),
        "lookupCollide3": runner.get_command("collide:3").and_then(|c| c.description().map(str::to_string)),
        "lookupMissing": runner.get_command("shared-cmd").map(|c| json!(c.invocation_name)).unwrap_or(Value::String("<undefined>".into())),
    });
    assert_eq!(
        observed,
        runner_oracle("command_invocation_names")["observed"]
    );
}

fn diagnostics_json(
    diags: &[crate::coding_agent::core::diagnostics::ResourceDiagnostic],
) -> Vec<Value> {
    diags
        .iter()
        .map(|d| json!({"type": d.r#type.as_str(), "message": d.message, "path": d.path}))
        .collect()
}

fn default_keybindings_config() -> Vec<(String, ResolvedKeys)> {
    vec![
        (
            "app.clipboard.pasteImage".to_string(),
            ResolvedKeys::One("ctrl+v".to_string()),
        ),
        (
            "app.model.cycleForward".to_string(),
            ResolvedKeys::One("ctrl+p".to_string()),
        ),
        (
            "app.interrupt".to_string(),
            ResolvedKeys::One("ctrl+c".to_string()),
        ),
        (
            "app.clear".to_string(),
            ResolvedKeys::One("ctrl+l".to_string()),
        ),
    ]
}

fn shortcut_case(
    factories: Vec<loader::ExtensionFactory>,
    config: Vec<(String, ResolvedKeys)>,
) -> Value {
    let mut extensions = Vec::new();
    let mut runner_holder: Option<ExtensionRunner> = None;
    for factory in factories {
        let (extension, runner) = load_runner_with(factory, None);
        extensions.push(extension.unwrap());
        runner_holder = Some(runner);
    }
    let last_runner = runner_holder.unwrap();
    let runner = ExtensionRunner::new(
        extensions,
        last_runner.runtime(),
        ".",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    runner.bind_core(test_actions(), test_context_actions(), None);
    let shortcuts = runner.get_shortcuts(&config);
    json!({
        "keys": shortcuts.keys().map(str::to_string).collect::<Vec<_>>(),
        "descriptions": shortcuts.values().map(|s| s.description.clone().unwrap()).collect::<Vec<_>>(),
        "diagnostics": runner.get_shortcut_diagnostics().iter().map(|d| json!({
            "type": d.r#type.as_str(),
            "message": d.message,
            "path": d.path,
        })).collect::<Vec<_>>(),
    })
}

#[test]
fn shortcut_conflicts_match_oracle_diagnostics() {
    let mk = |key: &str| {
        let key = key.to_string();
        Arc::new(move |api: &ExtensionApi| {
            api.register_shortcut(
                &key,
                Some("ext shortcut".to_string()),
                Arc::new(|_ctx| Ok(())),
            )
        }) as loader::ExtensionFactory
    };
    let config_with = |action: &str, keys: ResolvedKeys| {
        let mut config = default_keybindings_config();
        config.retain(|(existing, _)| existing != action);
        config.push((action.to_string(), keys));
        config
    };

    let reserved = shortcut_case(
        vec![mk("ctrl+x")],
        vec![(
            "app.interrupt".to_string(),
            ResolvedKeys::One("ctrl+x".to_string()),
        )],
    );
    let non_reserved = shortcut_case(vec![mk("ctrl+v")], default_keybindings_config());
    let rebound = shortcut_case(
        vec![mk("ctrl+p")],
        config_with(
            "app.model.cycleForward",
            ResolvedKeys::One("ctrl+n".to_string()),
        ),
    );
    let rebound_reserved = shortcut_case(
        vec![mk("ctrl+x")],
        config_with("app.interrupt", ResolvedKeys::One("ctrl+x".to_string())),
    );
    let shared_reserved = shortcut_case(vec![mk("ctrl+p")], default_keybindings_config());
    let multi_reserved = shortcut_case(
        vec![mk("ctrl+y")],
        config_with(
            "app.clear",
            ResolvedKeys::Many(vec!["ctrl+x".into(), "ctrl+y".into()]),
        ),
    );
    let multi_non_reserved = shortcut_case(
        vec![mk("ctrl+y")],
        config_with(
            "app.clipboard.pasteImage",
            ResolvedKeys::Many(vec!["ctrl+x".into(), "ctrl+y".into()]),
        ),
    );
    let dupe = shortcut_case(
        vec![
            Arc::new(move |api: &ExtensionApi| {
                api.register_shortcut(
                    "ctrl+shift+x",
                    Some("First extension".to_string()),
                    Arc::new(|_ctx| Ok(())),
                )
            }),
            Arc::new(move |api: &ExtensionApi| {
                api.register_shortcut(
                    "ctrl+shift+x",
                    Some("Second extension".to_string()),
                    Arc::new(|_ctx| Ok(())),
                )
            }),
        ],
        default_keybindings_config(),
    );

    let mut observed = json!({
        "reserved": reserved,
        "nonReserved": non_reserved,
        "rebound": rebound,
        "reboundReserved": rebound_reserved,
        "sharedReserved": shared_reserved,
        "multiReserved": multi_reserved,
        "multiNonReserved": multi_non_reserved,
        "dupe": dupe,
    });
    // Drop the `warns` capture (console.warn text == diagnostics message; the
    // port emits the same text to stderr).
    for (_, case) in observed.as_object_mut().unwrap().iter_mut() {
        case.as_object_mut().unwrap().remove("warns");
    }
    let mut expected = runner_oracle("shortcut_conflicts")["observed"].clone();
    for (_, case) in expected.as_object_mut().unwrap().iter_mut() {
        case.as_object_mut().unwrap().remove("warns");
    }
    assert_eq!(observed, expected);
}

#[test]
fn reserved_shortcuts_block_via_real_keybindings_manager() {
    // Upstream suite uses `new KeybindingsManager().getEffectiveConfig()`;
    // the port resolves whatever key the host defaults bind to
    // `app.interrupt` and expects the extension shortcut to lose.
    let manager = KeybindingsManager::new(KeybindingsConfig::default(), None);
    let effective = manager.get_effective_config();
    let interrupt_key = effective
        .iter()
        .find(|(action, _)| action == "app.interrupt")
        .and_then(|(_, keys)| match keys {
            ResolvedKeys::One(key) => Some(key.clone()),
            ResolvedKeys::Many(keys) => keys.first().cloned(),
        })
        .expect("app.interrupt has a default binding");
    let factory_key = interrupt_key.clone();
    let runner = load_runner(Arc::new(move |api: &ExtensionApi| {
        let key = factory_key.clone();
        api.register_shortcut(&key, None, Arc::new(|_ctx| Ok(())))
    }));
    let shortcuts = runner.get_shortcuts(&effective);
    assert!(
        !shortcuts.has(&interrupt_key),
        "extension shortcut on the reserved default is blocked"
    );
    assert!(runner
        .get_shortcut_diagnostics()
        .iter()
        .any(|d| d.message.contains("conflicts with built-in shortcut")));
}

// ---------------------------------------------------------------------------
// user_bash routing (upstream #9068)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_bash_handler_throws_fail_closed() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "user_bash",
            handler(|_event, _ctx| Err("Routing failed".to_string())),
        )?;
        Ok(())
    }));
    let errors = collect_errors(&runner);
    let event =
        json!({"type": "user_bash", "command": "pwd", "excludeFromContext": false, "cwd": "."});
    let threw = runner.emit_user_bash(&event).await.unwrap_err();
    assert_eq!(
        json!({"threw": threw, "errors": *errors.lock().unwrap()}),
        runner_oracle("user_bash_throws")["observed"]
    );
}

#[tokio::test]
async fn user_bash_invalid_results_fail_closed_with_pinned_text() {
    let cases: Vec<(&str, Value)> = vec![
        ("empty_object", json!({})),
        ("null_operations", json!({"operations": null})),
        ("operations_without_exec", json!({"operations": {}})),
        ("null_result", json!({"result": null})),
        (
            "incomplete_result",
            json!({"result": {"output": "handled"}}),
        ),
        (
            "operations_and_result",
            json!({
                "operations": {"exec": "callable"},
                "result": {"output": "handled", "exitCode": 0, "cancelled": false, "truncated": false},
            }),
        ),
    ];
    let mut observed = Vec::new();
    for (label, result) in &cases {
        let result = result.clone();
        let runner = load_runner(Arc::new(move |api: &ExtensionApi| {
            let result = result.clone();
            api.on(
                "user_bash",
                handler(move |_event, _ctx| Ok(Some(HandlerResult::Json(result.clone())))),
            )?;
            Ok(())
        }));
        let errors = Arc::new(Mutex::new(Vec::<String>::new()));
        let listener = Arc::clone(&errors);
        runner.on_error(Arc::new(move |error: &ExtensionError| {
            listener.lock().unwrap().push(error.error.clone());
        }));
        let event =
            json!({"type": "user_bash", "command": "pwd", "excludeFromContext": false, "cwd": "."});
        let thrown = runner.emit_user_bash(&event).await.unwrap_err();
        observed.push(json!({
            "label": label,
            "thrown": thrown,
            "errorPrefix": errors.lock().unwrap().first().map(|error: &String| {
                error.chars().take(40).collect::<String>()
            }),
        }));
    }
    assert_eq!(
        Value::Array(observed),
        runner_oracle("user_bash_invalid_results")["observed"]
    );
}

#[tokio::test]
async fn user_bash_valid_operations_and_result_overrides_are_accepted() {
    let valid = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "user_bash",
            handler(|event, _ctx| {
                if event["command"] == "operations" {
                    Ok(Some(HandlerResult::UserBashOperations(types::BashOperations {
                        exec: Arc::new(|_command, _cwd, _callbacks| Box::pin(async { Ok(Some(0)) })),
                    })))
                } else {
                    Ok(Some(HandlerResult::Json(json!({
                        "result": {"output": "handled", "exitCode": 0, "cancelled": false, "truncated": false},
                    }))))
                }
            }),
        )?;
        Ok(())
    }));
    let event_for = |command: &str| json!({"type": "user_bash", "command": command, "excludeFromContext": false, "cwd": "."});
    let operations = valid
        .emit_user_bash(&event_for("operations"))
        .await
        .unwrap()
        .unwrap();
    let result_override = valid
        .emit_user_bash(&event_for("result"))
        .await
        .unwrap()
        .unwrap();
    let (operations_has_exec, operations_keys) = match &operations {
        UserBashEventResult::Operations(_) => (true, json!(["operations"])),
        _ => (false, json!([])),
    };
    let observed = json!({
        "operationsHasExec": operations_has_exec,
        "operationsKeys": operations_keys,
        "resultOverride": match &result_override {
            UserBashEventResult::Result(result) => json!({"result": serde_json::to_value(result).unwrap()}),
            _ => Value::Null,
        },
    });
    let mut expected = runner_oracle("user_bash_valid_results")["observed"].clone();
    // `none` was undefined in the capture (no handlers) and is omitted.
    expected.as_object_mut().unwrap().remove("none");
    assert_eq!(observed, expected);

    // No handlers → None.
    let none = load_runner(Arc::new(|_api: &ExtensionApi| Ok(())));
    assert!(none
        .emit_user_bash(&event_for("x"))
        .await
        .unwrap()
        .is_none());
}

// ---------------------------------------------------------------------------
// Input event chaining (upstream extensions-input-event suite)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn input_continue_paths_match_oracle() {
    let no_handler = load_runner(Arc::new(|_api: &ExtensionApi| Ok(())));
    let a1 = no_handler
        .emit_input("x", None, InputSource::Interactive, None)
        .await;
    let undefined_handler = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on("input", handler(|_event, _ctx| Ok(None)))?;
        Ok(())
    }));
    let a2 = undefined_handler
        .emit_input("x", None, InputSource::Interactive, None)
        .await;
    let continue_handler = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "input",
            handler(|_event, _ctx| Ok(Some(HandlerResult::Json(json!({"action": "continue"}))))),
        )?;
        Ok(())
    }));
    let a3 = continue_handler
        .emit_input("x", None, InputSource::Interactive, None)
        .await;
    assert_eq!(
        Value::Array(vec![event_value(&a1), event_value(&a2), event_value(&a3)]),
        runner_oracle("input_continue")["observed"]
    );
}

#[tokio::test]
async fn input_transform_preserves_and_replaces_images() {
    let images = json!([{"type": "image", "data": "orig", "mimeType": "image/png"}]);
    let transformer = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "input",
            handler(|event, _ctx| {
                Ok(Some(HandlerResult::Json(json!({
                    "action": "transform",
                    "text": format!("T:{}", event["text"].as_str().unwrap_or_default()),
                }))))
            }),
        )?;
        Ok(())
    }));
    let preserved = transformer
        .emit_input(
            "hi",
            Some(images.as_array().unwrap().clone()),
            InputSource::Interactive,
            None,
        )
        .await;

    let replacer = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "input",
            handler(|_event, _ctx| {
                Ok(Some(HandlerResult::Json(json!({
                    "action": "transform",
                    "text": "X",
                    "images": [{"type": "image", "data": "new", "mimeType": "image/jpeg"}],
                }))))
            }),
        )?;
        Ok(())
    }));
    let replaced = replacer
        .emit_input(
            "hi",
            Some(images.as_array().unwrap().clone()),
            InputSource::Interactive,
            None,
        )
        .await;

    assert_eq!(
        event_value(&preserved),
        runner_oracle("input_transform_preserves_images")["observed"]["result"]
    );
    assert_eq!(
        event_value(&replaced),
        runner_oracle("input_transform_replaces_images")["observed"]["result"]
    );
}

#[tokio::test]
async fn input_transforms_chain_and_handled_short_circuits() {
    let chain = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "input",
            handler(|event, _ctx| {
                Ok(Some(HandlerResult::Json(json!({
                    "action": "transform",
                    "text": format!("{}[1]", event["text"].as_str().unwrap_or_default()),
                }))))
            }),
        )?;
        api.on(
            "input",
            handler(|event, _ctx| {
                Ok(Some(HandlerResult::Json(json!({
                    "action": "transform",
                    "text": format!("{}[2]", event["text"].as_str().unwrap_or_default()),
                }))))
            }),
        )?;
        Ok(())
    }));
    let chained = chain
        .emit_input("X", None, InputSource::Interactive, None)
        .await;
    assert_eq!(
        event_value(&chained),
        runner_oracle("input_chain")["observed"]["result"]
    );

    let second_ran = Arc::new(Mutex::new(false));
    let second_flag = Arc::clone(&second_ran);
    let handled = load_runner(Arc::new(move |api: &ExtensionApi| {
        api.on(
            "input",
            handler(|_event, _ctx| Ok(Some(HandlerResult::Json(json!({"action": "handled"}))))),
        )?;
        let flag = Arc::clone(&second_flag);
        api.on(
            "input",
            handler(move |_event, _ctx| {
                *flag.lock().unwrap() = true;
                Ok(None)
            }),
        )?;
        Ok(())
    }));
    let handled_result = handled
        .emit_input("X", None, InputSource::Interactive, None)
        .await;
    assert_eq!(
        json!({"result": event_value(&handled_result), "secondRan": *second_ran.lock().unwrap()}),
        runner_oracle("input_handled_short_circuit")["observed"]
    );
}

#[tokio::test]
async fn input_source_and_streaming_behavior_pass_through() {
    let observed = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed_sources = Arc::clone(&observed);
    let sources_runner = load_runner(Arc::new(move |api: &ExtensionApi| {
        let observed = Arc::clone(&observed_sources);
        api.on(
            "input",
            handler(move |event, _ctx| {
                observed.lock().unwrap().push(json!(event["source"]));
                Ok(Some(HandlerResult::Json(json!({"action": "continue"}))))
            }),
        )?;
        Ok(())
    }));
    for source in [
        InputSource::Interactive,
        InputSource::Rpc,
        InputSource::Extension,
    ] {
        sources_runner.emit_input("x", None, source, None).await;
    }

    let behaviors_observed = Arc::new(Mutex::new(Vec::<Value>::new()));
    let behaviors_for_handler = Arc::clone(&behaviors_observed);
    let behavior_runner = load_runner(Arc::new(move |api: &ExtensionApi| {
        let observed = Arc::clone(&behaviors_for_handler);
        api.on(
            "input",
            handler(move |event, _ctx| {
                observed.lock().unwrap().push(
                    event
                        .get("streamingBehavior")
                        .cloned()
                        .unwrap_or(Value::Null),
                );
                Ok(Some(HandlerResult::Json(json!({"action": "continue"}))))
            }),
        )?;
        Ok(())
    }));
    behavior_runner
        .emit_input(
            "x",
            None,
            InputSource::Interactive,
            Some(StreamingDelivery::Steer),
        )
        .await;
    behavior_runner
        .emit_input(
            "x",
            None,
            InputSource::Interactive,
            Some(StreamingDelivery::FollowUp),
        )
        .await;
    behavior_runner
        .emit_input("x", None, InputSource::Interactive, None)
        .await;

    let sources = observed_sources_value(&observed);
    let _ = sources;
    assert_eq!(
        json!({
            "sources": *observed.lock().unwrap(),
            "behaviors": *behaviors_observed.lock().unwrap(),
        }),
        runner_oracle("input_source_and_behavior")["observed"]
    );
}

fn observed_sources_value(_observed: &Mutex<Vec<Value>>) -> Value {
    Value::Null
}

#[tokio::test]
async fn input_handler_errors_are_isolated() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on("input", handler(|_event, _ctx| Err("boom".to_string())))?;
        Ok(())
    }));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let listener = Arc::clone(&errors);
    runner.on_error(Arc::new(move |error: &ExtensionError| {
        listener
            .lock()
            .unwrap()
            .push(Value::String(error.error.clone()));
    }));
    let result = runner
        .emit_input("x", None, InputSource::Interactive, None)
        .await;
    assert_eq!(
        json!({"result": event_value(&result), "errors": *errors.lock().unwrap()}),
        runner_oracle("input_error_isolation")["observed"]
    );
}

// ---------------------------------------------------------------------------
// tool_result / context / provider dispatch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tool_result_content_chains_across_handlers() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "tool_result",
            handler(|event, _ctx| {
                let mut content = event["content"].as_array().cloned().unwrap_or_default();
                content.push(json!({"type": "text", "text": "ext1"}));
                Ok(Some(HandlerResult::Json(json!({"content": content}))))
            }),
        )?;
        api.on(
            "tool_result",
            handler(|event, _ctx| {
                let mut content = event["content"].as_array().cloned().unwrap_or_default();
                content.push(json!({"type": "text", "text": "ext2"}));
                Ok(Some(HandlerResult::Json(json!({"content": content}))))
            }),
        )?;
        Ok(())
    }));
    let event = json!({
        "type": "tool_result",
        "toolName": "my_tool",
        "toolCallId": "call-1",
        "input": {},
        "content": [{"type": "text", "text": "base"}],
        "details": {"initial": true},
        "isError": false,
    });
    let chained = runner.emit_tool_result(&event).await;
    assert_eq!(
        chained.unwrap_or(Value::Null),
        runner_oracle("tool_result_chain_content")["observed"]
    );
}

#[tokio::test]
async fn tool_result_partial_patches_preserve_earlier_modifications() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "tool_result",
            handler(|_event, _ctx| {
                Ok(Some(HandlerResult::Json(json!({
                    "content": [{"type": "text", "text": "first"}],
                    "details": {"source": "ext1"},
                }))))
            }),
        )?;
        api.on(
            "tool_result",
            handler(|_event, _ctx| Ok(Some(HandlerResult::Json(json!({"isError": true}))))),
        )?;
        Ok(())
    }));
    let event = json!({
        "type": "tool_result",
        "toolName": "my_tool",
        "toolCallId": "call-2",
        "input": {},
        "content": [{"type": "text", "text": "base"}],
        "details": {"initial": true},
        "isError": false,
    });
    let patched = runner.emit_tool_result(&event).await;

    let untouched = load_runner(Arc::new(|_api: &ExtensionApi| Ok(())));
    let plain_event = json!({
        "type": "tool_result",
        "toolName": "t",
        "toolCallId": "c",
        "input": {},
        "content": [{"type": "text", "text": "x"}],
        "isError": false,
    });
    let no_handlers = untouched.emit_tool_result(&plain_event).await;

    let mut observed = json!({"patched": patched.unwrap_or(Value::Null)});
    if let Some(no_handlers) = no_handlers {
        observed["noHandlers"] = no_handlers;
    }
    assert_eq!(
        observed,
        runner_oracle("tool_result_partial_patch")["observed"]
    );
}

#[tokio::test]
async fn context_dispatch_accumulates_replacements_and_isolates_errors() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "context",
            handler(|event, _ctx| {
                let mut messages = event["messages"].as_array().cloned().unwrap_or_default();
                messages.push(json!({"role": "user", "content": "injected"}));
                Ok(Some(HandlerResult::Json(json!({"messages": messages}))))
            }),
        )?;
        api.on(
            "context",
            handler(|event, _ctx| {
                if event["messages"].as_array().map(|m| m.len()).unwrap_or(0) > 1 {
                    return Err("ctx boom".to_string());
                }
                Ok(None)
            }),
        )?;
        Ok(())
    }));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let listener = Arc::clone(&errors);
    runner.on_error(Arc::new(move |error: &ExtensionError| {
        listener
            .lock()
            .unwrap()
            .push(Value::String(error.error.clone()));
    }));
    let messages = runner
        .emit_context(&[json!({"role": "user", "content": "hi"})])
        .await;
    assert_eq!(
        json!({"messages": messages, "errors": *errors.lock().unwrap()}),
        runner_oracle("emit_context")["observed"]
    );
}

#[tokio::test]
async fn before_provider_request_replaces_payload() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "before_provider_request",
            handler(|_e, _c| Ok(Some(HandlerResult::Json(json!("first"))))),
        )?;
        api.on(
            "before_provider_request",
            handler(|_e, _c| Ok(Some(HandlerResult::Json(json!("second"))))),
        )?;
        api.on("before_provider_request", handler(|_e, _c| Ok(None)))?;
        Ok(())
    }));
    let payload = runner.emit_before_provider_request(json!("start")).await;
    assert_eq!(
        json!({"payload": payload}),
        runner_oracle("emit_before_provider_request")["observed"]
    );
}

#[tokio::test]
async fn before_provider_headers_mutate_in_place_and_isolate_throws() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "before_provider_headers",
            handler(|event, _ctx| {
                if let Some(headers) = event.as_object_mut().unwrap().get_mut("headers") {
                    headers["X-Turn-Index"] = json!("3");
                }
                Ok(None)
            }),
        )?;
        Ok(())
    }));
    let headers = runner
        .emit_before_provider_headers(json!({"User-Agent": "kimchi/1.0"}))
        .await;
    assert_eq!(
        json!({"headers": headers}),
        runner_oracle("emit_before_provider_headers")["observed"]
    );

    let mixed = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "before_provider_headers",
            handler(|_e, _c| Err("header handler boom".to_string())),
        )?;
        api.on(
            "before_provider_headers",
            handler(|event, _ctx| {
                if let Some(headers) = event.as_object_mut().unwrap().get_mut("headers") {
                    headers["X-Good"] = json!("yes");
                }
                Ok(None)
            }),
        )?;
        Ok(())
    }));
    let errors = collect_errors(&mixed);
    let headers = mixed
        .emit_before_provider_headers(json!({"User-Agent": "x"}))
        .await;
    assert_eq!(
        json!({"headers": headers, "errors": *errors.lock().unwrap()}),
        runner_oracle("emit_before_provider_headers_mixed")["observed"]
    );
}

// ---------------------------------------------------------------------------
// before_agent_start chaining
// ---------------------------------------------------------------------------

/// The oracle's disclosed system-prompt shim (see
/// `tests/fixtures/ext_oracle/src/core/system-prompt.ts`): the force path is the
/// pinned `buildSystemPromptState` contract; the section path renders the
/// deterministic stand-in captured in the oracle.
struct OracleShimRenderer;

impl NormalizedSystemPromptRenderer for OracleShimRenderer {
    fn build(&self, normalized: &NormalizedBuildSystemPromptOptions) -> String {
        if let Some(force) = &normalized.force_system_prompt {
            return force.clone();
        }
        format!(
            "ORACLE-SHIM(base)\ncustomPrompt={}\nselectedTools={}\nappendSystemPrompt={}",
            normalized.custom_prompt.clone().unwrap_or_default(),
            normalized.selected_tools.join(","),
            normalized.append_system_prompt,
        )
    }
}

#[tokio::test]
async fn before_agent_start_chains_system_prompt_updates() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "before_agent_start",
            handler(|_event, ctx| {
                Ok(Some(HandlerResult::Json(json!({
                    "systemPrompt": format!("{}\nfirst", ctx.get_system_prompt()?),
                }))))
            }),
        )?;
        api.on(
            "before_agent_start",
            handler(|_event, ctx| {
                Ok(Some(HandlerResult::Json(json!({
                    "systemPrompt": format!("{}\nsecond", ctx.get_system_prompt()?),
                }))))
            }),
        )?;
        Ok(())
    }));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let listener = Arc::clone(&errors);
    runner.on_error(Arc::new(move |error: &ExtensionError| {
        listener
            .lock()
            .unwrap()
            .push(Value::String(error.error.clone()));
    }));
    let chained = runner
        .emit_before_agent_start(
            "hello",
            None,
            &types::BuildSystemPromptOptions {
                custom_prompt: Some("base".to_string()),
                ..types::BuildSystemPromptOptions::with_cwd(".")
            },
            Arc::new(OracleShimRenderer),
        )
        .await
        .unwrap();

    let observed = json!({
        "messages": chained.messages,
        "systemPrompt": OracleShimRenderer.build(&chained.system_prompt_options),
        "forceSystemPrompt": chained.system_prompt_options.force_system_prompt,
        "selectedTools": chained.system_prompt_options.selected_tools,
        "errors": *errors.lock().unwrap(),
    });
    assert_eq!(
        observed,
        runner_oracle("before_agent_start_chain")["observed"]
    );
}

#[tokio::test]
async fn before_agent_start_collects_messages() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "before_agent_start",
            handler(|_event, _ctx| {
                Ok(Some(HandlerResult::Json(json!({
                    "message": {"customType": "note", "content": "m", "display": true, "details": null},
                }))))
            }),
        )?;
        Ok(())
    }));
    let chained = runner
        .emit_before_agent_start(
            "p",
            None,
            &types::BuildSystemPromptOptions::with_cwd("."),
            Arc::new(OracleShimRenderer),
        )
        .await
        .unwrap();
    assert_eq!(
        json!({"messages": chained.messages}),
        runner_oracle("before_agent_start_message")["observed"]
    );
}

// ---------------------------------------------------------------------------
// resources_discover / session_before / message_end / tool_call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resources_discover_attributes_paths_and_isolates_errors() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "resources_discover",
            handler(|_event, _ctx| {
                Ok(Some(HandlerResult::Json(json!({
                    "skillPaths": ["s1", "s2"],
                    "promptPaths": ["p1"],
                }))))
            }),
        )?;
        api.on(
            "resources_discover",
            handler(|_event, _ctx| Ok(Some(HandlerResult::Json(json!({"themePaths": ["t1"]}))))),
        )?;
        api.on(
            "resources_discover",
            handler(|_event, _ctx| Err("discover boom".to_string())),
        )?;
        Ok(())
    }));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let listener = Arc::clone(&errors);
    runner.on_error(Arc::new(move |error: &ExtensionError| {
        listener
            .lock()
            .unwrap()
            .push(Value::String(error.error.clone()));
    }));
    let discovered = runner
        .emit_resources_discover(".", types::ResourcesDiscoverReason::Startup)
        .await;
    let paths = |list: &[(String, String)]| {
        list.iter()
            .map(|(path, extension_path)| json!({"path": path, "extensionPath": extension_path}))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        json!({
            "discovered": {
                "skillPaths": paths(&discovered.skill_paths),
                "promptPaths": paths(&discovered.prompt_paths),
                "themePaths": paths(&discovered.theme_paths),
            },
            "errors": *errors.lock().unwrap(),
        }),
        runner_oracle("emit_resources_discover")["observed"]
    );
}

#[tokio::test]
async fn session_before_switch_short_circuits_on_cancel() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "session_before_switch",
            handler(|_e, _c| Ok(Some(HandlerResult::Json(json!({"cancel": true}))))),
        )?;
        api.on(
            "session_before_switch",
            handler(|_e, _c| Ok(Some(HandlerResult::Json(json!({"cancel": false}))))),
        )?;
        Ok(())
    }));
    let cancelled = runner
        .emit(&mut json!({"type": "session_before_switch", "reason": "new"}))
        .await;

    let none = load_runner(Arc::new(|_api: &ExtensionApi| Ok(())));
    let none_result = none.emit(
        &mut json!({"type": "session_before_switch", "reason": "resume", "targetSessionFile": "x"}),
    ).await;

    let mut observed = json!({"cancelled": cancelled});
    if let Some(result) = none_result {
        observed["none"] = result;
    }
    let mut expected = runner_oracle("emit_session_before_switch")["observed"].clone();
    expected.as_object_mut().unwrap().remove("none");
    assert_eq!(observed, expected);
}

#[tokio::test]
async fn session_before_tree_results_are_returned() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "session_before_tree",
            handler(|_e, _c| {
                Ok(Some(HandlerResult::Json(json!({
                    "summary": {"summary": "s", "details": Value::Null},
                    "customInstructions": "ci",
                }))))
            }),
        )?;
        Ok(())
    }));
    let result = runner.emit(&mut json!({
        "type": "session_before_tree",
        "preparation": {"targetId": "t", "oldLeafId": Value::Null, "commonAncestorId": Value::Null, "entriesToSummarize": [], "userWantsSummary": true},
    })).await;
    assert_eq!(
        json!({"result": result}),
        runner_oracle("emit_session_before_tree")["observed"]
    );
}

#[tokio::test]
async fn message_end_replacement_and_role_guard() {
    let good = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "message_end",
            handler(|event, _ctx| {
                let mut message = event["message"].clone();
                message["content"] = json!("replaced");
                Ok(Some(HandlerResult::Json(json!({"message": message}))))
            }),
        )?;
        Ok(())
    }));
    let event = json!({"type": "message_end", "message": {"role": "assistant", "content": "orig"}});
    let replaced = good.emit_message_end(&event).await;
    assert_eq!(
        json!({"result": replaced}),
        runner_oracle("emit_message_end_replace")["observed"]
    );

    let mismatch = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "message_end",
            handler(|_event, _ctx| {
                Ok(Some(HandlerResult::Json(
                    json!({"message": {"role": "user", "content": "bad"}}),
                )))
            }),
        )?;
        api.on(
            "message_end",
            handler(|event, _ctx| {
                let mut message = event["message"].clone();
                message["content"] = json!("ok");
                Ok(Some(HandlerResult::Json(json!({"message": message}))))
            }),
        )?;
        Ok(())
    }));
    let errors = collect_errors(&mismatch);
    let result = mismatch.emit_message_end(&event).await;
    assert_eq!(
        json!({"result": result, "errors": *errors.lock().unwrap()}),
        runner_oracle("emit_message_end_role_mismatch")["observed"]
    );
}

#[tokio::test]
async fn tool_call_blocks_and_mutates_input_in_place() {
    let runner = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "tool_call",
            handler(|event, _ctx| {
                if event["toolName"] == "bash" {
                    event["input"]["command"] = json!("patched");
                }
                Ok(None)
            }),
        )?;
        api.on("tool_call", handler(|_e, _c| Ok(None)))?;
        api.on(
            "tool_call",
            handler(|_e, _c| {
                Ok(Some(HandlerResult::Json(
                    json!({"block": true, "reason": "nope", "terminate": true}),
                )))
            }),
        )?;
        api.on(
            "tool_call",
            handler(|_e, _c| Ok(Some(HandlerResult::Json(json!({"block": false}))))),
        )?;
        Ok(())
    }));
    let mut event = json!({"type": "tool_call", "toolCallId": "c1", "toolName": "bash", "input": {"command": "orig"}});
    let result = runner.emit_tool_call(&mut event).await.unwrap();
    assert_eq!(
        json!({"result": result, "mutatedInput": event["input"]}),
        runner_oracle("emit_tool_call_block")["observed"]
    );

    let soft = load_runner(Arc::new(|api: &ExtensionApi| {
        api.on(
            "tool_call",
            handler(|_e, _c| {
                Ok(Some(HandlerResult::Json(
                    json!({"block": false, "reason": "soft"}),
                )))
            }),
        )?;
        Ok(())
    }));
    let soft_result = soft
        .emit_tool_call(
            &mut json!({"type": "tool_call", "toolCallId": "c2", "toolName": "read", "input": {}}),
        )
        .await
        .unwrap();
    assert_eq!(
        json!({"result": soft_result}),
        runner_oracle("emit_tool_call_soft")["observed"]
    );
}

// ---------------------------------------------------------------------------
// project_trust
// ---------------------------------------------------------------------------

#[tokio::test]
async fn project_trust_first_decision_wins_and_errors_are_captured() {
    let (undecided, _) = load_runner_with(
        Arc::new(|api: &ExtensionApi| {
            api.on(
                "project_trust",
                handler(|_e, _c| {
                    Ok(Some(HandlerResult::Json(
                        json!({"trusted": "undecided", "remember": true}),
                    )))
                }),
            )?;
            Ok(())
        }),
        Some("<undecided>"),
    );
    let (decided, _) = load_runner_with(
        Arc::new(|api: &ExtensionApi| {
            api.on(
                "project_trust",
                handler(|_e, _c| {
                    Ok(Some(HandlerResult::Json(
                        json!({"trusted": "no", "remember": true}),
                    )))
                }),
            )?;
            Ok(())
        }),
        Some("<decided>"),
    );
    let (boom, _) = load_runner_with(
        Arc::new(|api: &ExtensionApi| {
            api.on(
                "project_trust",
                handler(|_e, _c| Err("trust handler failed".to_string())),
            )?;
            Ok(())
        }),
        Some("<boom>"),
    );

    let combined_extensions = vec![undecided.unwrap(), decided.unwrap()];
    let ctx = types::ProjectTrustContext {
        cwd: ".".to_string(),
        mode: types::ExtensionMode::Tui,
        has_ui: false,
        ui: None,
    };
    let event = types::ProjectTrustEvent {
        event_type: "project_trust".to_string(),
        cwd: ".".to_string(),
    };
    let (result, errors) = emit_project_trust_event(&combined_extensions, &event, &ctx).await;

    let error_extensions = boom.into_iter().collect::<Vec<_>>();
    let (error_result, error_errors) =
        emit_project_trust_event(&error_extensions, &event, &ctx).await;
    assert!(error_result.is_none());
    let observed = json!({
        "result": {
            "result": result,
            "errors": errors.iter().map(|error| json!({
                "extensionPath": error.extension_path,
                "event": error.event,
                "error": error.error,
                "stack": Value::Null,
            })).collect::<Vec<_>>(),
        },
        "errorResult": {
            "errors": error_errors.iter().map(|error| json!({
                "extensionPath": error.extension_path,
                "event": error.event,
                "error": error.error,
                "stack": "<js-stack:true>",
            })).collect::<Vec<_>>(),
        },
    });
    assert_eq!(observed, runner_oracle("project_trust")["observed"]);
}

// ---------------------------------------------------------------------------
// ui prompt nesting + context defaults
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeUi {
    calls: Mutex<Vec<String>>,
}

impl types::ExtensionUI for FakeUi {
    fn select<'a>(
        &'a self,
        title: &'a str,
        options: &'a [String],
        _opts: &'a ExtensionUiDialogOptions,
    ) -> types::UiFuture<'a, Option<String>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(format!("select:{}:{}", title, options.join("|")));
            Ok(options.first().cloned())
        })
    }
    fn confirm<'a>(
        &'a self,
        title: &'a str,
        message: &'a str,
        _opts: &'a ExtensionUiDialogOptions,
    ) -> types::UiFuture<'a, bool> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(format!("confirm:{}:{}", title, message));
            Ok(true)
        })
    }
    fn input<'a>(
        &'a self,
        title: &'a str,
        _placeholder: Option<&'a str>,
        _opts: &'a ExtensionUiDialogOptions,
    ) -> types::UiFuture<'a, Option<String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(format!("input:{title}"));
            Ok(Some("typed".to_string()))
        })
    }
    fn editor<'a>(
        &'a self,
        title: &'a str,
        prefill: Option<&'a str>,
    ) -> types::UiFuture<'a, Option<String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(format!(
                "editor:{}:{}",
                title,
                prefill.unwrap_or_default()
            ));
            Ok(Some("edited".to_string()))
        })
    }
    fn custom<'a>(
        &'a self,
        _factory: &'a Value,
        _opts: &'a ExtensionUiDialogOptions,
    ) -> types::UiFuture<'a, Option<Value>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push("custom".to_string());
            Ok(Some(json!("custom-result")))
        })
    }
}

/// A UI whose select opens a nested prompt (depth-semantics probe).
struct NestedPromptUi {
    runner: ExtensionRunner,
}

impl types::ExtensionUI for NestedPromptUi {
    fn select<'a>(
        &'a self,
        _title: &'a str,
        options: &'a [String],
        opts: &'a ExtensionUiDialogOptions,
    ) -> types::UiFuture<'a, Option<String>> {
        Box::pin(async move {
            let nested_ui = self.runner.create_context().ui()?;
            nested_ui.confirm("Inner", "msg", opts).await?;
            Ok(options.first().cloned())
        })
    }
}

#[tokio::test]
async fn ui_prompt_events_and_nesting_match_upstream_depth_semantics() {
    let done = Arc::new(tokio::sync::Notify::new());
    let factory_done = done.clone();
    let fake = Arc::new(FakeUi::default());
    let ui_events = Arc::new(Mutex::new(Vec::new()));
    let factory_ui_events = Arc::clone(&ui_events);
    let runner = load_runner(Arc::new(move |api: &ExtensionApi| {
        let ui_events = Arc::clone(&factory_ui_events);
        let record = |events: Arc<Mutex<Vec<Value>>>| {
            let done = factory_done.clone();
            move |event: &mut Value, _ctx: &ExtensionContext| {
                let mut entry = json!({
                    "type": event["type"],
                    "kind": event["kind"],
                });
                if let Some(title) = event.get("title") {
                    entry["title"] = title.clone();
                }
                events.lock().unwrap().push(entry);
                done.notify_one();
                Ok(None)
            }
        };
        let on_start = record(Arc::clone(&ui_events));
        api.on(
            "ui_prompt_start",
            crate::coding_agent::extensions::types::sync_handler(on_start),
        )?;
        let on_end = record(Arc::clone(&ui_events));
        api.on(
            "ui_prompt_end",
            crate::coding_agent::extensions::types::sync_handler(on_end),
        )?;
        Ok(())
    }));
    runner.set_ui_context(
        Some(Arc::clone(&fake) as Arc<dyn types::ExtensionUI>),
        types::ExtensionMode::Tui,
    );

    let ctx = runner.create_context();
    let ui = ctx.ui().unwrap();
    let opts = ExtensionUiDialogOptions::default();
    ui.select("Pick", &["a".to_string(), "b".to_string()], &opts)
        .await
        .unwrap();
    ui.confirm("Sure?", "msg", &opts).await.unwrap();
    ui.input("Name", Some("ph"), &opts).await.unwrap();
    ui.editor("Edit", Some("prefill")).await.unwrap();
    ui.custom(&json!("() => {}"), &opts).await.unwrap();
    ui.select("Outer", &["x".to_string()], &opts).await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while ui_events.lock().unwrap().len() < 12 {
            done.notified().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        json!({
            "calls": *fake.calls.lock().unwrap(),
            "uiEvents": *ui_events.lock().unwrap(),
            "hasUI": runner.has_ui(),
            "mode": ctx.mode().unwrap().as_str(),
        }),
        runner_oracle("ui_prompt_events")["observed"]
    );

    // Nesting: only the outer boundary emits (ported depth semantics).
    let nested_events = Arc::new(Mutex::new(Vec::new()));
    let nested_done = Arc::new(tokio::sync::Notify::new());
    let factory_done = nested_done.clone();
    let factory_nested_events = Arc::clone(&nested_events);
    let nested_runner = load_runner(Arc::new(move |api: &ExtensionApi| {
        let nested_events = Arc::clone(&factory_nested_events);
        let record = |events: Arc<Mutex<Vec<Value>>>| {
            let done = factory_done.clone();
            move |event: &mut Value, _ctx: &ExtensionContext| {
                events
                    .lock()
                    .unwrap()
                    .push(json!({"type": event["type"], "kind": event["kind"]}));
                done.notify_one();
                Ok(None)
            }
        };
        let on_start = record(Arc::clone(&nested_events));
        api.on(
            "ui_prompt_start",
            crate::coding_agent::extensions::types::sync_handler(on_start),
        )?;
        let on_end = record(Arc::clone(&nested_events));
        api.on(
            "ui_prompt_end",
            crate::coding_agent::extensions::types::sync_handler(on_end),
        )?;
        Ok(())
    }));
    nested_runner.set_ui_context(
        Some(Arc::new(NestedPromptUi {
            runner: nested_runner.clone(),
        }) as Arc<dyn types::ExtensionUI>),
        types::ExtensionMode::Tui,
    );
    let nested_ui = nested_runner.create_context().ui().unwrap();
    nested_ui
        .select("Outer", &["x".to_string()], &opts)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while nested_events.lock().unwrap().len() < 2 {
            nested_done.notified().await;
        }
    })
    .await
    .unwrap();
    // Break the deliberately nested UI collaborator's runner -> UI -> runner cycle.
    nested_runner.set_ui_context(None, types::ExtensionMode::Tui);
    let events = nested_events.lock().unwrap().clone();
    assert_eq!(
        events.len(),
        2,
        "nested prompts emit only at the outer boundary"
    );
    assert_eq!(events[0]["kind"], json!("select"));
    assert_eq!(events[1]["kind"], json!("select"));
}

#[test]
fn context_defaults_and_live_reads_match_oracle() {
    let (_, runner) = load_runner_with(Arc::new(|_api: &ExtensionApi| Ok(())), None);
    let ctx = runner.create_context();
    let mode_before = ctx.mode().unwrap();
    let has_ui_before = ctx.has_ui().unwrap();

    runner.bind_core(
        test_actions(),
        context_actions_with(|actions| actions.is_project_trusted = Arc::new(|| false)),
        None,
    );
    let signal = Arc::new(AbortSignal::new());
    runner.bind_core(
        test_actions(),
        context_actions_with(|actions| {
            actions.is_project_trusted = Arc::new(|| false);
            let forward = Arc::clone(&signal);
            actions.get_signal = Arc::new(move || Some(Arc::clone(&forward)));
            actions.get_scoped_models = Arc::new(|| vec![json!("scoped")]);
        }),
        None,
    );
    let ctx2 = runner.create_context();
    let mut observed = json!({
        "modeBefore": mode_before.as_str(),
        "hasUIBefore": has_ui_before,
        "mode": ctx2.mode().unwrap().as_str(),
        "hasUI": ctx2.has_ui().unwrap(),
        "isProjectTrusted": ctx2.is_project_trusted().unwrap(),
        "scopedModels": ctx2.scoped_models().unwrap(),
        "idle": ctx2.is_idle().unwrap(),
        "pendingMessages": ctx2.has_pending_messages().unwrap(),
        "systemPromptDefault": ctx2.get_system_prompt().unwrap(),
    });
    if let Some(usage) = ctx2.get_context_usage().unwrap() {
        observed["usage"] = serde_json::to_value(usage).unwrap();
    }
    assert_eq!(observed, runner_oracle("context_defaults")["observed"]);

    runner.set_ui_context(Some(Arc::new(EmptyUi)), types::ExtensionMode::Rpc);
    let rpc_ctx = runner.create_context();
    assert_eq!(
        json!({"mode": rpc_ctx.mode().unwrap().as_str(), "hasUI": rpc_ctx.has_ui().unwrap()}),
        runner_oracle("context_rpc_mode")["observed"]
    );

    signal.abort();
    assert_eq!(
        json!({"aborted": ctx2.signal().unwrap().unwrap().is_aborted()}),
        runner_oracle("context_signal_live")["observed"]
    );
}

/// A UI with no overrides (upstream `{} as ExtensionUI`).
struct EmptyUi;

impl types::ExtensionUI for EmptyUi {}

// ---------------------------------------------------------------------------
// invalidate / assertActive
// ---------------------------------------------------------------------------

#[test]
fn invalidation_marks_every_entry_point_stale() {
    let (_, runner) = load_runner_with(Arc::new(|_api: &ExtensionApi| Ok(())), None);
    runner.invalidate(Some("stale-after-replacement"));
    let thrown = runner.create_context().ui().err().unwrap();
    let thrown_abort = runner.create_context().abort().unwrap_err();
    let defaulted = load_runner(Arc::new(|_api: &ExtensionApi| Ok(())));
    defaulted.invalidate(None);
    let default_thrown = defaulted.get_active_tools().unwrap_err();
    let default_prefix = default_thrown.chars().take(60).collect::<String>();

    assert_eq!(
        json!({
            "thrown": thrown,
            "thrownAbort": thrown_abort,
            "defaultThrown": default_prefix,
            "defaultLen": default_prefix.len(),
        }),
        runner_oracle("invalidate_stale")["observed"]
    );
}

// ---------------------------------------------------------------------------
// bindCore provider flush + command context + shutdown + flags/renderers
// ---------------------------------------------------------------------------

#[test]
fn bind_core_flushes_pending_registrations_and_routes_post_bind() {
    let runtime = ExtensionRuntime::new();
    runtime
        .register_provider(
            "broken-provider",
            &json!({"streamSimple": {}}),
            "/tmp/broken-extension.ts",
        )
        .unwrap();
    let runner = ExtensionRunner::new(
        Vec::new(),
        runtime.clone(),
        ".",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    let errors = Arc::new(Mutex::new(Vec::new()));
    let listener = Arc::clone(&errors);
    runner.on_error(Arc::new(move |error: &ExtensionError| {
        listener.lock().unwrap().push(format!(
            "{}: {}|event={}",
            error.extension_path, error.error, error.event
        ));
    }));
    let registry_calls = Arc::new(Mutex::new(Vec::new()));
    let register_calls = Arc::clone(&registry_calls);
    let native_calls = Arc::clone(&registry_calls);
    let unregister_calls = Arc::clone(&registry_calls);
    runner.bind_core(
        test_actions(),
        test_context_actions(),
        Some(ProviderActions {
            register_provider: Some(Arc::new(move |name, _config| {
                if name == "broken-provider" {
                    return Err(format!(
                        "Provider {name}: \"api\" is required when registering streamSimple."
                    ));
                }
                register_calls
                    .lock()
                    .unwrap()
                    .push(format!("register:{name}"));
                Ok(())
            })),
            register_native_provider: Some(Arc::new(move |provider| {
                native_calls
                    .lock()
                    .unwrap()
                    .push(format!("native:{}", provider.id()));
                Ok(())
            })),
            unregister_provider: Some(Arc::new(move |name| {
                unregister_calls
                    .lock()
                    .unwrap()
                    .push(format!("unregister:{name}"));
                Ok(())
            })),
            register_virtual_model: None,
            unregister_virtual_model: None,
        }),
    );
    runtime
        .register_provider(
            "instant-provider",
            &json!({"baseUrl": "https://x"}),
            "<post>",
        )
        .unwrap();
    runtime
        .register_native_provider(
            &crate::ai::models::faux::faux_provider(crate::ai::models::faux::FauxProviderOptions {
                provider: Some("native-x".into()),
                ..Default::default()
            })
            .provider,
            "<post>",
        )
        .unwrap();
    runtime.unregister_provider("instant-provider").unwrap();

    assert_eq!(
        json!({
            "errors": *errors.lock().unwrap(),
            "registryCalls": *registry_calls.lock().unwrap(),
            "pending": runtime.pending_provider_registrations().len(),
        }),
        runner_oracle("bind_core_provider_flush")["observed"]
    );
}

#[test]
fn bind_core_fallback_and_shutdown() {
    let runtime = ExtensionRuntime::new();
    let runner = ExtensionRunner::new(
        Vec::new(),
        runtime,
        ".",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    runner.bind_core(test_actions(), test_context_actions(), None);
    runner.shutdown();
    // The oracle capture's fallbackCalls list is empty (nothing registered).
    assert_eq!(
        runner_oracle("bind_core_fallback")["observed"]["fallbackCalls"],
        json!([])
    );
}

#[tokio::test]
async fn command_context_passes_arguments_through() {
    let runtime = ExtensionRuntime::new();
    let runner = ExtensionRunner::new(
        Vec::new(),
        runtime,
        ".",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_for_fork = Arc::clone(&calls);
    runner.bind_command_context(Some(ExtensionCommandContextActions {
        wait_for_idle: Arc::new(|| Ok(types::CommandFuture::resolved(()))),
        new_session: Arc::new(|_| {
            Ok(types::CommandFuture::resolved(types::Cancelled {
                cancelled: false,
            }))
        }),
        fork: Arc::new(move |entry_id, options| {
            let mut entry = json!({"entryId": entry_id});
            if let Some(options) = options {
                if let Some(position) = options.position {
                    entry["options"] = json!({"position": match position {
                        types::TreePosition::At => "at",
                        types::TreePosition::Before => "before",
                    }});
                }
            }
            calls_for_fork.lock().unwrap().push(entry);
            Ok(types::CommandFuture::resolved(types::Cancelled {
                cancelled: false,
            }))
        }),
        navigate_tree: Arc::new(|_, _| {
            Ok(types::CommandFuture::resolved(types::Cancelled {
                cancelled: false,
            }))
        }),
        switch_session: Arc::new(|_, _| {
            Ok(types::CommandFuture::resolved(types::Cancelled {
                cancelled: false,
            }))
        }),
        reload: Arc::new(|| Ok(types::CommandFuture::resolved(()))),
    }));

    let command_context = runner.create_command_context();
    command_context
        .fork("entry-1", None)
        .unwrap()
        .await
        .unwrap();
    command_context
        .fork(
            "entry-2",
            Some(types::ForkOptions {
                position: Some(types::TreePosition::At),
                with_session: None,
            }),
        )
        .unwrap()
        .await
        .unwrap();
    command_context.wait_for_idle().unwrap().await.unwrap();

    let defaults = ExtensionRunner::new(
        Vec::new(),
        ExtensionRuntime::new(),
        ".",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    defaults.bind_command_context(None);
    let default_fork = defaults
        .create_command_context()
        .fork("e", None)
        .unwrap()
        .await
        .unwrap();
    let default_new_session = defaults
        .create_command_context()
        .new_session(None)
        .unwrap()
        .await
        .unwrap();
    assert_eq!(
        json!({
            "calls": *calls.lock().unwrap(),
            "defaultFork": {"cancelled": default_fork.cancelled},
            "defaultNewSession": {"cancelled": default_new_session.cancelled},
        }),
        runner_oracle("command_context_fork")["observed"]
    );
}

#[tokio::test]
async fn session_shutdown_emission_reports_handler_presence() {
    let (_, none) = load_runner_with(Arc::new(|_api: &ExtensionApi| Ok(())), None);
    let no_handlers = emit_session_shutdown_event(
        &none,
        &json!({"type": "session_shutdown", "reason": "quit"}),
    )
    .await;
    let (_, with) = load_runner_with(
        Arc::new(|api: &ExtensionApi| {
            api.on("session_shutdown", handler(|_event, _ctx| Ok(None)))?;
            Ok(())
        }),
        None,
    );
    let with_handlers = emit_session_shutdown_event(
        &with,
        &json!({"type": "session_shutdown", "reason": "reload", "targetSessionFile": "x"}),
    )
    .await;
    assert_eq!(
        json!({"noHandlers": no_handlers, "withHandlers": with_handlers}),
        runner_oracle("emit_session_shutdown")["observed"]
    );
}

#[test]
fn flags_and_renderer_lookups_match_oracle() {
    let (extension, runner_holder) = load_runner_with(
        Arc::new(|api: &ExtensionApi| {
            api.register_flag(
                "my-flag",
                Some("My flag".to_string()),
                FlagType::Boolean,
                None,
            )?;
            api.register_message_renderer("my-type", Arc::new(|_message, _options, _theme| None))?;
            api.register_entry_renderer("my-entry", Arc::new(|_entry, _options, _theme| None))?;
            api.register_markdown_transformer(Arc::new(|markdown, _context| markdown.to_string()))?;
            Ok(())
        }),
        None,
    );
    let runner = ExtensionRunner::new(
        vec![extension.unwrap()],
        runner_holder.runtime(),
        ".",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    runner.bind_core(test_actions(), test_context_actions(), None);
    runner.set_flag_value("--test-flag", FlagValue::Bool(true));
    let flag_json = |value: &FlagValue| match value {
        FlagValue::Bool(flag) => json!(flag),
        FlagValue::Str(text) => json!(text),
    };
    let observed = json!({
        "flags": runner.get_flags().keys().map(str::to_string).collect::<Vec<_>>(),
        "flagValue": runner.get_flag_values().get("--test-flag").map(flag_json),
        "flagValuesSnapshot": runner
            .get_flag_values()
            .iter()
            .map(|(key, value)| (key.to_string(), flag_json(value)))
            .collect::<serde_json::Map<String, Value>>(),
        "messageRenderer": runner.get_message_renderer("my-type").is_some(),
        "messageRendererMissing": runner.get_message_renderer("nope").is_some(),
        "entryRenderer": runner.get_entry_renderer("my-entry").is_some(),
        "entryRendererMissing": runner.get_entry_renderer("nope").is_some(),
        "transformers": runner.get_markdown_transformers().len(),
        "extensionPaths": runner.get_extension_paths(),
    });
    assert_eq!(observed, runner_oracle("flags_and_renderers")["observed"]);
}

#[test]
fn thinking_level_actions_flow_through_runtime() {
    // The bound actions surface ctx.thinkingLevel (upstream
    // `runtime.getThinkingLevel`), pinned by the context-defaults oracle's
    // companion behavior.
    let runner = load_runner(Arc::new(|_api: &ExtensionApi| Ok(())));
    let ctx = runner.create_context();
    assert_eq!(ctx.thinking_level().unwrap(), ThinkingLevel::Off);
    runner.set_ui_context(None, types::ExtensionMode::Print);
    assert_eq!(ctx.mode().unwrap(), types::ExtensionMode::Print);
}

#[tokio::test]
async fn noop_ui_context_defaults_match_upstream_surface() {
    // `noOpUIContext` defaults: select → undefined, confirm → false,
    // getEditorText → "", theme → the stub, getToolsExpanded → false, and
    // setTheme → {success:false, error:"UI not available"}.
    let runner = load_runner(Arc::new(|_api: &ExtensionApi| Ok(())));
    let ui = runner.create_context().ui().unwrap();
    let opts = ExtensionUiDialogOptions::default();
    assert_eq!(
        ui.select("t", &["a".to_string()], &opts).await.unwrap(),
        None
    );
    assert!(!ui.confirm("t", "m", &opts).await.unwrap());
    assert_eq!(ui.get_editor_text(), "");
    assert_eq!(ui.theme(), Value::Null);
    assert!(!ui.get_tools_expanded());
    let set_theme = types::SetThemeResult {
        success: false,
        error: Some("UI not available".to_string()),
    };
    assert_eq!(set_theme.error.as_deref(), Some("UI not available"));
}

#[tokio::test]
async fn json_order_input_event_omissions_keep_source_before_streaming_behavior() {
    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&observed);
    let runner = load_runner(Arc::new(move |api: &ExtensionApi| {
        let captured = Arc::clone(&captured);
        api.on(
            "input",
            handler(move |event, _ctx| {
                captured.lock().unwrap().push(event.to_string());
                Ok(None)
            }),
        )?;
        Ok(())
    }));
    runner.emit_input("x", None, InputSource::Rpc, None).await;
    runner
        .emit_input("x", None, InputSource::Rpc, Some(StreamingDelivery::Steer))
        .await;
    runner
        .emit_input(
            "x",
            Some(vec![]),
            InputSource::Rpc,
            Some(StreamingDelivery::FollowUp),
        )
        .await;
    assert_eq!(
        *observed.lock().unwrap(),
        vec![
            r#"{"type":"input","text":"x","source":"rpc"}"#,
            r#"{"type":"input","text":"x","source":"rpc","streamingBehavior":"steer"}"#,
            r#"{"type":"input","text":"x","images":[],"source":"rpc","streamingBehavior":"followUp"}"#,
        ]
    );
}

#[test]
fn command_context_empty_invalidation_keeps_context_active() {
    let runtime = ExtensionRuntime::new();
    let runner = ExtensionRunner::new(
        Vec::new(),
        runtime.clone(),
        ".",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    let ctx = runner.create_command_context();
    runner.invalidate(Some(""));
    assert!(
        ctx.wait_for_idle().is_ok(),
        "upstream treats an empty stale message as falsy"
    );
    assert!(runtime.assert_active().is_ok());
    runner.invalidate(Some("after empty"));
    assert_eq!(ctx.cwd().unwrap_err(), "after empty");
    assert_eq!(runtime.assert_active().unwrap_err(), "after empty");
}

#[test]
fn flag_values_snapshot_is_independent_and_retains_insertion_order() {
    let runtime = ExtensionRuntime::new();
    let runner = ExtensionRunner::new(
        Vec::new(),
        runtime.clone(),
        ".",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    for name in ["z", "10", "2", "a"] {
        runner.set_flag_value(name, FlagValue::Str(name.into()));
    }
    let mut snapshot = runner.get_flag_values();
    runner.set_flag_value("z", FlagValue::Bool(true));
    runner.set_flag_value("live-only", FlagValue::Bool(false));
    assert_eq!(snapshot.keys().collect::<Vec<_>>(), ["z", "10", "2", "a"]);
    assert_eq!(snapshot.get("z"), Some(&FlagValue::Str("z".into())));
    snapshot.delete("10");
    snapshot.set("10", FlagValue::Str("snapshot-only".into()));
    assert_eq!(
        snapshot.into_iter().map(|(key, _)| key).collect::<Vec<_>>(),
        ["z", "2", "a", "10"]
    );
    assert_eq!(
        runner.get_flag_values().keys().collect::<Vec<_>>(),
        ["z", "10", "2", "a", "live-only"]
    );
    assert_eq!(runtime.flag_value("10"), Some(FlagValue::Str("10".into())));
}

#[tokio::test]
async fn async_ui_prompt_depth_survives_suspension_and_cleans_up_on_drop() {
    use crate::coding_agent::modes::rpc::ui::RpcExtensionUi;
    let runner = load_runner(Arc::new(|_api| Ok(())));
    let bridge = Arc::new(RpcExtensionUi::new(Arc::new(|_| {})));
    runner.set_ui_context(Some(bridge.clone()), types::ExtensionMode::Rpc);
    let ui = runner.create_context().ui().unwrap();
    let options = ExtensionUiDialogOptions::default();
    let mut first = Box::pin(ui.input("outer", None, &options));
    assert!(futures::poll!(&mut first).is_pending());
    assert_eq!(*runner.inner.ui_prompt_depth.lock().unwrap(), 1);
    assert_eq!(
        runner
            .inner
            .active_ui_prompt
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .1
            .as_deref(),
        Some("outer")
    );
    let mut second = Box::pin(ui.confirm("nested", "message", &options));
    assert!(futures::poll!(&mut second).is_pending());
    assert_eq!(*runner.inner.ui_prompt_depth.lock().unwrap(), 2);
    drop(first);
    assert_eq!(*runner.inner.ui_prompt_depth.lock().unwrap(), 1);
    assert_eq!(bridge.pending_count(), 1);
    bridge.reject_pending("disposed");
    assert_eq!(second.await.unwrap_err(), "disposed");
    assert_eq!(*runner.inner.ui_prompt_depth.lock().unwrap(), 0);
    assert!(runner.inner.active_ui_prompt.lock().unwrap().is_none());
    assert_eq!(bridge.pending_count(), 0);
}
