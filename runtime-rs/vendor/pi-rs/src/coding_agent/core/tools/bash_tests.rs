use super::super::{
    bash_process::{resolve_timeout_ms, ShellExit},
    powershell::{create_powershell_tool_definition, PowerShellToolOptions},
};
use super::*;
fn fixture() -> Value {
    serde_json::from_str(include_str!("bash_oracle.json")).unwrap()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn decode_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn normalize(value: &mut Value, spill: &str) {
    match value {
        Value::String(s) => *s = s.replace(spill, "$SPILL"),
        Value::Array(items) => {
            for item in items {
                normalize(item, spill);
            }
        }
        Value::Object(items) => {
            for item in items.values_mut() {
                normalize(item, spill);
            }
        }
        _ => {}
    }
}
fn metadata(def: &ToolDefinition) -> Value {
    let mut value = json!({"name":def.name,"label":def.label,"description":def.description,"parameters":def.parameters});
    for (key, value_opt) in [
        (
            "promptSnippet",
            def.prompt_snippet.as_ref().map(|s| json!(s)),
        ),
        (
            "promptGuidelines",
            def.prompt_guidelines.as_ref().map(|s| json!(s)),
        ),
        ("constrainedSampling", def.constrained_sampling.clone()),
    ] {
        if let Some(v) = value_opt {
            value[key] = v;
        }
    }
    value
}
#[test]
fn shell_metadata_and_timeout_validation_match_upstream() {
    let fixture = fixture();
    let defs = [
        create_bash_tool_definition("$CWD", Default::default()),
        create_powershell_tool_definition("$CWD", PowerShellToolOptions::default()),
        create_bash_tool_definition(
            "$CWD",
            BashToolOptions {
                expose_session_environment: Some(false),
                ..Default::default()
            },
        ),
    ];
    assert_eq!(
        json!(defs.iter().map(|d| metadata(d)).collect::<Vec<_>>()),
        fixture["metadata"]
    );
    for case in fixture["timeoutCases"].as_array().unwrap() {
        let value = case["input"]
            .as_str()
            .map(|s| s.parse::<f64>().unwrap())
            .or_else(|| case["input"].as_f64());
        let result = match resolve_timeout_ms(value) {
            Ok(v) => {
                json!({"value":v.map(|n| serde_json::from_str::<Value>(&crate::serde_support::js_number_string(n)).unwrap())})
            }
            Err(error) => json!({"error":error}),
        };
        assert_eq!(result, case["outcome"], "{}", case["input"]);
    }
}
#[tokio::test(start_paused = true)]
async fn shell_execution_matches_all_upstream_results_updates_env_and_spill_bytes() {
    for case in fixture()["cases"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let updates = Arc::new(Mutex::new(Vec::<Value>::new()));
        let ops = ShellOperations {
            exec: {
                let case = case.clone();
                let calls = calls.clone();
                Arc::new(move |command, cwd, options| {
                    let case = case.clone();
                    let mut call = json!({"command":command,"cwd":cwd,"env":options.env});
                    if let Some(timeout) = options.timeout {
                        call["timeout"] =
                            serde_json::from_str(&crate::serde_support::js_number_string(timeout))
                                .unwrap();
                    }
                    if let Some(signal) = &options.signal {
                        call["aborted"] = json!(signal.is_aborted());
                    }
                    calls.lock().unwrap().push(call);
                    Box::pin(async move {
                        let start = Instant::now();
                        for chunk in case["chunks"].as_array().unwrap() {
                            let at = chunk["at"].as_u64().unwrap_or(0);
                            if at > 0 {
                                tokio::time::sleep_until(start + Duration::from_millis(at)).await;
                            }
                            let bytes = if let Some(s) = chunk["hex"].as_str() {
                                decode_hex(s)
                            } else {
                                chunk["text"]
                                    .as_str()
                                    .unwrap_or("")
                                    .repeat(chunk["repeat"].as_u64().unwrap_or(1) as usize)
                                    .into_bytes()
                            };
                            (options.on_data)(&bytes)?;
                        }
                        if let Some(at) = case["endAt"].as_u64() {
                            tokio::time::sleep_until(start + Duration::from_millis(at)).await;
                        }
                        if let Some(error) = case["error"].as_str() {
                            return Err(error.into());
                        }
                        Ok(ShellExit {
                            exit_code: case
                                .get("exitCode")
                                .map(|v| v.as_i64().map(|v| v as i32))
                                .unwrap_or(Some(0)),
                        })
                    })
                })
            },
        };
        let hook: Option<BashSpawnHook> = case["hook"].as_str().map(|kind| {
            let kind = kind.to_owned();
            Arc::new(move |mut context: BashSpawnContext| {
                if kind == "error" {
                    return Err("hook failed".into());
                }
                context.command += "\nhook";
                context.cwd = "$HOOK-CWD".into();
                context.env.push(("HOOK".into(), "yes".into()));
                Ok(context)
            }) as BashSpawnHook
        });
        let options = BashToolOptions {
            command_prefix: case["commandPrefix"].as_str().map(str::to_owned),
            expose_session_environment: case["exposeSessionEnvironment"].as_bool(),
            spawn_hook: hook,
            ..Default::default()
        };
        let meta = case.get("context").map(|ctx| ShellSessionMetadata {
            session_id: ctx["sessionId"].as_str().unwrap().into(),
            session_file: ctx["sessionFile"].as_str().map(str::to_owned),
            provider: ctx["model"]["provider"].as_str().map(str::to_owned),
            model: ctx["model"]["id"].as_str().map(str::to_owned),
            reasoning_level: ctx["thinkingLevel"].as_str().map(str::to_owned),
        });
        let cwd = case["context"]["cwd"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("$CWD");
        let env = serde_json::from_value(
            case.get("env")
                .cloned()
                .unwrap_or(json!([["PATH", "base"]])),
        )
        .unwrap();
        let context = resolve_spawn_context("command", cwd, &options, env, meta.as_ref());
        let update: Option<AgentToolUpdateCallbackValue> = if case["emitUpdates"] == false {
            None
        } else {
            let updates = updates.clone();
            Some(Arc::new(move |value| {
                updates.lock().unwrap().push(value.clone())
            }))
        };
        let output = OutputAccumulator::with_temp_directory(Default::default(), directory.path());
        let result = match context {
            Ok(context) => {
                execute_shell_tool(
                    context,
                    case["timeout"].as_f64(),
                    &ops,
                    output,
                    None,
                    update,
                )
                .await
            }
            Err(e) => Err(e),
        };
        let mut outcome = match result {
            Ok(value) => json!({"value":value}),
            Err(error) => json!({"error":error}),
        };
        let files = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        assert!(files.len() <= 1);
        let raw = files.first().map(|p| hex(&std::fs::read(p).unwrap()));
        let mut updates = json!(*updates.lock().unwrap());
        if let Some(file) = files.first() {
            normalize(&mut outcome, file.to_str().unwrap());
            normalize(&mut updates, file.to_str().unwrap());
        }
        assert_eq!(
            json!(*calls.lock().unwrap()),
            case["calls"],
            "{} calls",
            case["id"]
        );
        assert_eq!(outcome, case["outcome"], "{} outcome", case["id"]);
        assert_eq!(updates, case["updates"], "{} updates", case["id"]);
        assert_eq!(json!(raw), case["fullOutputHex"], "{} spill", case["id"]);
    }
}
#[tokio::test(start_paused = true)]
async fn shell_finish_rejects_late_data_and_drop_cancels_pending_updates() {
    let late = Arc::new(Mutex::new(None));
    let updates = Arc::new(Mutex::new(vec![]));
    let ops = ShellOperations {
        exec: {
            let late = late.clone();
            Arc::new(move |_, _, options| {
                *late.lock().unwrap() = Some(options.on_data.clone());
                Box::pin(async move {
                    (options.on_data)(b"a")?;
                    (options.on_data)(b"b")?;
                    Ok(ShellExit { exit_code: Some(0) })
                })
            })
        },
    };
    let context = BashSpawnContext {
        command: "c".into(),
        cwd: "cwd".into(),
        env: vec![],
    };
    let update: AgentToolUpdateCallbackValue = {
        let updates = updates.clone();
        Arc::new(move |v| updates.lock().unwrap().push(v.clone()))
    };
    let result = execute_shell_tool(
        context.clone(),
        None,
        &ops,
        OutputAccumulator::default(),
        None,
        Some(update.clone()),
    )
    .await
    .unwrap();
    assert_eq!(result["content"][0]["text"], "ab");
    let before = updates.lock().unwrap().clone();
    late.lock().unwrap().as_ref().unwrap()(b"late").unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(*updates.lock().unwrap(), before);
    let dropped_sink = Arc::new(Mutex::new(None));
    let pending = ShellOperations {
        exec: {
            let dropped_sink = dropped_sink.clone();
            Arc::new(move |_, _, options| {
                *dropped_sink.lock().unwrap() = Some(options.on_data.clone());
                Box::pin(async move {
                    (options.on_data)(&vec![b'a'; 60_000])?;
                    (options.on_data)(b"b")?;
                    std::future::pending().await
                })
            })
        },
    };
    let directory = tempfile::tempdir().unwrap();
    let execution = execute_shell_tool(
        context,
        None,
        &pending,
        OutputAccumulator::with_temp_directory(Default::default(), directory.path()),
        None,
        Some(update),
    );
    let result = tokio::time::timeout(Duration::from_millis(20), execution).await;
    assert!(result.is_err());
    let before = updates.lock().unwrap().clone();
    let spill = std::fs::read_dir(directory.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let bytes = std::fs::read(&spill).unwrap();
    assert_eq!(bytes.len(), 60_001);
    dropped_sink.lock().unwrap().as_ref().unwrap()(b"ignored-after-drop").unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(*updates.lock().unwrap(), before);
    assert_eq!(std::fs::read(&spill).unwrap(), bytes);
}
