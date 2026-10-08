//! Differential execution through real ToolDefinition adapters, plus cancellation.
use super::{find::*, grep::*, search_process::*};
use crate::coding_agent::extensions::{
    loader::ExtensionRuntime,
    runner::ExtensionRunner,
    types::{self, AbortSignal},
};
use futures::FutureExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
fn context(cwd: &str) -> types::ExtensionContext {
    ExtensionRunner::new(
        Vec::new(),
        ExtensionRuntime::new(),
        cwd,
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    )
    .create_context()
}
#[tokio::test]
async fn search_definitions_match_real_upstream_execution_and_effects() {
    let oracle: Value = serde_json::from_str(include_str!("search_tools_oracle.json")).unwrap();
    for kind in ["find", "grep"] {
        let definition = if kind == "find" {
            create_find_tool_definition("", Default::default())
        } else {
            create_grep_tool_definition("", Default::default())
        };
        assert_eq!(
            json!({"name":definition.name,"label":definition.label,"description":definition.description,"parameters":definition.parameters,"promptSnippet":definition.prompt_snippet}),
            oracle["metadata"][kind]
        );
    }
    for case in oracle["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["windows"] == cfg!(windows))
    {
        let trace = Arc::new(Mutex::new(vec![]));
        let ensure: EnsureSearchTool = {
            let trace = trace.clone();
            let missing = case["unavailable"] == true;
            Arc::new(move |tool| {
                trace.lock().unwrap().push(json!(["ensure", tool]));
                async move { Ok((!missing).then(|| tool.to_string())) }.boxed()
            })
        };
        let runner: SearchRunner = {
            let case = case.clone();
            let trace = trace.clone();
            Arc::new(move |command, on_line| {
                let trace = trace.clone();
                let case = case.clone();
                async move {
                    trace
                        .lock()
                        .unwrap()
                        .push(json!(["spawn", command.program, command.args]));
                    if let Some(error) = case["spawnError"].as_str() {
                        return Err(error.into());
                    }
                    let mut killed = false;
                    let items = if case["kind"] == "find" {
                        case["lines"].as_array()
                    } else {
                        case["events"].as_array()
                    };
                    for value in items.into_iter().flatten() {
                        let line = value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string());
                        if !on_line(line) {
                            killed = true;
                            trace.lock().unwrap().push(json!(["kill"]));
                            break;
                        }
                    }
                    let code = if killed || case.get("code") == Some(&Value::Null) {
                        None
                    } else {
                        Some(case["code"].as_i64().unwrap_or(0) as i32)
                    };
                    Ok(SearchExit {
                        code,
                        stderr: case["stderr"].as_str().unwrap_or("").into(),
                    })
                }
                .boxed()
            })
        };
        let root = case["root"].as_str().unwrap();
        let definition = if case["kind"] == "find" {
            let custom = if case["custom"] == true {
                let exists: Exists = {
                    let trace = trace.clone();
                    let result = case["exists"] != false;
                    Arc::new(move |path| {
                        trace.lock().unwrap().push(json!(["exists", path]));
                        async move { Ok(result) }.boxed()
                    })
                };
                let glob: Glob = {
                    let trace = trace.clone();
                    let case = case.clone();
                    Arc::new(move |pattern, cwd, options| {
                        trace.lock().unwrap().push(json!(["glob",pattern,cwd,{"ignore":options.ignore,"limit":serde_json::from_str::<Value>(&crate::serde_support::js_number_string(options.limit)).unwrap()}]));
                        let case = case.clone();
                        async move {
                            if let Some(error) = case["globError"].as_str() {
                                return Err(error.into());
                            }
                            Ok(case["results"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|s| s.as_str().unwrap().into())
                                .collect())
                        }
                        .boxed()
                    })
                };
                Some(FindOperations { exists, glob })
            } else {
                None
            };
            let path_exists: Exists = {
                let trace = trace.clone();
                let git = case["gitAt"]
                    .as_str()
                    .map(|s| crate::coding_agent::core::path_join(s, ".git"));
                Arc::new(move |path| {
                    trace.lock().unwrap().push(json!(["pathExists", path]));
                    let found = git.as_ref() == Some(&path);
                    async move { Ok(found) }.boxed()
                })
            };
            create_find_tool_definition(
                "fallback-must-not-be-used",
                FindToolOptions {
                    operations: custom,
                    ensure_tool: Some(ensure),
                    runner: Some(runner),
                    path_exists: Some(path_exists),
                },
            )
        } else {
            let is_directory = {
                let case = case.clone();
                let trace = trace.clone();
                Arc::new(move |path| {
                    trace.lock().unwrap().push(json!(["isDirectory", path]));
                    let result = if case["statError"] == true {
                        Err("missing".into())
                    } else {
                        Ok(case["isDirectory"] != false)
                    };
                    async move { result }.boxed()
                }) as _
            };
            let read_file = {
                let files = case["files"].clone();
                let trace = trace.clone();
                Arc::new(move |path: String| {
                    trace.lock().unwrap().push(json!(["readFile", path]));
                    let result = files[&path]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "unreadable".into());
                    async move { result }.boxed()
                }) as _
            };
            create_grep_tool_definition(
                "fallback-must-not-be-used",
                GrepToolOptions {
                    operations: Some(GrepOperations {
                        is_directory,
                        read_file,
                    }),
                    ensure_tool: Some(ensure),
                    runner: Some(runner),
                },
            )
        };
        let abort = Arc::new(AbortSignal::new());
        if case["preAbort"] == true {
            abort.abort();
        }
        let result = (definition.execute_async.as_ref().unwrap())(
            "id".into(),
            case["input"].clone(),
            Some(abort),
            None,
            context(root),
        )
        .await;
        let outcome = match result {
            Ok(value) => json!({"value":value}),
            Err(error) => json!({"error":error}),
        };
        assert_eq!(outcome, case["outcome"], "outcome {}", case["id"]);
        assert_eq!(
            json!(*trace.lock().unwrap()),
            case["trace"],
            "trace {}",
            case["id"]
        );
    }
}
#[test]
fn both_platform_find_path_oracles() {
    let oracle: Value = serde_json::from_str(include_str!("search_tools_oracle.json")).unwrap();
    for case in oracle["pathCases"].as_array().unwrap() {
        assert_eq!(
            relativize_find_result_path(
                case["input"].as_str().unwrap(),
                case["root"].as_str().unwrap(),
                case["windows"].as_bool().unwrap()
            ),
            case["value"].as_str().unwrap(),
            "{case}"
        );
    }
    assert_eq!(
        build_fd_args("src/**/*.ts", "C:\\work", 7.0, false, true),
        vec![
            "--glob",
            "--color=never",
            "--hidden",
            "--no-require-git",
            "--max-results",
            "7",
            "--full-path",
            "--",
            r"**[/\\]src[/\\]**[/\\]*.ts",
            "C:\\work"
        ]
    );
}
#[tokio::test]
async fn cancellation_drops_remote_glob_and_native_search_adapters() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Guard(Arc<AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    for kind in ["custom-find", "native-find", "grep"] {
        let drops = Arc::new(AtomicUsize::new(0));
        let ready = Arc::new(tokio::sync::Notify::new());
        let signal = Arc::new(AbortSignal::new());
        let runner: SearchRunner = {
            let drops = drops.clone();
            let ready = ready.clone();
            Arc::new(move |_, _| {
                let guard = Guard(drops.clone());
                let ready = ready.clone();
                async move {
                    let _guard = guard;
                    ready.notify_one();
                    std::future::pending().await
                }
                .boxed()
            })
        };
        let ensure: EnsureSearchTool =
            Arc::new(|tool| async move { Ok(Some(tool.into())) }.boxed());
        let tool = if kind == "grep" {
            create_grep_tool_definition(
                "",
                GrepToolOptions {
                    ensure_tool: Some(ensure),
                    runner: Some(runner),
                    operations: Some(GrepOperations {
                        is_directory: Arc::new(|_| async { Ok(true) }.boxed()),
                        read_file: Arc::new(|_| async { panic!("no file read") }.boxed()),
                    }),
                },
            )
        } else {
            let operations = if kind == "custom-find" {
                let drops = drops.clone();
                let ready = ready.clone();
                Some(FindOperations {
                    exists: Arc::new(|_| async { Ok(true) }.boxed()),
                    glob: Arc::new(move |_, _, _| {
                        let guard = Guard(drops.clone());
                        let ready = ready.clone();
                        async move {
                            let _guard = guard;
                            ready.notify_one();
                            std::future::pending().await
                        }
                        .boxed()
                    }),
                })
            } else {
                None
            };
            create_find_tool_definition(
                "",
                FindToolOptions {
                    operations,
                    ensure_tool: Some(ensure),
                    runner: Some(runner),
                    path_exists: Some(Arc::new(|_| async { Ok(true) }.boxed())),
                },
            )
        };
        let future = (tool.execute_async.as_ref().unwrap())(
            "id".into(),
            json!({"pattern":"*"}),
            Some(signal.clone()),
            None,
            context(if cfg!(windows) { "C:\\work" } else { "/work" }),
        );
        tokio::pin!(future);
        tokio::select! {result=&mut future=>panic!("completed early: {result:?}"),_=ready.notified()=>{}}
        signal.abort();
        assert_eq!(future.await.unwrap_err(), "Operation aborted");
        assert_eq!(drops.load(Ordering::SeqCst), 1, "{kind}");
    }
}
