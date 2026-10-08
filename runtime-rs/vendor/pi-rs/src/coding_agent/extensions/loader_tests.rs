//! Tests for the ported `coding-agent/src/core/extensions/loader.ts`.
//!
//! Sources of truth:
//! - upstream `test/extensions-discovery.test.ts` (adapted to the module
//!   loader seam — see module docs; the jiti node_modules resolution test is
//!   seam-owned and cannot exist here),
//! - the loader oracle capture (`tests/fixtures/ext_oracle/loader.oracle.json`,
//!   generator `oracle_loader.mjs`): byte comparisons against the verbatim
//!   upstream sources under node, machine paths relativized to `<root>`.

use crate::coding_agent::extensions::{loader, types};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::coding_agent::extensions::loader::{
    clear_extension_cache, discover_and_load_extensions, load_extension_from_factory,
    load_extensions, load_extensions_cached, read_pi_manifest, ExecResult, ExtensionApi,
    ExtensionFactory, ExtensionModuleLoader, ExtensionRuntime,
};
use crate::coding_agent::extensions::oracle_data::{LOADER, RUNNER};
use crate::coding_agent::extensions::types::{
    CommandHandler, FlagType, FlagValue, HandlerResult, SourceInfo, ToolDefinition,
};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Test module loader: paths resolve to pre-registered factories (the jiti
/// seam).
#[derive(Default)]
struct TestLoader {
    modules: Mutex<HashMap<String, Option<ExtensionFactory>>>,
}

impl TestLoader {
    fn register(&self, path: &str, factory: ExtensionFactory) {
        self.modules
            .lock()
            .unwrap()
            .insert(path.to_string(), Some(factory));
    }

    /// Register a module whose default export is not a function.
    fn register_non_factory(&self, path: &str) {
        self.modules.lock().unwrap().insert(path.to_string(), None);
    }
}

impl ExtensionModuleLoader for TestLoader {
    fn load(&self, resolved_path: &str) -> Result<Option<ExtensionFactory>, String> {
        match self.modules.lock().unwrap().get(resolved_path) {
            Some(factory) => Ok(factory.clone()),
            None => Err(format!("Cannot find module '{resolved_path}'")),
        }
    }
}

fn noop_command_handler() -> CommandHandler {
    Arc::new(|_args: &str, _ctx: &types::ExtensionCommandContext| Ok(None))
}

fn command_factory() -> ExtensionFactory {
    Arc::new(|api: &ExtensionApi| api.register_command("test", None, noop_command_handler()))
}

fn temp_root(tag: &str) -> String {
    let base = std::env::temp_dir().join(format!("pi-ext-rust-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    base.to_string_lossy().to_string()
}

/// Replace the fixture root with `<root>` (both separator flavors), matching
/// the oracle's `deepRel`.
/// Like [`rel_root`], but also converts separators to forward slashes (the
/// oracle's discovery scenarios relativize through `rel()`, which normalizes
/// separators).
fn rel_root_fwd(root: &str, value: &Value) -> Value {
    fn walk(value: &Value, root: &str) -> Value {
        match value {
            Value::String(text) => {
                // Mimic the oracle's `rel()`: strip the fixture root (with its
                // trailing separator), then normalize separators.
                let text = text
                    .replacen(root, "", 1)
                    .trim_start_matches(['/', '\\'])
                    .to_string();
                Value::String(text.replace('\\', "/"))
            }
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| walk(item, root)).collect())
            }
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| (key.clone(), walk(item, root)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    walk(value, root)
}

fn rel_root(root: &str, value: &Value) -> Value {
    let root_fwd = root.replace('\\', "/");
    fn walk(value: &Value, root: &str, root_fwd: &str) -> Value {
        match value {
            Value::String(text) => {
                let text = text.split(root).collect::<Vec<_>>().join("<root>");
                let text = text.split(root_fwd).collect::<Vec<_>>().join("<root>");
                Value::String(text)
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| walk(item, root, root_fwd))
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| (key.clone(), walk(item, root, root_fwd)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    walk(value, root, &root_fwd)
}

fn oracle_scenario(file: &str, name: &str) -> Value {
    let raw = if file == "loader" { LOADER } else { RUNNER };
    let parsed: Value = serde_json::from_str(raw).expect("oracle json parses");
    parsed["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scenario| scenario["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("scenario {name} missing from {file} oracle"))
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

// ---------------------------------------------------------------------------
// Runtime stubs (oracle: runtime_stubs_throw)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runtime_stubs_throw_with_upstream_messages() {
    let runtime = ExtensionRuntime::new();
    let mut observed: Vec<String> = Vec::new();
    let mut check = |name: &str, result: Result<(), String>| match result {
        Ok(()) => observed.push(format!("{name}: no-throw")),
        Err(error) => observed.push(format!("{name}: {error}")),
    };
    check(
        "sendMessage",
        runtime
            .send_message(&json!({}), &Default::default())
            .map(|_| ()),
    );
    check("appendEntry", runtime.append_entry("t", None));
    check("setSessionName", runtime.set_session_name("x"));
    check("getSessionName", runtime.get_session_name().map(|_| ()));
    check("setLabel", runtime.set_label("e", None));
    check("getActiveTools", runtime.get_active_tools().map(|_| ()));
    check("getAllTools", runtime.get_all_tools().map(|_| ()));
    check("setActiveTools", runtime.set_active_tools(&[]));
    check("getCommands", runtime.get_commands().map(|_| ()));
    check("getThinkingLevel", runtime.get_thinking_level().map(|_| ()));
    check(
        "setThinkingLevel",
        runtime.set_thinking_level(types::ThinkingLevel::Off),
    );
    let set_model_rejection = match runtime.set_model(&json!({"id": "m"})).unwrap().await {
        Ok(_) => "resolved".to_string(),
        Err(error) => format!("rejected: {error}"),
    };
    runtime.refresh_tools();
    observed.push("refreshTools: ok".to_string());
    observed.push(format!("setModel: {set_model_rejection}"));
    let flag_json = |value: &FlagValue| match value {
        FlagValue::Bool(flag) => json!(flag),
        FlagValue::Str(text) => json!(text),
    };
    let flag_map: serde_json::Map<String, Value> = runtime
        .flag_values()
        .iter()
        .map(|(key, value)| (key.to_string(), flag_json(value)))
        .collect();
    observed.push(format!(
        "flagValues: {}",
        serde_json::to_string(&flag_map).unwrap()
    ));

    let expected = oracle_scenario("loader", "runtime_stubs_throw")["observed"].clone();
    assert_eq!(
        Value::Array(observed.into_iter().map(Value::String).collect()),
        expected
    );
}

#[test]
fn runtime_invalidate_default_message_is_pinned() {
    let runtime = ExtensionRuntime::new();
    runtime.invalidate(None);
    let first = runtime.assert_active().unwrap_err();
    runtime.invalidate(Some("second message"));
    let second = runtime.assert_active().unwrap_err();

    let expected =
        oracle_scenario("loader", "runtime_invalidate_default_message")["observed"].clone();
    assert_eq!(first, expected["first"].as_str().unwrap());
    assert_eq!(second, expected["second"].as_str().unwrap());
    assert!(first.len() > 100);
    assert_eq!(first, second, "first invalidation wins");
}

#[test]
fn runtime_tracks_event_bus_subscriptions_until_invalidation() {
    use crate::coding_agent::core::event_bus::EventBusController;
    let runtime = ExtensionRuntime::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let bus = EventBusController::new().bus().clone();
    let calls1 = Arc::clone(&calls);
    let tracked = runtime.track_event_bus_subscription(bus.on(
        "ch",
        Arc::new(move |_| calls1.lock().unwrap().push("tracked")),
    ));
    let calls2 = Arc::clone(&calls);
    let direct = runtime.track_event_bus_subscription(bus.on(
        "ch",
        Arc::new(move |_| calls2.lock().unwrap().push("direct")),
    ));
    runtime.invalidate(Some("stale"));
    tracked.unsubscribe();
    direct.unsubscribe();
    bus.emit("ch", &Value::Null);
    assert_eq!(*calls.lock().unwrap(), Vec::<&str>::new());

    let expected = oracle_scenario("loader", "runtime_track_event_bus")["observed"]
        ["callsAfterInvalidateAndUnsub"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(0, expected);
}

// ---------------------------------------------------------------------------
// Loader lifecycle (oracle scenarios)
// ---------------------------------------------------------------------------

fn load_result_errors(result: &types::LoadExtensionsResult, root: &str) -> Value {
    rel_root(
        root,
        &json!(result
            .errors
            .iter()
            .map(|error| json!({"path": error.path, "error": error.error}))
            .collect::<Vec<_>>()),
    )
}

#[test]
fn flag_default_mismatch_fails_the_load_with_upstream_text() {
    let root = temp_root("flag-mismatch");
    let path = format!("{}\\bad-flag-default.ts", root);
    let loader = TestLoader::default();
    loader.register(
        &path,
        Arc::new(|api: &ExtensionApi| {
            api.register_flag(
                "safe-mode",
                None,
                FlagType::Boolean,
                Some(FlagValue::Str("false".to_string())),
            )
        }),
    );
    let result = load_extensions(std::slice::from_ref(&path), &root, None, None, &loader);

    let expected = oracle_scenario("loader", "flag_default_mismatch")["observed"].clone();
    assert_eq!(load_result_errors(&result, &root), expected["errors"]);
    // flagValues stay empty (oracle: flag_default_mismatch_flagvalues).
    assert!(result.runtime.flag_values().is_empty());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn tool_parameter_schema_validation_errors_are_pinned() {
    let root = temp_root("tool-params");
    let cases: Vec<(&str, Value)> = vec![
        ("undefined", Value::Null),
        ("array", json!([])),
        ("null", Value::Null),
        ("string", json!("nope")),
    ];
    let mut observed = Vec::new();
    for (label, parameters) in cases {
        let path = format!("{root}\\missing-params-{label}.ts");
        let loader = TestLoader::default();
        loader.register(
            &path,
            Arc::new(move |api: &ExtensionApi| {
                let parameters = parameters.clone();
                let tool = ToolDefinition::new("noop", "No-op", "Do nothing", parameters);
                api.register_tool(tool)
            }),
        );
        let result = load_extensions(&[path], &root, None, None, &loader);
        observed.push(json!({
            "label": label,
            "errors": result.errors.iter().map(|error| json!({"path": error.path, "error": error.error})).collect::<Vec<_>>(),
        }));
    }
    let expected =
        oracle_scenario("loader", "tool_parameter_schema_validation")["observed"].clone();
    assert_eq!(rel_root(&root, &Value::Array(observed)), expected);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn flag_lifecycle_pending_then_commit() {
    let root = temp_root("flag-lifecycle");
    let runtime = ExtensionRuntime::new();
    let seen: Arc<Mutex<HashMap<String, Value>>> = Arc::new(Mutex::new(HashMap::new()));
    let seen_in_factory = Arc::clone(&seen);
    let extension = load_extension_from_factory(
        Arc::new(move |api: &ExtensionApi| {
            api.register_flag("with-default", None, FlagType::Boolean, Some(FlagValue::Bool(true)))?;
            api.register_flag(
                "string-default",
                None,
                FlagType::String,
                Some(FlagValue::Str("dflt".to_string())),
            )?;
            api.register_flag("no-default", None, FlagType::Boolean, None)?;
            let mut seen = seen_in_factory.lock().unwrap();
            seen.insert(
                "duringLoad".to_string(),
                json!({
                    "withDefault": api.get_flag("with-default")?.map(|value| value == FlagValue::Bool(true)),
                }),
            );
            seen.insert("flagValuesDuringLoad".to_string(), json!(runtime_snapshot(api)));
            Ok(())
        }),
        &root,
        crate::coding_agent::core::event_bus::EventBusController::new().bus().clone(),
        &runtime,
        None,
    )
    .unwrap();

    let during = seen.lock().unwrap().clone();
    let mut observed = json!({
        "duringLoad": during["duringLoad"],
        "flagValuesDuringLoad": during["flagValuesDuringLoad"],
    });
    let flag_json = |value: Option<FlagValue>| match value {
        Some(FlagValue::Bool(flag)) => json!(flag),
        Some(FlagValue::Str(text)) => json!(text),
        None => Value::Bool(false),
    };
    observed["afterCommit"] = json!({
        "withDefault": flag_json(runtime.flag_value("with-default")),
        "stringDefault": flag_json(runtime.flag_value("string-default")),
        // Upstream captures `runtime.flagValues.has("no-default")` — a
        // flag registered without a default never enters the map.
        "noDefault": runtime.flag_value("no-default").is_some(),
    });
    let _ = &extension;

    assert_eq!(
        observed,
        oracle_scenario("loader", "flag_lifecycle")["observed"],
        "flag lifecycle matches the oracle"
    );
    let _ = fs::remove_dir_all(&root);
}

fn runtime_snapshot(api: &ExtensionApi) -> Value {
    let values = api.runtime.flag_values();
    let mut map = serde_json::Map::new();
    for (key, value) in values {
        map.insert(
            key,
            match value {
                FlagValue::Bool(flag) => json!(flag),
                FlagValue::Str(text) => json!(text),
            },
        );
    }
    Value::Object(map)
}

#[test]
fn factory_throws_fail_the_load() {
    let root = temp_root("factory-throws");
    let loader = TestLoader::default();
    let path = format!("{root}\\throws.ts");
    loader.register(
        &path,
        Arc::new(|_api: &ExtensionApi| Err("Initialization failed!".to_string())),
    );
    let result = load_extensions(&[path], &root, None, None, &loader);
    let expected = oracle_scenario("loader", "factory_throws")["observed"].clone();
    assert_eq!(load_result_errors(&result, &root), expected["errors"]);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn non_factory_exports_are_reported() {
    let root = temp_root("non-factory");
    let loader = TestLoader::default();
    let path = format!("{root}\\no-default.ts");
    loader.register_non_factory(&path);
    let result = load_extensions(&[path], &root, None, None, &loader);
    let expected = oracle_scenario("loader", "non_factory_export")["observed"].clone();
    assert_eq!(load_result_errors(&result, &root), expected["errors"]);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn provider_queueing_unregister_and_discard_semantics() {
    use crate::coding_agent::core::event_bus::EventBusController;

    let root = temp_root("provider-queue");
    let runtime = ExtensionRuntime::new();
    let bus = EventBusController::new();
    let event_bus = bus.bus().clone();
    let bus_calls = Arc::new(Mutex::new(Vec::new()));

    let ok_calls = Arc::clone(&bus_calls);
    let ok_extension = load_extension_from_factory(
        Arc::new(move |api: &ExtensionApi| {
            api.register_provider("queued-provider", &json!({"baseUrl": "https://x.test"}))?;
            api.register_native_provider(
                &crate::ai::models::faux::faux_provider(
                    crate::ai::models::faux::FauxProviderOptions {
                        provider: Some("native-provider".into()),
                        ..Default::default()
                    },
                )
                .provider,
            )?;
            api.unregister_provider("queued-provider")?;
            api.register_provider("kept-provider", &json!({"baseUrl": "https://y.test"}))?;
            let calls = Arc::clone(&ok_calls);
            api.on_event(
                "bus-chan",
                Arc::new(move |_| calls.lock().unwrap().push("ok")),
            )?;
            Ok(())
        }),
        &root,
        event_bus.clone(),
        &runtime,
        None,
    )
    .unwrap();

    let committed = runtime
        .pending_provider_registrations()
        .iter()
        .map(|r| json!({"name": r.name, "extensionPath": rel_root(&root, &Value::String(r.extension_path.clone()))}))
        .collect::<Vec<_>>();
    let committed_native = runtime
        .pending_native_provider_registrations()
        .iter()
        .map(|r| json!({"id": r.provider.id(), "extensionPath": rel_root(&root, &Value::String(r.extension_path.clone()))}))
        .collect::<Vec<_>>();
    bus.bus().emit("bus-chan", &Value::Null);

    // discard path: a failed load unsubscribes its bus registrations.
    let fail_runtime = ExtensionRuntime::new();
    let fail_calls = Arc::clone(&bus_calls);
    let fail_result = load_extension_from_factory(
        Arc::new(move |api: &ExtensionApi| {
            api.register_provider("doomed-provider", &json!({"baseUrl": "https://z.test"}))?;
            let calls = Arc::clone(&fail_calls);
            api.on_event(
                "bus-chan",
                Arc::new(move |_| calls.lock().unwrap().push("doomed")),
            )?;
            Err("discard me".to_string())
        }),
        &root,
        event_bus.clone(),
        &fail_runtime,
        None,
    );
    let discard_rethrows = match fail_result {
        Ok(_) => String::from("no-throw"),
        Err(error) => error,
    };
    bus.bus().emit("bus-chan", &Value::Null);

    assert_eq!(ok_extension.path, "<inline>");
    let expected = oracle_scenario("loader", "provider_queueing")["observed"].clone();
    assert_eq!(
        json!({
            "committed": committed,
            "committedNative": committed_native,
            "discardRethrows": discard_rethrows,
            "busCalls": *bus_calls.lock().unwrap(),
        }),
        expected
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn failed_extension_api_becomes_inert_with_upstream_text() {
    let root = temp_root("inert");
    let runtime = ExtensionRuntime::new();
    let captured: Arc<Mutex<Option<ExtensionApi>>> = Arc::new(Mutex::new(None));
    let captured_in_factory = Arc::clone(&captured);
    let result = load_extension_from_factory(
        Arc::new(move |api: &ExtensionApi| {
            // The factory may keep a handle; a failed load must make it inert.
            *captured_in_factory.lock().unwrap() = Some(api.clone());
            Err("boom during load".to_string())
        }),
        &root,
        crate::coding_agent::core::event_bus::EventBusController::new()
            .bus()
            .clone(),
        &runtime,
        None,
    );
    assert!(result.is_err());

    let captured = captured.lock().unwrap();
    let api = captured.as_ref().unwrap();
    let expected_message = "Extension \"<inline>\" failed to load and its API is no longer active.";
    assert_eq!(
        api.register_command("x", None, noop_command_handler())
            .unwrap_err(),
        expected_message
    );
    assert_eq!(api.get_flag("x").unwrap_err(), expected_message);
    assert_eq!(
        api.send_message(&json!({"customType": "t"}), &Default::default())
            .unwrap_err(),
        expected_message
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn stale_runtime_error_text_through_captured_api() {
    let root = temp_root("stale");
    let runtime = ExtensionRuntime::new();
    let _ = load_extension_from_factory(
        Arc::new(|_api: &ExtensionApi| Ok(())),
        &root,
        crate::coding_agent::core::event_bus::EventBusController::new()
            .bus()
            .clone(),
        &runtime,
        None,
    )
    .unwrap();
    runtime.invalidate(Some("stale-after-replacement"));
    let error = runtime
        .send_message(&json!({"customType": "t"}), &Default::default())
        .unwrap_err();
    let expected = oracle_scenario("loader", "stale_extension_api")["observed"][0].clone();
    assert_eq!(Value::String(error), expected);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn synthetic_source_info_matches_oracle() {
    let root = temp_root("source-info");
    let runtime = ExtensionRuntime::new();
    let inline = load_extension_from_factory(
        Arc::new(|_api: &ExtensionApi| Ok(())),
        &root,
        crate::coding_agent::core::event_bus::EventBusController::new()
            .bus()
            .clone(),
        &runtime,
        None,
    )
    .unwrap();

    let loader = TestLoader::default();
    // The win32 capture registers a backslash-joined path (the host
    // separator); on POSIX the same synthetic input uses `/` — `\` is not a
    // separator there, so dirname would land outside the fixture root
    // (upstream node's `path.dirname` behaves identically).
    let local_path = if cfg!(windows) {
        format!("{root}\\sub\\local.ts")
    } else {
        format!("{root}/sub/local.ts")
    };
    loader.register(&local_path, Arc::new(|_api: &ExtensionApi| Ok(())));
    let via_load = load_extensions(&[local_path], &root, None, None, &loader);

    let observed = json!({
        "inline": source_info_value(&inline.source_info, &root),
        "local": source_info_value(&via_load.extensions[0].source_info, &root),
    });
    // environment-anchored: both sides normalized. The captured path/baseDir
    // carry win32 separators (`<root>\sub\local.ts`); the port renders host
    // separators, and upstream-on-linux would render POSIX ones. Unify
    // separators to `/` on both sides so the pin covers the path shape, not
    // the capture host's separator.
    let expected = oracle_scenario("loader", "source_info")["observed"].clone();
    let expected = rel_root_fwd(&root, &expected);
    let observed = rel_root_fwd(&root, &observed);
    assert_eq!(observed, expected);
    let _ = fs::remove_dir_all(&root);
}

fn source_info_value(info: &SourceInfo, root: &str) -> Value {
    let mut object = json!({
        "path": info.path,
        "source": info.source,
        "scope": match info.scope {
            types::SourceScope::User => "user",
            types::SourceScope::Project => "project",
            types::SourceScope::Temporary => "temporary",
        },
        "origin": match info.origin {
            types::SourceOrigin::Package => "package",
            types::SourceOrigin::TopLevel => "top-level",
        },
    });
    match &info.base_dir {
        Some(base_dir) => object["baseDir"] = Value::String(base_dir.clone()),
        None => {
            object["baseDir"] = Value::String("<undefined>".to_string());
        }
    }
    rel_root(root, &object)
}

// ---------------------------------------------------------------------------
// readPiManifest (oracle battery)
// ---------------------------------------------------------------------------

#[test]
fn read_pi_manifest_battery_matches_oracle() {
    let root = temp_root("manifests");
    let cases: Vec<(&str, Value)> = vec![
        (
            "pi_extensions",
            json!({"name": "x", "pi": {"extensions": ["./a.ts", "b.js"], "skills": ["s"], "prompts": ["p"], "themes": ["t"]}}),
        ),
        ("no_pi_field", json!({"name": "x", "version": "1.0.0"})),
        ("pi_not_object", json!({"pi": "nope"})),
        (
            "non_string_entries",
            json!({"pi": {"extensions": ["ok", 42]}}),
        ),
        ("empty_entries", json!({"pi": {"extensions": []}})),
        ("non_array_entries", json!({"pi": {"extensions": "a.ts"}})),
    ];
    let mut observed = serde_json::Map::new();
    for (label, body) in &cases {
        let path = Path::new(&root).join(format!("manifest-{label}.json"));
        write_file(&path, &serde_json::to_string(body).unwrap());
        observed.insert(
            label.to_string(),
            manifest_value(read_pi_manifest(path.to_str().unwrap())),
        );
    }
    let bom_path = Path::new(&root).join("manifest-bom.json");
    write_file(
        &bom_path,
        &format!("\u{FEFF}{}", json!({"pi": {"extensions": ["bom.ts"]}})),
    );
    observed.insert(
        "bom".to_string(),
        manifest_value(read_pi_manifest(bom_path.to_str().unwrap())),
    );
    observed.insert(
        "missing_file".to_string(),
        manifest_value(read_pi_manifest(
            Path::new(&root)
                .join("manifest-does-not-exist.json")
                .to_str()
                .unwrap(),
        )),
    );
    let bad_path = Path::new(&root).join("manifest-bad.json");
    write_file(&bad_path, "{not json");
    observed.insert(
        "bad_json".to_string(),
        manifest_value(read_pi_manifest(bad_path.to_str().unwrap())),
    );

    assert_eq!(
        Value::Object(observed),
        oracle_scenario("loader", "read_pi_manifest")["observed"]
    );
    let _ = fs::remove_dir_all(&root);
}

fn manifest_value(manifest: Option<loader::PiManifest>) -> Value {
    match manifest {
        None => Value::Null,
        Some(manifest) => {
            let mut object = serde_json::Map::new();
            if let Some(extensions) = manifest.extensions {
                object.insert("extensions".to_string(), json!(extensions));
            }
            if let Some(skills) = manifest.skills {
                object.insert("skills".to_string(), json!(skills));
            }
            if let Some(prompts) = manifest.prompts {
                object.insert("prompts".to_string(), json!(prompts));
            }
            if let Some(themes) = manifest.themes {
                object.insert("themes".to_string(), json!(themes));
            }
            Value::Object(object)
        }
    }
}

// ---------------------------------------------------------------------------
// Discovery (oracle battery + pipeline)
// ---------------------------------------------------------------------------

fn seed_discovery_fixtures(root: &str) -> (String, String) {
    let extension_code = r#"export default function(pi) { pi.registerCommand("test", { handler: async () => {} }); }"#;
    let write = |relative: &str, content: &str| {
        write_file(&Path::new(root).join(relative), content);
    };
    write("fixtures/mixed/extensions/direct.ts", extension_code);
    write("fixtures/mixed/extensions/foo.ts", extension_code);
    write("fixtures/mixed/extensions/bar.ts", extension_code);
    write(
        "fixtures/mixed/extensions/with-index/index.ts",
        extension_code,
    );
    write(
        "fixtures/mixed/extensions/with-index/index.js",
        extension_code,
    );
    write(
        "fixtures/mixed/extensions/with-manifest/package.json",
        r#"{"pi":{"extensions":["./entry.ts"]}}"#,
    );
    write(
        "fixtures/mixed/extensions/with-manifest/entry.ts",
        extension_code,
    );
    write(
        "fixtures/mixed/extensions/not-an-extension/helper.ts",
        extension_code,
    );
    write(
        "fixtures/mixed/extensions/container/nested/index.ts",
        extension_code,
    );

    write(
        "fixtures/manifests/extensions/precedence/index.ts",
        extension_code,
    );
    write(
        "fixtures/manifests/extensions/precedence/custom.ts",
        extension_code,
    );
    write(
        "fixtures/manifests/extensions/precedence/package.json",
        r#"{"name":"p","pi":{"extensions":["./custom.ts"]}}"#,
    );
    write(
        "fixtures/manifests/extensions/multi/package.json",
        r#"{"pi":{"extensions":["./ext1.ts","./ext2.ts"]}}"#,
    );
    write(
        "fixtures/manifests/extensions/multi/ext1.ts",
        extension_code,
    );
    write(
        "fixtures/manifests/extensions/multi/ext2.ts",
        extension_code,
    );
    write(
        "fixtures/manifests/extensions/no-pi-field/index.ts",
        extension_code,
    );
    write(
        "fixtures/manifests/extensions/no-pi-field/package.json",
        r#"{"version":"1.0.0"}"#,
    );
    write(
        "fixtures/manifests/extensions/skip-missing/package.json",
        r#"{"pi":{"extensions":["./exists.ts","./missing.ts"]}}"#,
    );
    write(
        "fixtures/manifests/extensions/skip-missing/exists.ts",
        extension_code,
    );
    write(
        "fixtures/manifests/extensions/tilde/package.json",
        r#"{"pi":{"extensions":["~entry.ts","~/entry.ts"]}}"#,
    );
    write(
        "fixtures/manifests/extensions/tilde/~entry.ts",
        extension_code,
    );
    write(
        "fixtures/manifests/extensions/tilde/~/entry.ts",
        extension_code,
    );

    (
        format!("{root}/fixtures/mixed/extensions"),
        format!("{root}/fixtures/manifests/extensions"),
    )
}

#[test]
fn discovery_battery_matches_oracle() {
    let root = temp_root("discovery");
    let (mixed_dir, manifests_dir) = seed_discovery_fixtures(&root);

    let mut observed = serde_json::Map::new();
    for (label, extensions_dir) in [
        ("mixed", mixed_dir.as_str()),
        ("manifests", manifests_dir.as_str()),
    ] {
        let loader = CatchAllLoader;
        let empty_cwd = Path::new(&root).join("empty-cwd");
        fs::create_dir_all(&empty_cwd).unwrap();
        let agent_dir = Path::new(&extensions_dir)
            .parent()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let result = discover_and_load_extensions(
            &[],
            &empty_cwd.to_string_lossy(),
            &agent_dir,
            None,
            &loader,
        );
        // environment-anchored: both sides sorted. Upstream discovers through
        // unsorted `fs.readdirSync`, so the raw order is the OS readdir order
        // and differs between the win32 capture machine and POSIX; sorting
        // both sides pins the discovered set instead of the host readdir
        // order.
        let mut paths: Vec<Value> = result
            .extensions
            .iter()
            .map(|extension| rel_root_fwd(&root, &Value::String(extension.path.clone())))
            .collect();
        paths.sort_by(|a, b| {
            a.as_str()
                .unwrap_or_default()
                .cmp(b.as_str().unwrap_or_default())
        });
        observed.insert(
            label.to_string(),
            json!({
                "paths": paths,
                "errors": result.errors.iter().map(|error| json!({"path": error.path, "error": error.error})).collect::<Vec<_>>(),
            }),
        );
    }

    // Same stated rule on the oracle side: sort each scenario's `paths`.
    let mut expected = rel_root_fwd(
        &root,
        &oracle_scenario("loader", "discovery_battery")["observed"].clone(),
    );
    if let Value::Object(entries) = &mut expected {
        for (_, scenario) in entries.iter_mut() {
            if let Some(paths) = scenario.get_mut("paths").and_then(Value::as_array_mut) {
                paths.sort_by(|a, b| {
                    a.as_str()
                        .unwrap_or_default()
                        .cmp(b.as_str().unwrap_or_default())
                });
            }
        }
    }
    assert_eq!(Value::Object(observed), expected);
    let _ = fs::remove_dir_all(&root);
}

/// A loader that resolves every path to a command-registering factory.
#[derive(Default)]
struct CatchAllLoader;

impl ExtensionModuleLoader for CatchAllLoader {
    fn load(&self, _resolved_path: &str) -> Result<Option<ExtensionFactory>, String> {
        Ok(Some(command_factory()))
    }
}

#[test]
fn discover_pipeline_matches_oracle() {
    let root = temp_root("pipeline");
    let extension_code = r#"export default function(pi) { pi.registerCommand("test", { handler: async () => {} }); }"#;
    let cwd = Path::new(&root).join("project");
    let agent_dir = Path::new(&root).join("agent2");
    write_file(&cwd.join(".pi/extensions/local.ts"), extension_code);
    write_file(&agent_dir.join("extensions/global.ts"), extension_code);
    write_file(&cwd.join("explicit.ts"), extension_code);
    write_file(&cwd.join("explicit-dir/index.ts"), extension_code);

    let configured = vec![
        cwd.join("explicit.ts").to_string_lossy().to_string(),
        cwd.join("explicit.ts").to_string_lossy().to_string(),
        cwd.join("explicit-dir").to_string_lossy().to_string(),
        "./relative-missing.ts".to_string(),
    ];
    // Upstream jiti resolves exactly the four real files; the missing
    // relative path fails resolution and lands in `errors`.
    let loader = TestLoader::default();
    for real in [
        cwd.join(".pi/extensions/local.ts"),
        agent_dir.join("extensions/global.ts"),
        cwd.join("explicit.ts"),
        cwd.join("explicit-dir/index.ts"),
    ] {
        // Discovery emits host-separator paths; normalize the mixed-separator
        // joins to match.
        let key = if cfg!(windows) {
            real.to_string_lossy().replace('/', "\\")
        } else {
            real.to_string_lossy().to_string()
        };
        loader.register(&key, command_factory());
    }
    let result = discover_and_load_extensions(
        &configured,
        &cwd.to_string_lossy(),
        &agent_dir.to_string_lossy(),
        None,
        &loader,
    );

    let observed = json!({
        "order": result.extensions.iter().map(|extension| rel_root_fwd(&root, &Value::String(extension.path.clone()))).collect::<Vec<_>>(),
        "errors": result.errors.iter().map(|error| rel_root_fwd(&root, &Value::String(error.path.clone()))).collect::<Vec<_>>(),
    });
    assert_eq!(
        observed,
        oracle_scenario("loader", "discover_pipeline")["observed"]
    );
    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Upstream discovery-suite behaviors over the seam
// ---------------------------------------------------------------------------

#[test]
fn discovers_direct_and_subdirectory_entries_like_upstream_suite() {
    let root = temp_root("suite-discovery");
    let extensions_dir = Path::new(&root).join("extensions");
    fs::create_dir_all(&extensions_dir).unwrap();
    let code = "export default function(pi) { pi.registerCommand(\"test\", { handler: async () => {} }); }";
    fs::write(extensions_dir.join("foo.ts"), code).unwrap();
    fs::write(extensions_dir.join("bar.ts"), code).unwrap();

    let result = discover_and_load_extensions(&[], &root, &root, None, &CatchAllLoader);
    assert!(result.errors.is_empty());
    assert_eq!(result.extensions.len(), 2);
    let mut names: Vec<String> = result
        .extensions
        .iter()
        .map(|extension| {
            Path::new(&extension.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["bar.ts", "foo.ts"]);
    // Commands registered through the factory.
    assert!(result
        .extensions
        .iter()
        .all(|extension| extension.commands.has("test")));

    // Subdirectory with index is discovered; helper-only subdirectories and
    // nested containers are not.
    fs::create_dir_all(extensions_dir.join("my-extension")).unwrap();
    fs::write(extensions_dir.join("my-extension/index.ts"), code).unwrap();
    fs::create_dir_all(extensions_dir.join("not-an-extension")).unwrap();
    fs::write(extensions_dir.join("not-an-extension/helper.ts"), code).unwrap();
    let result = discover_and_load_extensions(&[], &root, &root, None, &CatchAllLoader);
    assert!(result.errors.is_empty());
    assert!(result
        .extensions
        .iter()
        .any(|extension| extension.path.contains("my-extension")));
    assert!(!result
        .extensions
        .iter()
        .any(|extension| extension.path.contains("not-an-extension")));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn explicit_paths_skip_discovery() {
    let root = temp_root("suite-explicit");
    let extensions_dir = Path::new(&root).join("extensions");
    fs::create_dir_all(&extensions_dir).unwrap();
    let code = "export default function(pi) { pi.registerCommand(\"test\", { handler: async () => {} }); }";
    fs::write(extensions_dir.join("discovered.ts"), code).unwrap();
    let explicit_path = Path::new(&root).join("explicit.ts");
    fs::write(&explicit_path, code).unwrap();

    let result = load_extensions(
        &[explicit_path.to_string_lossy().to_string()],
        &root,
        None,
        None,
        &CatchAllLoader,
    );
    assert!(result.errors.is_empty());
    assert_eq!(result.extensions.len(), 1);
    assert!(result.extensions[0].path.contains("explicit.ts"));

    let empty = load_extensions(&[], &root, None, None, &CatchAllLoader);
    assert!(empty.extensions.is_empty());
    assert!(empty.errors.is_empty());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn factory_cache_is_validated_by_cwd_and_generation() {
    clear_extension_cache();
    let root = temp_root("cache");
    let loader = TestLoader::default();
    let path = format!("{root}\\cached.ts");
    let factory: ExtensionFactory = Arc::new(|_api: &ExtensionApi| Ok(()));
    loader.register(&path, Arc::clone(&factory));

    // First load populates the cache for this cwd.
    let first = load_extensions_cached(std::slice::from_ref(&path), &root, None, None, &loader);
    assert_eq!(first.extensions.len(), 1);
    // Second load with the same cwd hits the cache (loader not consulted).
    let second = load_extensions_cached(std::slice::from_ref(&path), &root, None, None, &loader);
    assert_eq!(second.extensions.len(), 1);
    // Uncached loads bypass the cache.
    let uncached = load_extensions(std::slice::from_ref(&path), &root, None, None, &loader);
    assert_eq!(uncached.extensions.len(), 1);
    // clear_extension_cache() bumps the generation.
    clear_extension_cache();
    let third = load_extensions_cached(&[path], &root, None, None, &loader);
    assert_eq!(third.extensions.len(), 1);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn exec_command_runs_and_reports_status() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result: ExecResult = runtime.block_on(async {
        let (command, args) = if cfg!(windows) {
            (
                "cmd.exe".to_string(),
                vec!["/C".to_string(), "echo".to_string(), "hello".to_string()],
            )
        } else {
            ("echo".to_string(), vec!["hello".to_string()])
        };
        loader::exec_command(
            &command,
            &args,
            std::env::temp_dir().to_str().unwrap(),
            None,
        )
        .await
    });
    assert_eq!(result.code, 0);
    assert!(result.stdout.contains("hello"));
    assert!(!result.killed);

    // Timeout kills the process.
    let killed: ExecResult = runtime.block_on(async {
        let (command, args) = if cfg!(windows) {
            (
                "cmd.exe".to_string(),
                vec![
                    "/C".to_string(),
                    "ping".to_string(),
                    "-n".to_string(),
                    "5".to_string(),
                    "127.0.0.1".to_string(),
                ],
            )
        } else {
            ("sleep".to_string(), vec!["5".to_string()])
        };
        loader::exec_command(
            &command,
            &args,
            std::env::temp_dir().to_str().unwrap(),
            Some(loader::ExecOptions {
                timeout: Some(150),
                ..Default::default()
            }),
        )
        .await
    });
    assert!(killed.killed);

    // Spawn failure resolves code 1 (upstream error path).
    let missing: ExecResult = runtime.block_on(async {
        loader::exec_command(
            "definitely-not-a-real-binary-9z",
            &[],
            std::env::temp_dir().to_str().unwrap(),
            None,
        )
        .await
    });
    assert_eq!(missing.code, 1);
}

// The runner oracle file is referenced so both captures are pinned from this
// module tree (runner comparisons live in runner_tests.rs).
#[test]
fn oracle_captures_are_loadable() {
    let loader: Value = serde_json::from_str(LOADER).unwrap();
    assert_eq!(loader["scenarios"].as_array().unwrap().len(), 16);
    let runner: Value = serde_json::from_str(RUNNER).unwrap();
    assert_eq!(runner["scenarios"].as_array().unwrap().len(), 45);
}

#[test]
fn handler_unsubscribe_removes_only_its_own_registration() {
    let root = temp_root("unsub");
    let runtime = ExtensionRuntime::new();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_a = Arc::clone(&calls);
    let unsubscribe_handle = Mutex::new(None::<loader::HandlerUnsubscribe>);
    let handle_slot = Arc::new(unsubscribe_handle);
    let handle_for_factory = Arc::clone(&handle_slot);
    let extension = load_extension_from_factory(
        Arc::new(move |api: &ExtensionApi| {
            let calls = Arc::clone(&calls_a);
            let unsubscribe = api.on(
                "agent_end",
                crate::coding_agent::extensions::types::sync_handler(move |_event, _ctx| {
                    calls.lock().unwrap().push("A".to_string());
                    Ok(None)
                }),
            )?;
            *handle_for_factory.lock().unwrap() = Some(unsubscribe);
            let calls_b = Arc::clone(&calls_a);
            api.on(
                "agent_end",
                crate::coding_agent::extensions::types::sync_handler(move |_event, _ctx| {
                    calls_b.lock().unwrap().push("B".to_string());
                    Ok(Some(HandlerResult::Json(Value::Null)))
                }),
            )?;
            Ok(())
        }),
        &root,
        crate::coding_agent::core::event_bus::EventBusController::new()
            .bus()
            .clone(),
        &runtime,
        None,
    )
    .unwrap();

    handle_slot.lock().unwrap().take().unwrap().unsubscribe();
    // B remains registered; A's handler is gone.
    assert_eq!(
        extension
            .handlers
            .get("agent_end")
            .map(|handlers| handlers.len()),
        Some(1)
    );
    let _ = fs::remove_dir_all(&root);
}

/// Pending defaults and the published runtime both have JS Map (not Object or
/// HashMap) order. A second declaration does not replace the first pending default.
#[test]
fn flag_defaults_commit_and_updates_preserve_js_map_order() {
    let runtime = ExtensionRuntime::new();
    runtime.set_flag_value("seed", FlagValue::Bool(false));
    load_extension_from_factory(
        Arc::new(|api: &ExtensionApi| {
            for name in ["tail", "10", "2", "alpha"] {
                api.register_flag(
                    name,
                    None,
                    FlagType::String,
                    Some(FlagValue::Str(name.into())),
                )?;
            }
            api.register_flag(
                "tail",
                None,
                FlagType::String,
                Some(FlagValue::Str("ignored".into())),
            )?;
            api.register_flag("seed", None, FlagType::Boolean, Some(FlagValue::Bool(true)))?;
            assert_eq!(api.get_flag("tail")?, Some(FlagValue::Str("tail".into())));
            assert_eq!(
                api.runtime.flag_values().keys().collect::<Vec<_>>(),
                ["seed"]
            );
            Ok(())
        }),
        ".",
        crate::coding_agent::core::event_bus::EventBusController::new()
            .bus()
            .clone(),
        &runtime,
        Some("<ordered-flags>"),
    )
    .unwrap();
    assert_eq!(
        runtime.flag_values().keys().collect::<Vec<_>>(),
        ["seed", "tail", "10", "2", "alpha"]
    );
    assert_eq!(runtime.flag_value("seed"), Some(FlagValue::Bool(false)));
    assert_eq!(
        runtime.flag_value("tail"),
        Some(FlagValue::Str("tail".into()))
    );
    runtime.set_flag_value("tail", FlagValue::Str("updated".into()));
    runtime.set_flag_value("late", FlagValue::Bool(true));
    assert_eq!(
        runtime
            .flag_values()
            .into_iter()
            .map(|(key, _)| key)
            .collect::<Vec<_>>(),
        ["seed", "tail", "10", "2", "alpha", "late"]
    );
    assert_eq!(
        runtime.flag_value("tail"),
        Some(FlagValue::Str("updated".into()))
    );
}
