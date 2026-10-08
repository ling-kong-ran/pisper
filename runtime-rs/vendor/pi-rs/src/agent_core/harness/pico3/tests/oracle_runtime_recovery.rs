//! Reopen-at-durable-phase oracles ported from recovery.test.ts/retention.test.ts.
//! The upstream crash fixture closes the harness; here that is suspend/close,
//! not an OS process kill. Atomic torn-tail storage tests live separately.

use super::support_runtime::*;
use crate::agent_core::harness::pico3::kinds::post_tools::PostToolsHandlers;
use crate::agent_core::harness::pico3::runtime::{
    abort_closure_from, Kind, PhaseFn, Step, TaskKind,
};
use crate::agent_core::harness::pico3::session::CreateTaskOptions;
use crate::agent_core::harness::pico3::types::{Completion, SendInput, TaskStatus, UserInput};
use serde_json::{json, Value};
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

async fn send(env: &Env, text: &str) -> RootInput {
    env.root
        .send(
            SendInput {
                content: UserInput::Text(text.to_owned()),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap()
}
async fn settle(input: &RootInput) {
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(8), input.wait(ctx()))
            .await
            .unwrap()
            .unwrap()
            .status,
        "done"
    );
}
async fn idle(env: &Env) {
    tokio::time::timeout(Duration::from_secs(8), env.root.wait_for_idle(ctx()))
        .await
        .expect("recovery must settle")
        .unwrap();
}
async fn arrived(gate: &Gate, count: usize) {
    tokio::time::timeout(Duration::from_secs(8), gate.arrivals(count))
        .await
        .expect("durable phase must be entered");
}
async fn open_hook(
    mut options: OpenOptions,
    kind: &str,
    handlers: Arc<dyn Any + Send + Sync>,
) -> Env {
    options.paused = true;
    let env = open(options).await.unwrap();
    let namespace = env
        .h
        .namespace("spec.recovery", Default::default(), None)
        .unwrap();
    let _off = env
        .h
        .hooks(&namespace, &env.h.builtin_kind(kind).unwrap(), handlers)
        .unwrap();
    env.h.resume();
    env
}

