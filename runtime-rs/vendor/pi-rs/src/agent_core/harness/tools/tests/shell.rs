use super::support::{fake_output, TestEnv};
use super::*;
use crate::agent_core::harness::utils::DEFAULT_MAX_LINES;
use crate::agent_core::harness::{
    ExecutionError, ExecutionErrorCode, FileSystem, ShellExecOptions,
};
use std::sync::Mutex;
use std::time::Duration;

#[tokio::test]
async fn bash_combines_streams_empty_output_prefix_and_fractional_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let context = env(&dir);
    let output = text(
        &execute(
            create_bash_tool(Default::default()),
            context.clone(),
            json!({"command":"printf out; printf err >&2"}),
        )
        .await
        .unwrap(),
    );
    assert!(output.contains("out"));
    assert!(output.contains("err"));
    assert_eq!(
        text(
            &execute(
                create_bash_tool(Default::default()),
                context.clone(),
                json!({"command":":"})
            )
            .await
            .unwrap()
        ),
        "(no output)"
    );
    assert_eq!(
        text(
            &execute(
                create_bash_tool(BashToolOptions {
                    command_prefix: Some("value=hello".into()),
                    ..Default::default()
                }),
                context.clone(),
                json!({"command":"printf $value"})
            )
            .await
            .unwrap()
        ),
        "hello"
    );
    let error = execute(
        create_bash_tool(Default::default()),
        context.clone(),
        json!({"command":"printf failed; exit 7"}),
    )
    .await
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("failed\n\nCommand exited with code 7"));
    let error = execute(
        create_bash_tool(Default::default()),
        context,
        json!({"command":"sleep 2","timeout":0.01}),
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Command timed out after 0.01 seconds"),
        "{error}"
    );
}
#[tokio::test]
async fn bash_validates_timeout_before_prepare_or_execution() {
    let dir = tempfile::tempdir().unwrap();
    let mut wrapper = TestEnv::new(&dir);
    wrapper.exec = Some(Arc::new(|_, _, _| {
        panic!("invalid timeout must not execute")
    }));
    let context = ExecutionToolContext {
        env: Arc::new(wrapper),
    };
    for value in [0.0, -1.0, 2_147_483.648] {
        let error = execute(
            create_bash_tool(BashToolOptions {
                prepare: Some(Arc::new(|_, _, _| {
                    panic!("invalid timeout must not prepare")
                })),
                ..Default::default()
            }),
            context.clone(),
            json!({"command":":","timeout":value}),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().starts_with("Invalid timeout:"));
    }
}
#[tokio::test]
async fn bash_preparation_receives_custom_context_and_controls_execution() {
    struct Custom {
        env: Arc<dyn ExecutionEnv>,
        workspace: String,
    }
    impl HasExecutionEnv for Custom {
        fn execution_env(&self) -> Arc<dyn ExecutionEnv> {
            self.env.clone()
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut wrapper = TestEnv::new(&dir);
    let root = wrapper.inner.cwd().to_string();
    wrapper.exec = Some(Arc::new(|command, options, context| {
        Box::pin(async move {
            assert_eq!(command, "prefix=ready\n:\nprepared");
            assert!(options.cwd.unwrap().ends_with("workspace"));
            assert_eq!(
                options.env.unwrap().get("EXPLICIT").map(String::as_str),
                Some("value")
            );
            assert_eq!(options.inherit_env, Some(false));
            assert!(context.abort_signal().is_some());
            Ok(fake_output("done", &ShellExecOptions::default(), None))
        })
    }));
    let token = tokio_util::sync::CancellationToken::new();
    let context = crate::agent_core::harness::with_abort_signal(token, background_context());
    let tool = create_bash_tool_for(BashToolOptions {
        command_prefix: Some("prefix=ready".into()),
        prepare: Some(Arc::new(|execution, context: &Custom, call_context| {
            Box::pin(async move {
                assert!(Arc::strong_count(&context.env) >= 1);
                assert!(call_context.abort_signal().is_some());
                execution.cwd = context.workspace.clone();
                execution.env.insert("EXPLICIT".into(), "value".into());
                execution.inherit_env = false;
                execution.command += "\nprepared";
                Ok(())
            })
        })),
    });
    let result = (tool.execute)(
        "call".into(),
        json!({"command":":"}),
        Arc::new(|_, _| {}),
        Custom {
            env: Arc::new(wrapper),
            workspace: format!("{root}/workspace"),
        },
        Arc::new(Invocation),
        context,
    )
    .await
    .unwrap();
    assert_eq!(text(&result), "(no output)");
}
#[tokio::test]
async fn bash_ignores_late_callbacks_after_success_and_failure() {
    for failure in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut wrapper = TestEnv::new(&dir);
        let late = Arc::new(Mutex::new(None::<ShellExecOptions>));
        let saved = late.clone();
        wrapper.exec = Some(Arc::new(move |_, options, _| {
            let saved = saved.clone();
            Box::pin(async move {
                let result = fake_output("before\n", &options, None);
                *saved.lock().unwrap() = Some(options);
                if failure {
                    Err(ExecutionError::new(ExecutionErrorCode::Aborted, "aborted"))
                } else {
                    Ok(result)
                }
            })
        }));
        let updates = Arc::new(Mutex::new(Vec::new()));
        let out = updates.clone();
        let result = (create_bash_tool(Default::default()).execute)(
            "call".into(),
            json!({"command":"late"}),
            Arc::new(move |r, _| out.lock().unwrap().push(text(r))),
            ExecutionToolContext {
                env: Arc::new(wrapper),
            },
            Arc::new(Invocation),
            background_context(),
        )
        .await;
        if failure {
            assert_eq!(
                result.unwrap_err().to_string(),
                "before\n\n\nCommand aborted"
            );
        } else {
            assert_eq!(text(&result.unwrap()), "before\n");
        }
        let before = updates.lock().unwrap().clone();
        fake_output(
            "before\nlate\n",
            late.lock().unwrap().as_ref().unwrap(),
            None,
        );
        assert_eq!(*updates.lock().unwrap(), before);
    }
}
#[tokio::test(start_paused = true)]
async fn bash_distinct_checkpoint_snapshots_have_two_second_cadence() {
    let dir = tempfile::tempdir().unwrap();
    let mut wrapper = TestEnv::new(&dir);
    wrapper.exec = Some(Arc::new(|_, options, _| {
        Box::pin(async move {
            fake_output("one\n", &options, None);
            tokio::time::sleep(Duration::from_millis(2100)).await;
            fake_output("one\ntwo\n", &options, None);
            tokio::time::sleep(Duration::from_millis(100)).await;
            fake_output("one\ntwo\nthree\n", &options, None);
            tokio::time::sleep(Duration::from_millis(2000)).await;
            fake_output("one\ntwo\nthree\nfour\n", &options, None);
            tokio::time::sleep(Duration::from_millis(2100)).await;
            fake_output("one\ntwo\nthree\nfour\n", &options, None);
            Ok(fake_output("one\ntwo\nthree\nfour\nfive\n", &options, None))
        })
    }));
    let checkpoints = Arc::new(Mutex::new(Vec::new()));
    let out = checkpoints.clone();
    (create_bash_tool(Default::default()).execute)(
        "call".into(),
        json!({"command":"controlled"}),
        Arc::new(move |r, o| {
            if o.checkpoint {
                out.lock().unwrap().push(text(r));
            }
        }),
        ExecutionToolContext {
            env: Arc::new(wrapper),
        },
        Arc::new(Invocation),
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(
        *checkpoints.lock().unwrap(),
        vec![
            "one\ntwo\n",
            "one\ntwo\nthree\nfour\n",
            "one\ntwo\nthree\nfour\nfive\n"
        ]
    );
}
#[tokio::test]
async fn bash_timeout_preserves_bounded_capture_and_spill_reference() {
    let dir = tempfile::tempdir().unwrap();
    let mut wrapper = TestEnv::new(&dir);
    let inner = wrapper.inner.clone();
    wrapper.exec = Some(Arc::new(move |_, options, _| {
        let inner = inner.clone();
        Box::pin(async move {
            assert_eq!(options.timeout, Some(0.05));
            let output = (1..=DEFAULT_MAX_LINES + 1)
                .map(|n| format!("line-{n}\n"))
                .collect::<String>();
            inner
                .write_file(
                    "full",
                    crate::agent_core::harness::FileContent::Text(output.clone()),
                    background_context(),
                )
                .await
                .unwrap();
            let path = inner
                .absolute_path("full", background_context())
                .await
                .unwrap();
            fake_output(&output, &options, Some(path));
            Err(ExecutionError::new(ExecutionErrorCode::Timeout, "timeout"))
        })
    }));
    let error = execute(
        create_bash_tool(Default::default()),
        ExecutionToolContext {
            env: Arc::new(wrapper),
        },
        json!({"command":"controlled","timeout":0.05}),
    )
    .await
    .unwrap_err();
    let output = error.to_string();
    assert!(output.contains("Showing lines 2-2001 of 2001"), "{output}");
    assert!(output.contains("Full output:"));
    assert!(output.ends_with("Command timed out after 0.05 seconds"));
    assert!(std::fs::read_to_string(dir.path().join("full"))
        .unwrap()
        .starts_with("line-1\nline-2\n"));
}
#[tokio::test]
async fn bash_coalesces_live_updates_and_preserves_full_spilled_output() {
    let dir = tempfile::tempdir().unwrap();
    let context = env(&dir);
    let updates = Arc::new(Mutex::new(Vec::new()));
    let out = updates.clone();
    let result = (create_bash_tool(Default::default()).execute)(
        "call".into(),
        json!({"command":"i=1; while [ $i -le 3000 ]; do echo line-$i; i=$((i + 1)); done"}),
        Arc::new(move |r, _| out.lock().unwrap().push(r.clone())),
        context.clone(),
        Arc::new(Invocation),
        background_context(),
    )
    .await
    .unwrap();
    assert!(updates.lock().unwrap().len() < 25);
    let details = result.details.unwrap();
    assert_eq!(details["truncation"]["totalLines"], 3000);
    assert_eq!(details["truncation"]["outputLines"], 2000);
    let path = details["fullOutputPath"].as_str().unwrap();
    let full = context
        .env
        .read_text_file(path, background_context())
        .await
        .unwrap();
    assert!(full.starts_with("line-1\nline-2\n"));
    assert!(full.ends_with("line-2999\nline-3000\n"));
    assert!(text(updates.lock().unwrap().last().unwrap()).contains("line-3000"));
    let result = execute(
        create_bash_tool(Default::default()),
        context,
        json!({"command":"printf '%060000d' 0"}),
    )
    .await
    .unwrap();
    assert!(text(&result).contains("Showing last 50.0KB of line 1 (line is 58.6KB). Full output:"));
}
