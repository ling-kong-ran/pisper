use super::*;
use crate::coding_agent::extensions::{loader::ExtensionRuntime, runner::ExtensionRunner, types};
use futures::FutureExt;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
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
async fn execution_metadata_and_filesystem_effects_match_real_upstream() {
    let oracle: Value = serde_json::from_str(include_str!("ls_oracle.json")).unwrap();
    let definition = create_ls_tool_definition("", Default::default());
    assert_eq!(
        json!({"name":definition.name,"label":definition.label,"description":definition.description,"parameters":definition.parameters,"promptSnippet":definition.prompt_snippet}),
        oracle["metadata"]
    );
    for case in oracle["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["windows"] == cfg!(windows))
    {
        let root = case["root"].as_str().unwrap();
        let dir_path = resolve_to_cwd(
            case["input"]["path"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("."),
            root,
        )
        .unwrap();
        let trace = Arc::new(Mutex::new(vec![]));
        let exists: LsExists = {
            let case = case.clone();
            let trace = trace.clone();
            Arc::new(move |path| {
                trace.lock().unwrap().push(json!(["exists", path]));
                let result = if let Some(error) = case["existsError"].as_str() {
                    Err(error.to_owned())
                } else {
                    Ok(case["exists"] != false)
                };
                async move { result }.boxed()
            })
        };
        let stat: LsStat = {
            let case = case.clone();
            let trace = trace.clone();
            let root = dir_path.clone();
            Arc::new(move |path| {
                trace.lock().unwrap().push(json!(["stat", path]));
                let result = if path == root {
                    if let Some(error) = case["statError"].as_str() {
                        Err(error.to_owned())
                    } else {
                        Ok(case["isDirectory"] != false)
                    }
                } else {
                    let belongs = |name: &str| path == path_join(&root, name);
                    let contains = |key: &str| {
                        case[key].as_array().is_some_and(|values| {
                            values.iter().any(|value| belongs(value.as_str().unwrap()))
                        })
                    };
                    if contains("statFailures") {
                        Err("inaccessible".into())
                    } else {
                        Ok(contains("directories"))
                    }
                };
                async move { result }.boxed()
            })
        };
        let readdir: LsReaddir = {
            let case = case.clone();
            let trace = trace.clone();
            Arc::new(move |path| {
                trace.lock().unwrap().push(json!(["readdir", path]));
                let result = if let Some(error) = case["readdirError"].as_str() {
                    Err(error.to_owned())
                } else {
                    Ok(case["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().to_owned())
                        .collect())
                };
                async move { result }.boxed()
            })
        };
        let tool = create_ls_tool_definition(
            "fallback-must-not-be-used",
            LsToolOptions {
                operations: Some(LsOperations {
                    exists,
                    stat,
                    readdir,
                }),
                locale: case["locale"].as_str().map(str::to_owned),
            },
        );
        let signal = Arc::new(AbortSignal::new());
        if case["preAbort"] == true {
            signal.abort();
        }
        let result = (tool.execute_async.as_ref().unwrap())(
            "id".into(),
            case["input"].clone(),
            Some(signal),
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
#[tokio::test]
async fn native_listing_includes_dotfiles_follows_links_and_rejects_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy();
    for file in ["z.txt", "A.txt", ".hidden"] {
        std::fs::write(dir.path().join(file), "test").unwrap();
    }
    std::fs::create_dir(dir.path().join("dir")).unwrap();
    let options = LsToolOptions {
        locale: Some("en-US".into()),
        ..Default::default()
    };
    let result = execute_ls(&Default::default(), &root, &options, None)
        .await
        .unwrap();
    assert_eq!(result, text_result(".hidden\nA.txt\ndir/\nz.txt"));
    let result = execute_ls(
        &LsToolInput {
            path: None,
            limit: Some(2.0),
        },
        &root,
        &options,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        result["content"][0]["text"],
        ".hidden\nA.txt\n\n[2 entries limit reached. Use limit=4 for more]"
    );
    assert_eq!(result["details"]["entryLimitReached"], 2);
    let result = execute_ls(
        &LsToolInput {
            path: Some("dir".into()),
            limit: None,
        },
        &root,
        &options,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result, text_result("(empty directory)"));
    for (path, prefix) in [
        ("z.txt", "Not a directory: "),
        ("absent", "Path not found: "),
    ] {
        let error = execute_ls(
            &LsToolInput {
                path: Some(path.into()),
                limit: None,
            },
            &root,
            &options,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(error, format!("{prefix}{}", path_join(&root, path)));
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("dir"), dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("absent"), dir.path().join("broken")).unwrap();
        let result = execute_ls(&Default::default(), &root, &options, None)
            .await
            .unwrap();
        assert_eq!(result, text_result(".hidden\nA.txt\ndir/\nlink/\nz.txt"));
    }
}
#[tokio::test]
async fn abort_releases_each_inflight_filesystem_operation() {
    struct Guard(Arc<AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    for stage in ["exists", "stat-root", "readdir", "stat-entry"] {
        let root = if cfg!(windows) { "C:\\work" } else { "/work" };
        let signal = Arc::new(AbortSignal::new());
        let ready = Arc::new(tokio::sync::Notify::new());
        let drops = Arc::new(AtomicUsize::new(0));
        let pending: LsExists = {
            let ready = ready.clone();
            let drops = drops.clone();
            Arc::new(move |_| {
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
        let exists: LsExists = if stage == "exists" {
            pending.clone()
        } else {
            Arc::new(|_| async { Ok(true) }.boxed())
        };
        let stat: LsStat = {
            let pending = pending.clone();
            Arc::new(move |path| {
                if (stage == "stat-root" && path == root) || (stage == "stat-entry" && path != root)
                {
                    pending(path)
                } else {
                    async { Ok(true) }.boxed()
                }
            })
        };
        let readdir: LsReaddir = Arc::new(move |path| {
            let pending = pending.clone();
            async move {
                if stage == "readdir" {
                    pending(path).await?;
                }
                Ok(vec!["entry".into()])
            }
            .boxed()
        });
        let tool = create_ls_tool_definition(
            root,
            LsToolOptions {
                operations: Some(LsOperations {
                    exists,
                    stat,
                    readdir,
                }),
                locale: Some("en-US".into()),
            },
        );
        let future = (tool.execute_async.as_ref().unwrap())(
            "id".into(),
            json!({}),
            Some(signal.clone()),
            None,
            context(root),
        );
        tokio::pin!(future);
        tokio::select! { result=&mut future=>panic!("completed early: {result:?}"), _=ready.notified()=>{} }
        signal.abort();
        assert_eq!(future.await.unwrap_err(), "Operation aborted");
        assert_eq!(drops.load(Ordering::SeqCst), 1, "{stage}");
    }
}