#[tokio::test]
async fn requesting_reopen_records_interrupted_usage_and_preserves_request_id() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Gate::new();
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        models: Some(FakeModels::new(Arc::new(echo_script)).with_gate(gate.clone())),
        ..Default::default()
    })
    .await
    .unwrap();
    let input = env
        .root
        .send(
            SendInput {
                content: UserInput::Text("A".to_owned()),
                request_id: Some("r".to_owned()),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    until_phase(&env, "pi.generation", Some("requesting"))
        .await
        .unwrap();
    arrived(&gate, 1).await;
    env.close(ctx()).await.unwrap();
    let models = FakeModels::new(Arc::new(echo_script));
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        models: Some(models.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let same = env
        .root
        .send(
            SendInput {
                content: UserInput::Text("dup".to_owned()),
                request_id: Some("r".to_owned()),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    assert_eq!(same.id, input.id);
    settle(&same).await;
    idle(&env).await;
    let entries = env.entries(1).await.unwrap();
    assert_eq!(kinds(&entries), "user system usage assistant");
    assert_eq!(
        entries[2].data,
        Some(
            json!({"attempt":1,"error":"interrupted"})
                .as_object()
                .unwrap()
                .clone()
        )
    );
    assert_eq!(models.calls(), 1);
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn prepared_reopen_reruns_hook_without_duplicate_system_or_interrupted_usage() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Gate::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let handlers = Arc::new(GenerationHandlers {
        before_request: Some(Arc::new({
            let gate = gate.clone();
            let calls = calls.clone();
            move |_, _, _, context| {
                let gate = gate.clone();
                let calls = calls.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    gate.wait(context).await?;
                    Ok(None)
                })
            }
        })),
        ..Default::default()
    });
    let env = open_hook(
        OpenOptions {
            dir: Some(dir.path().to_owned()),
            ..Default::default()
        },
        "pi.generation",
        handlers.clone(),
    )
    .await;
    let input = send(&env, "A").await;
    until_phase(&env, "pi.generation", Some("prepared"))
        .await
        .unwrap();
    arrived(&gate, 1).await;
    env.close(ctx()).await.unwrap();
    gate.open();
    let env = open_hook(
        OpenOptions {
            dir: Some(dir.path().to_owned()),
            ..Default::default()
        },
        "pi.generation",
        handlers,
    )
    .await;
    idle(&env).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let entries = env.entries(1).await.unwrap();
    assert_eq!(entries.iter().filter(|e| e.kind == "pi.system").count(), 1);
    assert_eq!(entries.iter().filter(|e| e.kind == "pi.usage").count(), 0);
    assert_eq!(env.input(input.id).await.unwrap().unwrap().status, "done");
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn retrying_reopen_honors_the_persisted_deadline_before_requesting_again() {
    let dir = tempfile::tempdir().unwrap();
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        models: Some(FakeModels::new(Arc::new(|_, _| Response {
            error: Some("overloaded".to_owned()),
            ..Default::default()
        }))),
        root_sticky: Some(
            json!({"retry":{"enabled":true,"maxRetries":3,"baseDelayMs":300}})
                .as_object()
                .unwrap()
                .clone(),
        ),
        ..Default::default()
    })
    .await
    .unwrap();
    let input = send(&env, "A").await;
    let task = until_phase(&env, "pi.generation", Some("retrying"))
        .await
        .unwrap();
    let until = task.checkpoint.as_ref().unwrap()["untilMs"]
        .as_i64()
        .unwrap();
    env.close(ctx()).await.unwrap();
    let models = FakeModels::new(Arc::new(echo_script));
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        models: Some(models.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    idle(&env).await;
    assert!(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            >= until
    );
    assert_eq!(env.input(input.id).await.unwrap().unwrap().status, "done");
    assert_eq!(models.calls(), 1);
    assert_eq!(
        kinds(&env.entries(1).await.unwrap()),
        "user system usage assistant"
    );
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn started_tool_reopen_reinvokes_safe_but_synthesizes_interrupted_for_unsafe() {
    for replay in ["safe", "unsafe"] {
        let dir = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let first = tool(
            "x",
            ToolOptions {
                replay: Some(replay.to_owned()),
                gate: Some(gate.clone()),
                ..Default::default()
            },
        );
        let env = open(OpenOptions {
            dir: Some(dir.path().to_owned()),
            tools: vec![first.declaration()],
            ..Default::default()
        })
        .await
        .unwrap();
        let input = send(&env, "tool:x").await;
        until_phase(&env, "pi.tool", Some("started")).await.unwrap();
        arrived(&gate, 1).await;
        env.close(ctx()).await.unwrap();
        assert_eq!(first.calls(), 1);
        let second = tool(
            "x",
            ToolOptions {
                replay: Some(replay.to_owned()),
                ..Default::default()
            },
        );
        let env = open(OpenOptions {
            dir: Some(dir.path().to_owned()),
            tools: vec![second.declaration()],
            ..Default::default()
        })
        .await
        .unwrap();
        idle(&env).await;
        assert_eq!(env.input(input.id).await.unwrap().unwrap().status, "done");
        let entries = env.entries(1).await.unwrap();
        let result = entries.iter().find(|e| e.kind == "pi.tool_result").unwrap();
        if replay == "safe" {
            assert_eq!(second.calls(), 1);
            assert!(content_of(result).contains("x(x)"));
        } else {
            assert_eq!(second.calls(), 0);
            assert!(content_of(result).contains("interrupted"));
        }
        assert_eq!(
            kinds(&entries),
            "user system assistant tool_result assistant"
        );
        env.close(ctx()).await.unwrap();
    }
}

#[tokio::test]
async fn before_started_reopen_reruns_approval_but_executes_the_tool_once() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Gate::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let handlers = Arc::new(ToolHandlers {
        before_tool: Some(Arc::new({
            let gate = gate.clone();
            let calls = calls.clone();
            move |_, _, context| {
                let gate = gate.clone();
                let calls = calls.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    gate.wait(context).await?;
                    Ok(None)
                })
            }
        })),
        ..Default::default()
    });
    let first = tool("x", ToolOptions::default());
    let env = open_hook(
        OpenOptions {
            dir: Some(dir.path().to_owned()),
            tools: vec![first.declaration()],
            ..Default::default()
        },
        "pi.tool",
        handlers.clone(),
    )
    .await;
    let input = send(&env, "tool:x").await;
    arrived(&gate, 1).await;
    let pending = until_phase(&env, "pi.tool", None).await.unwrap();
    assert!(pending.checkpoint.is_none());
    env.close(ctx()).await.unwrap();
    assert_eq!(first.calls(), 0);
    gate.open();
    let second = tool("x", ToolOptions::default());
    let env = open_hook(
        OpenOptions {
            dir: Some(dir.path().to_owned()),
            tools: vec![second.declaration()],
            ..Default::default()
        },
        "pi.tool",
        handlers,
    )
    .await;
    idle(&env).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(second.calls(), 1);
    assert_eq!(env.input(input.id).await.unwrap().unwrap().status, "done");
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn post_tools_reopen_reruns_observer_but_creates_exactly_one_continuation() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Gate::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let handlers = Arc::new(PostToolsHandlers {
        after_tools: Some(Arc::new({
            let gate = gate.clone();
            let calls = calls.clone();
            move |_, _, _, context| {
                let gate = gate.clone();
                let calls = calls.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    gate.wait(context).await
                })
            }
        })),
    });
    let env = open_hook(
        OpenOptions {
            dir: Some(dir.path().to_owned()),
            tools: vec![tool("x", ToolOptions::default()).declaration()],
            ..Default::default()
        },
        "pi.post_tools",
        handlers.clone(),
    )
    .await;
    let input = send(&env, "tool:x").await;
    arrived(&gate, 1).await;
    assert_eq!(
        env.live_tasks(None)
            .await
            .unwrap()
            .iter()
            .map(|t| t.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["pi.post_tools"]
    );
    env.close(ctx()).await.unwrap();
    gate.open();
    let env = open_hook(
        OpenOptions {
            dir: Some(dir.path().to_owned()),
            tools: vec![tool("x", ToolOptions::default()).declaration()],
            ..Default::default()
        },
        "pi.post_tools",
        handlers,
    )
    .await;
    idle(&env).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(env.input(input.id).await.unwrap().unwrap().status, "done");
    assert_eq!(
        kinds(&env.entries(1).await.unwrap()),
        "user system assistant tool_result assistant"
    );
    let tasks = env.tasks(None).await.unwrap();
    let pt = tasks.iter().find(|t| t.kind == "pi.post_tools").unwrap();
    assert_eq!(
        tasks
            .iter()
            .filter(|t| t.kind == "pi.generation" && t.id > pt.id)
            .count(),
        1
    );
    env.close(ctx()).await.unwrap();
}

