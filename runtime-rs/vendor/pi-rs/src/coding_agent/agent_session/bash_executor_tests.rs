use super::*;
use serde_json::{json, Value};
use std::time::Duration;

fn bytes(chunk: &Value) -> Vec<u8> {
    if let Some(hex) = chunk["hex"].as_str() {
        hex.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect()
    } else {
        chunk["text"]
            .as_str()
            .unwrap()
            .repeat(chunk["repeat"].as_u64().unwrap_or(1) as usize)
            .into_bytes()
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn executor_matches_upstream_raw_stream_sanitization_rolling_and_spill_oracle() {
    let fixture: Value = serde_json::from_str(include_str!("bash_executor_oracle.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let streamed = Arc::new(Mutex::new(Vec::<String>::new()));
        let operations = BashOperationsHandle {
            exec: {
                let case = case.clone();
                let calls = calls.clone();
                Arc::new(move |command, cwd, callbacks| {
                    let case = case.clone();
                    let calls = calls.clone();
                    Box::pin(async move {
                        calls
                            .lock()
                            .unwrap()
                            .push(json!({"command":command,"cwd":cwd}));
                        for chunk in case["chunks"].as_array().unwrap() {
                            (callbacks.on_bytes)(&bytes(chunk))?;
                        }
                        if case["signal"] == "after" {
                            callbacks.signal.cancel();
                        }
                        if let Some(error) = case["error"].as_str() {
                            return Err(error.to_owned());
                        }
                        Ok(
                            if case["missingExit"] == true
                                || case.get("exitCode").is_some_and(Value::is_null)
                            {
                                None
                            } else {
                                Some(case["exitCode"].as_i64().unwrap_or(0))
                            },
                        )
                    })
                })
            },
        };
        let signal = case["signal"].as_str().map(|mode| {
            let signal = CancellationToken::new();
            if mode == "before" {
                signal.cancel();
            }
            signal
        });
        let on_chunk: Option<OnChunkCallback> = if case["emitChunks"] == false {
            None
        } else {
            let streamed = streamed.clone();
            Some(Arc::new(move |text| {
                streamed.lock().unwrap().push(text.to_owned())
            }))
        };
        let run = execute_with_temp_directory(
            "command",
            "$CWD",
            operations,
            BashExecutorOptions { on_chunk, signal },
            directory.path(),
        )
        .await;
        let outcome = match run {
            Ok(mut result) => {
                if result.full_output_path.is_some() {
                    result.full_output_path = Some("$SPILL".into());
                }
                json!({"value":result})
            }
            Err(error) => json!({"error":error}),
        };
        let files = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        assert!(files.len() <= 1);
        let spill = files.first().map(|p| hex(&std::fs::read(p).unwrap()));
        assert_eq!(
            json!(*calls.lock().unwrap()),
            case["calls"],
            "{} calls",
            case["id"]
        );
        assert_eq!(
            json!(*streamed.lock().unwrap()),
            case["streamedChunks"],
            "{} chunks",
            case["id"]
        );
        assert_eq!(outcome, case["outcome"], "{} result", case["id"]);
        assert_eq!(json!(spill), case["fullOutputHex"], "{} spill", case["id"]);
    }
}

#[test]
fn sanitize_binary_output_keeps_text() {
    assert_eq!(
        sanitize_binary_output("hello\r\nworld\u{7}\t!"),
        "hello\r\nworld\t!"
    );
    assert_eq!(sanitize_binary_output("好的\u{fff9}x\u{fffb}"), "好的x");
}
#[test]
fn shared_text_decoder_handles_split_sequences_without_flushing_incomplete_eof() {
    let mut decoder = TextDecoder::default();
    assert_eq!(decoder.decode(b"\xef", false), "");
    assert_eq!(decoder.decode(b"\xbb\xbf", false), "");
    assert_eq!(decoder.decode(b"h\xc3", false), "h");
    assert_eq!(decoder.decode(b"\xa9llo\xf0\x9f", false), "éllo");
}
#[tokio::test]
async fn executes_and_streams_with_shared_native_discovery() {
    let chunks = Arc::new(Mutex::new(Vec::new()));
    let sink = chunks.clone();
    let result = execute_bash_with_operations(
        "printf 'a\\nb\\n'",
        ".",
        create_local_bash_operations(None),
        BashExecutorOptions {
            on_chunk: Some(Arc::new(move |delta| {
                sink.lock().unwrap().push(delta.to_owned())
            })),
            signal: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert!(!result.cancelled && !result.truncated);
    assert_eq!(result.output, "a\nb\n");
    assert_eq!(chunks.lock().unwrap().join(""), result.output);
}
#[tokio::test]
async fn failed_spawn_errors_and_preabort_precedes_shell_and_cwd_validation() {
    let missing = tempfile::tempdir().unwrap();
    let shell = missing
        .path()
        .join("not-a-shell")
        .to_string_lossy()
        .into_owned();
    let result = execute_bash_with_operations(
        "echo hi",
        ".",
        create_local_bash_operations(Some(&shell)),
        BashExecutorOptions::default(),
    )
    .await;
    assert_eq!(
        result.unwrap_err(),
        format!("Custom shell path not found: {shell}")
    );
    let signal = CancellationToken::new();
    signal.cancel();
    let result = execute_bash_with_operations(
        "echo hi",
        "not-a-cwd",
        create_local_bash_operations(Some(&shell)),
        BashExecutorOptions {
            on_chunk: None,
            signal: Some(signal),
        },
    )
    .await
    .unwrap();
    assert_eq!(result.output, "");
    assert!(result.cancelled);
    assert_eq!(result.exit_code, None);
}
#[tokio::test]
async fn native_cancellation_bridge_preserves_partial_output_and_drains_tree() {
    let signal = CancellationToken::new();
    let cancel = signal.clone();
    let result = tokio::time::timeout(
        Duration::from_secs(12),
        execute_bash_with_operations(
            "printf READY; sleep 8; printf NEVER",
            ".",
            create_local_bash_operations(None),
            BashExecutorOptions {
                on_chunk: Some(Arc::new(move |text| {
                    if text.contains("READY") {
                        cancel.cancel();
                    }
                })),
                signal: Some(signal),
            },
        ),
    )
    .await
    .expect("native cancellation must terminate promptly")
    .unwrap();
    assert_eq!(result.output, "READY");
    assert!(result.cancelled);
    assert_eq!(result.exit_code, None);
}
#[tokio::test]
async fn spill_failures_propagate_for_raw_and_legacy_extension_callbacks() {
    let directory = tempfile::tempdir().unwrap();
    for legacy in [false, true] {
        let operations = BashOperationsHandle {
            exec: Arc::new(move |_, _, callbacks| {
                Box::pin(async move {
                    let text = "x".repeat(51_201);
                    if legacy {
                        (callbacks.on_data)(&text);
                    } else {
                        (callbacks.on_bytes)(text.as_bytes())?;
                    }
                    Ok(Some(0))
                })
            }),
        };
        let error = execute_with_temp_directory(
            "c",
            ".",
            operations,
            BashExecutorOptions::default(),
            &directory.path().join("missing"),
        )
        .await
        .unwrap_err();
        assert!(!error.is_empty());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
#[tokio::test(start_paused = true)]
async fn dropped_and_finished_executor_revoke_retained_callbacks_and_close_spill() {
    for drop_run in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let callback = Arc::new(Mutex::new(None));
        let chunks = Arc::new(Mutex::new(Vec::new()));
        let operations = BashOperationsHandle {
            exec: {
                let callback = callback.clone();
                Arc::new(move |_, _, callbacks| {
                    *callback.lock().unwrap() = Some(callbacks.on_bytes.clone());
                    Box::pin(async move {
                        (callbacks.on_bytes)(&vec![b'x'; 51_201])?;
                        if drop_run {
                            std::future::pending::<()>().await;
                        }
                        Ok(Some(0))
                    })
                })
            },
        };
        let on_chunk: OnChunkCallback = {
            let chunks = chunks.clone();
            Arc::new(move |text| chunks.lock().unwrap().push(text.to_owned()))
        };
        let run = tokio::time::timeout(
            Duration::from_millis(20),
            execute_with_temp_directory(
                "c",
                ".",
                operations,
                BashExecutorOptions {
                    on_chunk: Some(on_chunk),
                    signal: None,
                },
                directory.path(),
            ),
        )
        .await;
        assert_eq!(run.is_err(), drop_run);
        if let Ok(result) = run {
            result.unwrap();
        }
        let path = std::fs::read_dir(directory.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let before = std::fs::read(&path).unwrap();
        assert_eq!(before.len(), 51_201);
        (callback.lock().unwrap().as_ref().unwrap())(b"not-after-completion").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert_eq!(chunks.lock().unwrap().len(), 1);
    }
}
