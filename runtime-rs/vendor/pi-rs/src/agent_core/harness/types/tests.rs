//! Tests for the `types.ts` port: serde wire shapes for the data vocabulary,
//! error-code values, and the callback/tool surface. Derived from the
//! upstream source (`types.ts`).

use super::*;

#[test]
fn skill_round_trips_upstream_wire_shape() {
    // types.ts:49-60 — camelCase keys, optional flag omitted when absent.
    let skill = Skill {
        name: "release".into(),
        description: "Prepare releases".into(),
        content: "# Release".into(),
        file_path: "/repo/.pi/skills/release.md".into(),
        disable_model_invocation: None,
    };
    let json = serde_json::to_value(&skill).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "name": "release",
            "description": "Prepare releases",
            "content": "# Release",
            "filePath": "/repo/.pi/skills/release.md",
        })
    );
    assert_eq!(serde_json::from_value::<Skill>(json).unwrap(), skill);

    let flagged = Skill {
        disable_model_invocation: Some(true),
        ..skill
    };
    let json = serde_json::to_value(&flagged).unwrap();
    assert_eq!(json["disableModelInvocation"], true);
}

#[test]
fn prompt_template_round_trips_with_optional_description() {
    // types.ts:62-70.
    let template = PromptTemplate {
        name: "review".into(),
        description: None,
        content: "Review {args}".into(),
    };
    let json = serde_json::to_value(&template).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"name": "review", "content": "Review {args}"})
    );
    assert_eq!(
        serde_json::from_value::<PromptTemplate>(json).unwrap(),
        template
    );
}

#[test]
fn resources_default_and_carry_optional_lists() {
    // types.ts:72-81.
    let empty: AgentHarnessResources = AgentHarnessResources::default();
    let json = serde_json::to_value(&empty).unwrap();
    assert_eq!(json, serde_json::json!({}));
    let resources: AgentHarnessResources = AgentHarnessResources {
        prompt_templates: None,
        skills: Some(vec![Skill {
            name: "s".into(),
            ..Skill::default()
        }]),
    };
    let json = serde_json::to_value(&resources).unwrap();
    assert_eq!(json["skills"].as_array().unwrap().len(), 1);
    assert!(json.get("promptTemplates").is_none());
}

#[test]
fn stream_options_round_trip_the_upstream_field_set() {
    // types.ts:129-147 — every curated option, camelCase, optional fields
    // omitted; and no run-signal field exists (the harness owns it).
    let options = AgentHarnessStreamOptions {
        transport: Some(Transport::WebsocketCached),
        timeout_ms: Some(5_000),
        max_retries: Some(3),
        max_retry_delay_ms: Some(1_000),
        headers: Some(BTreeMap::from([("x-test".into(), "1".into())])),
        metadata: Some(BTreeMap::from([("kind".into(), "run".into())])),
        cache_retention: Some(CacheRetention::Long),
        deferred: Some(DeferredFlag::Object {
            window: Some(crate::ai::types::options::DeferredWindow::OneHour),
        }),
    };
    let json = serde_json::to_value(&options).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "transport": "websocket-cached",
            "timeoutMs": 5000,
            "maxRetries": 3,
            "maxRetryDelayMs": 1000,
            "headers": {"x-test": "1"},
            "metadata": {"kind": "run"},
            "cacheRetention": "long",
            "deferred": {"window": "1h"},
        })
    );
    assert_eq!(
        serde_json::from_value::<AgentHarnessStreamOptions>(json).unwrap(),
        options
    );

    let empty = AgentHarnessStreamOptions::default();
    assert_eq!(serde_json::to_value(&empty).unwrap(), serde_json::json!({}));
}

