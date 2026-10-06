//! Real upstream filetool execution oracle plus filesystem and scheduling regressions.
use super::*;
use crate::coding_agent::extensions::{loader::ExtensionRuntime, runner::ExtensionRunner, types};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;

fn read_ops(contents: Vec<u8>) -> read::ReadOperations {
    read::ReadOperations {
        read_file: Arc::new(move |_| {
            let bytes = contents.clone();
            Box::pin(async move { Ok(bytes) })
        }),
        access: Arc::new(|_| Box::pin(async { Ok(()) })),
        detect_image_mime_type: None,
    }
}
fn metadata(tool: &types::ToolDefinition) -> Value {
    let mut out = json!({"name":tool.name,"label":tool.label,"description":tool.description,"parameters":tool.parameters});
    for (key, value) in [
        (
            "promptSnippet",
            tool.prompt_snippet.as_ref().map(|v| json!(v)),
        ),
        (
            "promptGuidelines",
            tool.prompt_guidelines.as_ref().map(|v| json!(v)),
        ),
        ("constrainedSampling", tool.constrained_sampling.clone()),
        ("renderShell", tool.render_shell.as_ref().map(|v| json!(v))),
    ] {
        if let Some(value) = value {
            out[key] = value;
        }
    }
    out
}
fn ctx(cwd: &str) -> types::ExtensionContext {
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
async fn coding_agent_filetools_match_all_upstream_oracle_cases() {
    let fixture: Value = serde_json::from_str(include_str!("filetools_oracle.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    let defs = [
        read::create_read_tool_definition(cwd, Default::default()),
        write::create_write_tool_definition(cwd, Default::default()),
        edit::create_edit_tool_definition(cwd, Default::default()),
    ];
    for (def, expected) in defs.iter().zip(fixture["metadata"].as_array().unwrap()) {
        assert_eq!(metadata(def), *expected, "{} metadata", def.name);
    }
    for row in fixture["cases"].as_array().unwrap() {
        let name = row["id"].as_str().unwrap();
        let input = row["input"].clone();
        let observed = match row["kind"].as_str().unwrap() {
            "read" => {
                read::execute_read(
                    &serde_json::from_value(input).unwrap(),
                    cwd,
                    &read_ops(row["contents"].as_str().unwrap().as_bytes().to_vec()),
                    true,
                    false,
                    None,
                )
                .await
            }
            "edit" => {
                let contents = row["contents"].as_str().unwrap().as_bytes().to_vec();
                let written = Arc::new(Mutex::new(None));
                let out = written.clone();
                let ops = edit::EditOperations {
                    read_file: read_ops(contents).read_file,
                    access: Arc::new(|_| Box::pin(async { Ok(()) })),
                    write_file: Arc::new(move |_, text| {
                        let out = out.clone();
                        Box::pin(async move {
                            *out.lock().unwrap() = Some(text);
                            Ok(())
                        })
                    }),
                };
                let result = edit::execute_edit(
                    input["path"].as_str().unwrap(),
                    &serde_json::from_value::<Vec<edit::Edit>>(input["edits"].clone()).unwrap(),
                    cwd,
                    &ops,
                    None,
                )
                .await;
                assert_eq!(
                    json!(*written.lock().unwrap()),
                    row["written"],
                    "{name} write bytes"
                );
                result
            }
            "prepare" => Ok(edit::prepare_edit_arguments(input)),
            "preabort" => {
                let signal = Arc::new(types::AbortSignal::new());
                signal.abort();
                let def = defs
                    .iter()
                    .find(|d| d.name == row["tool"].as_str().unwrap())
                    .unwrap();
                (def.execute_async.as_ref().unwrap())(
                    "call".into(),
                    input,
                    Some(signal),
                    None,
                    ctx(cwd),
                )
                .await
            }
            "write" => {
                let trace = Arc::new(Mutex::new(Vec::<Value>::new()));
                let mkdir = trace.clone();
                let write = trace.clone();
                let base = dir.path().to_path_buf();
                let base2 = base.clone();
                let ops = write::WriteOperations {
                    mkdir: Arc::new(move |p| {
                        let trace = mkdir.clone();
                        let base = base.clone();
                        Box::pin(async move {
                            trace.lock().unwrap().push(json!([
                                "mkdir",
                                std::path::Path::new(&p)
                                    .strip_prefix(base)
                                    .unwrap()
                                    .to_string_lossy()
                                    .replace('\\', "/")
                            ]));
                            Ok(())
                        })
                    }),
                    write_file: Arc::new(move |p, text| {
                        let trace = write.clone();
                        let base = base2.clone();
                        Box::pin(async move {
                            trace.lock().unwrap().push(json!([
                                "write",
                                std::path::Path::new(&p)
                                    .strip_prefix(base)
                                    .unwrap()
                                    .to_string_lossy()
                                    .replace('\\', "/"),
                                text
                            ]));
                            Ok(())
                        })
                    }),
                };
                let result = write::execute_write(
                    input["path"].as_str().unwrap(),
                    input["content"].as_str().unwrap(),
                    cwd,
                    &ops,
                    None,
                )
                .await;
                assert_eq!(json!(*trace.lock().unwrap()), row["effects"], "{name}");
                result
            }
            kind => panic!("unknown {kind}"),
        };
        if let Some(expected) = row.get("error") {
            assert_eq!(observed.unwrap_err(), expected.as_str().unwrap(), "{name}");
        } else {
            assert_eq!(observed.unwrap(), row["ok"], "{name}");
        }
    }
}
#[tokio::test]
async fn real_filetools_create_read_edit_bom_crlf_and_keep_bom_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    let text = "\u{feff}one\r\ntwo\r\nthree\r\n";
    assert_eq!(
        write::execute_write("nested/file.txt", text, cwd, &Default::default(), None)
            .await
            .unwrap(),
        text_result("Successfully wrote to nested/file.txt")
    );
    let input = read::ReadToolInput {
        path: "nested/file.txt".into(),
        offset: None,
        limit: None,
    };
    assert_eq!(
        read::execute_read(&input, cwd, &Default::default(), true, false, None)
            .await
            .unwrap(),
        text_result(text)
    );
    let edits = vec![edit::Edit {
        old_text: "two".into(),
        new_text: "TWO\nsecond".into(),
    }];
    let result = edit::execute_edit(&input.path, &edits, cwd, &Default::default(), None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(dir.path().join(&input.path)).unwrap(),
        "\u{feff}one\r\nTWO\r\nsecond\r\nthree\r\n".as_bytes()
    );
    assert_eq!(result["details"]["firstChangedLine"], 2);
    assert!(result["details"]["patch"]
        .as_str()
        .unwrap()
        .contains("+TWO"));
}
#[tokio::test]
async fn read_buffer_utf8_replacement_and_cancellation_are_observable() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    let input = read::ReadToolInput {
        path: "file".into(),
        offset: None,
        limit: None,
    };
    assert_eq!(
        read::execute_read(
            &input,
            cwd,
            &read_ops(vec![0xef, 0xbb, 0xbf, 0xff, b'A']),
            true,
            false,
            None
        )
        .await
        .unwrap(),
        text_result("\u{feff}�A")
    );
    let signal = Arc::new(types::AbortSignal::new());
    let started = Arc::new(Notify::new());
    let notice = started.clone();
    let s = signal.clone();
    let task = tokio::spawn(async move {
        notice.notified().await;
        s.abort();
    });
    let ops = read::ReadOperations {
        read_file: Arc::new(move |_| {
            let n = started.clone();
            Box::pin(async move {
                n.notify_one();
                std::future::pending().await
            })
        }),
        ..read_ops(Vec::new())
    };
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(2),
            read::execute_read(&input, cwd, &ops, true, false, Some(&signal))
        )
        .await
        .unwrap()
        .unwrap_err(),
        "Operation aborted"
    );
    task.await.unwrap();
}
#[tokio::test]
async fn aborted_write_settles_before_following_mutation_and_different_files_progress() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap().to_owned();
    let target = dir.path().join("file");
    std::fs::write(&target, "old").unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let trace = Arc::new(Mutex::new(Vec::new()));
    let s = started.clone();
    let r = release.clone();
    let t = trace.clone();
    let ops = write::WriteOperations {
        mkdir: Arc::new(|_| Box::pin(async { Ok(()) })),
        write_file: Arc::new(move |_, value| {
            let (s, r, t) = (s.clone(), r.clone(), t.clone());
            Box::pin(async move {
                t.lock().unwrap().push(format!("start:{value}"));
                if value == "first" {
                    s.notify_one();
                    r.notified().await;
                }
                t.lock().unwrap().push(format!("end:{value}"));
                Ok(())
            })
        }),
    };
    let signal = Arc::new(types::AbortSignal::new());
    let c = cwd.clone();
    let o = ops.clone();
    let sig = signal.clone();
    let first =
        tokio::spawn(
            async move { write::execute_write("file", "first", &c, &o, Some(&sig)).await },
        );
    started.notified().await;
    signal.abort();
    let c = cwd.clone();
    let o = ops.clone();
    let second =
        tokio::spawn(async move { write::execute_write("./file", "second", &c, &o, None).await });
    write::execute_write("different", "other", &cwd, &ops, None)
        .await
        .unwrap();
    assert!(!first.is_finished());
    assert!(!second.is_finished());
    assert_eq!(
        *trace.lock().unwrap(),
        ["start:first", "start:other", "end:other"]
    );
    release.notify_one();
    assert_eq!(first.await.unwrap().unwrap_err(), "Operation aborted");
    second.await.unwrap().unwrap();
    assert_eq!(
        *trace.lock().unwrap(),
        [
            "start:first",
            "start:other",
            "end:other",
            "end:first",
            "start:second",
            "end:second"
        ]
    );
}
#[tokio::test]
async fn newly_created_file_keeps_the_same_queue_key_until_operation_settles() {
    let dir = tempfile::tempdir().unwrap();
    // environment-anchored: CI temp dirs can be 8.3 short paths (RUNNER~1)
    // while the queue canonicalizes existing files to the long form; start
    // from the canonical path so both queue keys agree on every machine.
    let canonical_dir = crate::coding_agent::utils::paths::canonicalize_path(
        dir.path().to_str().expect("utf8 temp dir"),
    );
    let path = std::path::Path::new(&canonical_dir)
        .join("new")
        .to_string_lossy()
        .into_owned();
    let path2 = path.clone();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (e, r) = (entered.clone(), release.clone());
    let first = tokio::spawn(async move {
        file_mutation_queue::with_file_mutation_queue(&path, || async {
            tokio::fs::write(&path, b"one").await.unwrap();
            e.notify_one();
            r.notified().await;
            Ok(())
        })
        .await
    });
    entered.notified().await;
    let second = tokio::spawn(async move {
        file_mutation_queue::with_file_mutation_queue(&path2, || async { Ok(()) }).await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !second.is_finished(),
        "canonical and missing path keys must agree"
    );
    release.notify_one();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
}
#[tokio::test]
async fn path_fallbacks_share_nfd_and_curly_quote_resolution_with_cli() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    // The entire filename must be NFD, including the first accent in écran.
    let name = "Capture d’e\u{301}cran e\u{301}.txt";
    std::fs::write(dir.path().join(name), "x").unwrap();
    let expected = dir.path().join(name).to_string_lossy().into_owned();
    assert_eq!(
        path_utils::resolve_read_path("@Capture d'écran é.txt", cwd).unwrap(),
        expected
    );
    assert_eq!(
        path_utils::resolve_read_path_async("@Capture d'écran é.txt", cwd)
            .await
            .unwrap(),
        expected
    );
    assert_eq!(
        crate::coding_agent::cli::file_processor::resolve_read_path("@Capture d'écran é.txt", cwd),
        expected
    );
}