fn summary_script(messages: &[Value], call: usize) -> Response {
    if messages.iter().any(|m| {
        m["role"] == "user"
            && m["content"]
                .as_str()
                .is_some_and(|s| s.starts_with("Summarize"))
    }) {
        Response {
            text: Some("the summary".to_owned()),
            ..Default::default()
        }
    } else {
        echo_script(messages, call)
    }
}
#[tokio::test]
async fn summarizing_reopen_counts_interruption_retries_and_lands_a_summary_head() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Gate::new();
    gate.open();
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        models: Some(FakeModels::new(Arc::new(summary_script)).with_gate(gate.clone())),
        root_rewindable: Some(
            json!({"model":{"provider":"anthropic","modelId":"fake-1"},"keepRecent":10})
                .as_object()
                .unwrap()
                .clone(),
        ),
        root_sticky: Some(
            json!({"retry":{"enabled":true,"maxRetries":3,"baseDelayMs":1}})
                .as_object()
                .unwrap()
                .clone(),
        ),
        ..Default::default()
    })
    .await
    .unwrap();
    for text in ["one", "two", "three"] {
        settle(&send(&env, text).await).await;
    }
    gate.close();
    let id = env.root.collapse(None, ctx()).await.unwrap();
    until_phase(&env, "pi.collapse", Some("summarizing"))
        .await
        .unwrap();
    arrived(&gate, 4).await;
    env.close(ctx()).await.unwrap();
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        models: Some(FakeModels::new(Arc::new(summary_script))),
        ..Default::default()
    })
    .await
    .unwrap();
    let task = until_terminal(&env, id).await.unwrap();
    assert_eq!(outcome_status(&task), "completed", "{task:?}");
    let entries = env.entries(1).await.unwrap();
    let summary = entries.iter().find(|e| e.kind == "pi.summary").unwrap();
    assert_eq!(content_of(summary), "the summary ");
    assert!(summary.head.unwrap() > 0);
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn marked_task_reopen_aborts_without_reissuing_provider_request() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Gate::new();
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        models: Some(FakeModels::new(Arc::new(echo_script)).with_gate(gate.clone())),
        ..Default::default()
    })
    .await
    .unwrap();
    let input = send(&env, "A").await;
    let task = until_phase(&env, "pi.generation", Some("requesting"))
        .await
        .unwrap();
    arrived(&gate, 1).await;
    env.h.mark_task(task.id, ctx()).await.unwrap();
    env.close(ctx()).await.unwrap();
    let models = FakeModels::new(Arc::new(echo_script));
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        models: Some(models.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(
        outcome_status(&until_terminal(&env, task.id).await.unwrap()),
        "aborted"
    );
    assert_eq!(models.calls(), 0);
    assert_eq!(
        env.input(input.id)
            .await
            .unwrap()
            .unwrap()
            .reason
            .as_deref(),
        Some("aborted")
    );
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn memory_and_jsonl_produce_identical_transcript_and_task_outcome_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut results = vec![];
    for directory in [None, Some(dir.path().to_owned())] {
        let env = open(OpenOptions {
            dir: directory,
            tools: vec![tool("a", ToolOptions::default()).declaration()],
            ..Default::default()
        })
        .await
        .unwrap();
        for text in ["tool:a", "plain", "tool:a"] {
            settle(&send(&env, text).await).await;
        }
        idle(&env).await;
        results.push((
            kinds(&env.entries(1).await.unwrap()),
            env.tasks(None)
                .await
                .unwrap()
                .iter()
                .map(|t| (t.kind.clone(), outcome_status(t)))
                .collect::<Vec<_>>(),
        ));
        env.close(ctx()).await.unwrap();
    }
    assert_eq!(results[0], results[1]);
}