#[test]
fn stream_options_patch_uses_delete_capable_maps() {
    // types.ts:149-156: `undefined` values delete keys, an explicit
    // `headers: undefined` clears the map, absent fields are omitted. The
    // double options carry the JS in-check/undefined distinction: None =
    // field absent, Some(None) = explicit undefined, Some(Some(v)) = set.
    let patch = AgentHarnessStreamOptionsPatch {
        timeout_ms: Some(Some(5000)),
        max_retries: Some(None),
        headers: Some(Some(BTreeMap::from([
            ("x-keep".into(), Some("1".into())),
            ("x-drop".into(), None),
        ]))),
        metadata: Some(None),
        ..AgentHarnessStreamOptionsPatch::default()
    };
    let json = serde_json::to_value(&patch).unwrap();
    assert_eq!(json["timeoutMs"], 5000);
    assert_eq!(json["maxRetries"], serde_json::Value::Null);
    assert_eq!(json["headers"]["x-drop"], serde_json::Value::Null);
    assert_eq!(json["headers"]["x-keep"], "1");
    assert_eq!(json["metadata"], serde_json::Value::Null);
    assert!(json.get("transport").is_none());
    let parsed: AgentHarnessStreamOptionsPatch = serde_json::from_value(json).unwrap();
    assert_eq!(parsed, patch);
}

#[test]
fn file_error_codes_serializes_upstream_literals() {
    // types.ts:162-170.
    let codes = [
        (FileErrorCode::Aborted, "aborted"),
        (FileErrorCode::NotFound, "not_found"),
        (FileErrorCode::PermissionDenied, "permission_denied"),
        (FileErrorCode::NotDirectory, "not_directory"),
        (FileErrorCode::IsDirectory, "is_directory"),
        (FileErrorCode::Invalid, "invalid"),
        (FileErrorCode::NotSupported, "not_supported"),
        (FileErrorCode::Unknown, "unknown"),
    ];
    for (code, literal) in codes {
        assert_eq!(serde_json::to_value(code).unwrap(), literal);
    }
}

#[test]
fn file_error_carries_code_message_and_path() {
    use std::error::Error as _;
    // types.ts:173-185.
    let error = FileError::new(
        FileErrorCode::NotFound,
        "no such file",
        Some("/tmp/missing".into()),
    );
    assert_eq!(error.code, FileErrorCode::NotFound);
    assert_eq!(error.to_string(), "no such file");
    assert_eq!(error.path.as_deref(), Some("/tmp/missing"));
    assert!(error.source().is_none());

    let cause = FileError::new(FileErrorCode::Invalid, "bad input", None);
    let wrapped =
        FileError::new(FileErrorCode::Unknown, "wrapped", None).with_cause(Some(Box::new(cause)));
    let source = wrapped.source().expect("cause is preserved");
    assert_eq!(source.to_string(), "bad input");
}

#[test]
fn execution_compaction_and_branch_error_codes_and_errors() {
    // types.ts:188-236.
    let execution = [
        (ExecutionErrorCode::Aborted, "aborted"),
        (ExecutionErrorCode::Timeout, "timeout"),
        (ExecutionErrorCode::ShellUnavailable, "shell_unavailable"),
        (ExecutionErrorCode::SpawnError, "spawn_error"),
        (ExecutionErrorCode::CallbackError, "callback_error"),
        (ExecutionErrorCode::Unknown, "unknown"),
    ];
    for (code, literal) in execution {
        assert_eq!(serde_json::to_value(code).unwrap(), literal);
    }
    let error = ExecutionError::new(ExecutionErrorCode::Timeout, "timed out");
    assert_eq!(error.code, ExecutionErrorCode::Timeout);
    assert_eq!(error.to_string(), "timed out");

    assert_eq!(
        serde_json::to_value(CompactionErrorCode::SummarizationFailed).unwrap(),
        "summarization_failed"
    );
    assert_eq!(
        serde_json::to_value(BranchSummaryErrorCode::Aborted).unwrap(),
        "aborted"
    );
    let compaction = CompactionError::new(CompactionErrorCode::Aborted, "cancelled");
    assert_eq!(compaction.code, CompactionErrorCode::Aborted);
    let branch = BranchSummaryError::new(
        BranchSummaryErrorCode::SummarizationFailed,
        "summary failed",
    );
    assert_eq!(branch.message, "summary failed");
}

#[test]
fn file_info_and_text_line_round_trip_camel_case() {
    // types.ts:239-257.
    let info = FileInfo {
        name: "notes.txt".into(),
        path: "/tmp/notes.txt".into(),
        kind: FileKind::File,
        size: 12,
        mtime_ms: 1_758_240_000_000.0,
    };
    let json = serde_json::to_value(&info).unwrap();
    assert_eq!(json["mtimeMs"], 1_758_240_000_000.0f64);
    assert_eq!(json["kind"], "file");
    assert_eq!(serde_json::from_value::<FileInfo>(json).unwrap(), info);

    let line = TextLine {
        text: "hello".into(),
        terminated: true,
    };
    let json = serde_json::to_value(&line).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"text": "hello", "terminated": true})
    );
}

#[test]
fn shell_output_update_kinds_round_trip() {
    // types.ts:367-371 — replace/append/slide/metadata, tagged by `kind`.
    let truncation = ShellOutputTruncation {
        truncated: true,
        truncated_by: Some(TruncatedBy::Bytes),
        total_lines: 10,
        total_bytes: 100,
        output_lines: 5,
        output_bytes: 50,
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines: 5,
        max_bytes: 50,
    };
    let metadata = ShellOutputMetadata {
        truncation,
        spill_path: Some("/tmp/spill".into()),
        last_line_bytes: Some(7),
    };
    let cases = [
        (
            ShellOutputUpdate::Replace {
                output: ShellOutputView {
                    metadata: metadata.clone(),
                    text: "abc".into(),
                },
            },
            serde_json::json!({
                "kind": "replace",
                "output": {
                    "truncation": {
                        "truncated": true, "truncatedBy": "bytes",
                        "totalLines": 10, "totalBytes": 100,
                        "outputLines": 5, "outputBytes": 50,
                        "lastLinePartial": false, "firstLineExceedsLimit": false,
                        "maxLines": 5, "maxBytes": 50,
                    },
                    "spillPath": "/tmp/spill",
                    "lastLineBytes": 7,
                    "text": "abc",
                }
            }),
        ),
        (
            ShellOutputUpdate::Append {
                text: "more".into(),
                metadata: metadata.clone(),
            },
            serde_json::json!({
                "kind": "append",
                "text": "more",
                "metadata": {
                    "truncation": {
                        "truncated": true, "truncatedBy": "bytes",
                        "totalLines": 10, "totalBytes": 100,
                        "outputLines": 5, "outputBytes": 50,
                        "lastLinePartial": false, "firstLineExceedsLimit": false,
                        "maxLines": 5, "maxBytes": 50,
                    },
                    "spillPath": "/tmp/spill",
                    "lastLineBytes": 7,
                },
            }),
        ),
        (
            ShellOutputUpdate::Slide {
                drop: 3,
                text: "tail".into(),
                metadata: metadata.clone(),
            },
            serde_json::json!({
                "kind": "slide",
                "drop": 3,
                "text": "tail",
                "metadata": {
                    "truncation": {
                        "truncated": true, "truncatedBy": "bytes",
                        "totalLines": 10, "totalBytes": 100,
                        "outputLines": 5, "outputBytes": 50,
                        "lastLinePartial": false, "firstLineExceedsLimit": false,
                        "maxLines": 5, "maxBytes": 50,
                    },
                    "spillPath": "/tmp/spill",
                    "lastLineBytes": 7,
                },
            }),
        ),
        (
            ShellOutputUpdate::Metadata {
                metadata: metadata.clone(),
            },
            serde_json::json!({
                "kind": "metadata",
                "metadata": {
                    "truncation": {
                        "truncated": true, "truncatedBy": "bytes",
                        "totalLines": 10, "totalBytes": 100,
                        "outputLines": 5, "outputBytes": 50,
                        "lastLinePartial": false, "firstLineExceedsLimit": false,
                        "maxLines": 5, "maxBytes": 50,
                    },
                    "spillPath": "/tmp/spill",
                    "lastLineBytes": 7,
                },
            }),
        ),
    ];
    for (update, json) in cases {
        assert_eq!(serde_json::to_value(&update).unwrap(), json);
        assert_eq!(
            serde_json::from_value::<ShellOutputUpdate>(json).unwrap(),
            update
        );
    }
}

#[test]
fn shell_exec_result_flattens_its_metadata() {
    // types.ts:374-376 — `extends ShellOutputMetadata { exitCode }`.
    let truncation = ShellOutputTruncation {
        truncated: false,
        truncated_by: None,
        total_lines: 1,
        total_bytes: 3,
        output_lines: 1,
        output_bytes: 3,
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines: 2000,
        max_bytes: 51_200,
    };
    let result = ShellExecResult {
        metadata: ShellOutputMetadata {
            truncation,
            spill_path: None,
            last_line_bytes: None,
        },
        exit_code: 0,
    };
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["exitCode"], 0);
    assert!(json.get("metadata").is_none(), "metadata is flattened");
    assert_eq!(json["truncation"]["truncated"], false);
}