#[tokio::test]
async fn retained_terminal_tasks_keep_typed_outcomes_after_150_retirements_and_reopen_without_sidecars(
) {
    let dir = tempfile::tempdir().unwrap();
    let initial: PhaseFn = Arc::new(|task, _, _| {
        Box::pin(async move {
            Ok(Step::Done(Box::new(move |_, _, _| {
                Box::pin(async move { Ok(Completion::completed(task.input.clone())) })
            })))
        })
    });
    let kind: Arc<dyn Kind> = Arc::new(TaskKind::new(
        "retained.n",
        initial,
        HashMap::new(),
        Arc::new(|_, _, _| {
            Box::pin(async {
                Ok(abort_closure_from(|_, _, _| {
                    Box::pin(async { Ok(Value::Null) })
                }))
            })
        }),
    ));
    let metadata = kind.metadata();
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        task_kinds: vec![kind.clone()],
        ..Default::default()
    })
    .await
    .unwrap();
    let mut first = 0;
    for n in 1..=150 {
        let metadata = metadata.clone();
        let id = env
            .root
            .commit(
                move |tx, _| {
                    Box::pin(async move {
                        Ok(tx
                            .create_task_kind(
                                &metadata,
                                json!({"n":n}),
                                CreateTaskOptions {
                                    conversation_id: Some(1),
                                    background: true,
                                    after: vec![],
                                },
                            )?
                            .id)
                    })
                },
                ctx(),
            )
            .await
            .unwrap();
        if n == 1 {
            first = id;
        }
        until_terminal(&env, id).await.unwrap();
    }
    assert_eq!(
        result_of(&env.h.get_task(first, ctx()).await.unwrap().unwrap()),
        Some(json!({"n":1}))
    );
    assert_eq!(
        env.tasks(None)
            .await
            .unwrap()
            .iter()
            .filter(|t| t.status == TaskStatus::Terminal)
            .count(),
        150
    );
    let plugin_kind = env.h.builtin_kind("pi.plugin").unwrap();
    let plugin = env
        .root
        .commit(
            move |tx, _| {
                Box::pin(async move {
                    Ok(tx
                        .create_task_kind(
                            &plugin_kind,
                            json!({"handler":"none","input":null}),
                            CreateTaskOptions {
                                conversation_id: Some(1),
                                background: true,
                                after: vec![],
                            },
                        )?
                        .id)
                })
            },
            ctx(),
        )
        .await
        .unwrap();
    until_terminal(&env, plugin).await.unwrap();
    env.close(ctx()).await.unwrap();
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("task-")));
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        task_kinds: vec![kind],
        ..Default::default()
    })
    .await
    .unwrap();
    let first_task = env.h.get_task(first, ctx()).await.unwrap().unwrap();
    assert_eq!(result_of(&first_task), Some(json!({"n":1})));
    assert!(first_task.checkpoint.is_none());
    assert_eq!(
        env.h.wait_for_task(first, ctx()).await.unwrap().status,
        TaskStatus::Terminal
    );
    assert_eq!(
        outcome_status(&env.h.get_task(plugin, ctx()).await.unwrap().unwrap()),
        "failed"
    );
    assert_eq!(
        env.tasks(None)
            .await
            .unwrap()
            .iter()
            .filter(|t| t.kind == "retained.n")
            .count(),
        150
    );
    env.close(ctx()).await.unwrap();
}