#[test]
fn harness_tool_exposes_its_declaration() {
    // types.ts:107-122 — `Omit<AgentTool, "execute">` shares the declaration
    // surface with the M3a AgentTool.
    let tool: AgentHarnessTool<()> = AgentHarnessTool {
        name: "greet".into(),
        label: "Greet".into(),
        description: "Say hi".into(),
        parameters: serde_json::json!({"type": "object"}),
        constrained_sampling: None,
        execute: Arc::new(
            |_tool_call_id, _params, _on_update, (), _invocation, _context| {
                Box::pin(async { Ok(AgentToolResult::default()) })
            },
        ),
        prepare_arguments: None,
        replay: Some(ToolReplay::Safe),
        execution_mode: Some(ToolExecutionMode::Sequential),
    };
    let declaration = tool.declaration();
    assert_eq!(declaration.name, "greet");
    assert_eq!(declaration.description, "Say hi");
    assert_eq!(declaration.parameters["type"], "object");

    // Debug skips the executor; Clone shares it; the update options default.
    assert!(format!("{tool:?}").contains("greet"));
    let cloned = tool.clone();
    assert_eq!(cloned.name, tool.name);
    assert_eq!(cloned.replay, Some(ToolReplay::Safe));
    let options = AgentHarnessToolUpdateOptions::default();
    assert!(!options.checkpoint);
}

#[tokio::test]
async fn harness_tool_invocation_contract_is_object_safe_and_async() {
    // types.ts:95-105 — the runtime implements this; a test double proves the
    // async memo accessors are usable through the trait object.
    struct Invocation;

    impl AgentHarnessToolInvocation for Invocation {
        fn invocation_id(&self) -> &str {
            "inv-1"
        }
        fn operation_id(&self) -> &str {
            "op-1"
        }
        fn turn_id(&self) -> &str {
            "turn-1"
        }
        fn get_memo<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<serde_json::Value>> {
            Box::pin(async { Some(serde_json::json!({"step": 1})) })
        }
        fn set_memo<'a>(
            &'a self,
            _name: &'a str,
            _value: Option<serde_json::Value>,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async {})
        }
    }

    let invocation: Arc<dyn AgentHarnessToolInvocation> = Arc::new(Invocation);
    assert_eq!(invocation.invocation_id(), "inv-1");
    assert_eq!(invocation.operation_id(), "op-1");
    assert_eq!(invocation.turn_id(), "turn-1");
    let memo = invocation.get_memo("checkpoint").await;
    assert_eq!(memo, Some(serde_json::json!({"step": 1})));
    invocation.set_memo("checkpoint", None).await;
}

#[test]
fn tool_context_source_supports_static_and_provided_contexts() {
    // types.ts:124-127.
    let source: AgentHarnessToolContextSource<&'static str> =
        AgentHarnessToolContextSource::Static("static");
    assert!(matches!(
        source,
        AgentHarnessToolContextSource::Static("static")
    ));

    let provided: AgentHarnessToolContextSource<String> =
        AgentHarnessToolContextSource::Provider(Arc::new(|_context| {
            Box::pin(async { "resolved".to_string() })
        }));
    let resolved = match &provided {
        AgentHarnessToolContextSource::Provider(provider) => {
            let future = provider(Context::background());
            futures::executor::block_on(future)
        }
        AgentHarnessToolContextSource::Static(_) => unreachable!(),
    };
    assert_eq!(resolved, "resolved");
}

#[test]
fn file_content_and_shell_options_construct() {
    // types.ts:296, 379-392 — the unions/options carry text or binary
    // content and shell execution knobs.
    let content = FileContent::Binary(vec![1, 2, 3]);
    assert!(matches!(content, FileContent::Binary(_)));
    let options = ShellExecOptions {
        cwd: Some("/tmp".into()),
        timeout: Some(10.0),
        ..ShellExecOptions::default()
    };
    assert_eq!(options.timeout, Some(10.0));
    assert!(format!("{options:?}").contains("timeout: Some(10.0)"));
}
