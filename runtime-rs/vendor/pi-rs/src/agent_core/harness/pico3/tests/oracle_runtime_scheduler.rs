//! Scheduler lifecycle/admission oracles from spec-scheduler-process.test.ts.
use super::support_runtime::*;
use crate::agent_core::harness::pico3::runtime::{
    abort_closure_from, Kind, Next, PhaseFn, Step, TaskKind,
};
use crate::agent_core::harness::pico3::session::CreateTaskOptions;
use crate::agent_core::harness::pico3::types::{
    self, Completion, Input, NewEntry, SendInput, TaskStatus, UserInput,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
fn input(text: &str, when_busy: Option<&str>) -> SendInput {
    SendInput {
        content: UserInput::Text(text.to_owned()),
        when_busy: when_busy.map(str::to_owned),
        ..Default::default()
    }
}
async fn settle(i: &RootInput) -> Input {
    tokio::time::timeout(Duration::from_secs(8), i.wait(ctx()))
        .await
        .unwrap()
        .unwrap()
}
async fn idle(env: &Env) {
    tokio::time::timeout(Duration::from_secs(8), env.root.wait_for_idle(ctx()))
        .await
        .unwrap()
        .unwrap();
}
async fn arrived(gate: &Gate) {
    tokio::time::timeout(Duration::from_secs(8), gate.arrivals(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn terminal_generation_failure_drains_passive_writes_and_settles_successor_trigger_group() {
    let gate = Gate::new();
    let models = FakeModels::new(Arc::new(|_, _| Response {
        error: Some("provider down".into()),
        ..Default::default()
    }))
    .with_gate(gate.clone());
    let env=open(OpenOptions {models:Some(models.clone()),root_sticky:Some(json!({"retry":{"enabled":false,"maxRetries":0,"baseDelayMs":1},"followUpMode":"all","steeringMode":"all"}).as_object().unwrap().clone()),..Default::default()}).await.unwrap();
    let first = env.root.send(input("first", None), ctx()).await.unwrap();
    arrived(&gate).await;
    let a = env.root.send(input("follow-a", None), ctx()).await.unwrap();
    let steer = env
        .root
        .send(input("steer", Some("steer")), ctx())
        .await
        .unwrap();
    let b = env.root.send(input("follow-b", None), ctx()).await.unwrap();
    let passive = env
        .root
        .write(
            NewEntry {
                kind: "passive.note".to_owned(),
                data: Some(json!({"durable":true}).as_object().unwrap().clone()),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    env.root
        .config_reset(vec!["model".to_owned()], ctx())
        .await
        .unwrap();
    gate.open();
    for i in [&first, &a, &steer, &b] {
        let done = settle(i).await;
        assert_eq!(done.status, "unanswered");
        assert_eq!(done.reason.as_deref(), Some("failed"));
    }
    idle(&env).await;
    assert_eq!(env.input(passive).await.unwrap().unwrap().status, "done");
    assert_eq!(env.root.sticky(ctx()).await.unwrap()["inbox"], json!([]));
    let generations = env
        .tasks(None)
        .await
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == "pi.generation")
        .collect::<Vec<_>>();
    assert_eq!(generations.len(), 2);
    assert_eq!(
        generations
            .iter()
            .map(|t| failure_of(t).unwrap()["reason"].clone())
            .collect::<Vec<_>>(),
        vec![json!("provider"), json!("no_model")]
    );
    assert_eq!(models.calls(), 1);
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn provider_failure_display_stays_in_history_but_not_in_next_request() {
    let models = FakeModels::new(Arc::new(|messages, call| {
        if call == 0 {
            Response {
                error: Some("fatal".to_owned()),
                ..Default::default()
            }
        } else {
            echo_script(messages, call)
        }
    }));
    let env = open(OpenOptions {
        models: Some(models.clone()),
        root_sticky: Some(
            json!({"retry":{"enabled":false,"maxRetries":0,"baseDelayMs":1}})
                .as_object()
                .unwrap()
                .clone(),
        ),
        ..Default::default()
    })
    .await
    .unwrap();
    let first = env.root.send(input("first", None), ctx()).await.unwrap();
    assert_eq!(settle(&first).await.reason.as_deref(), Some("failed"));
    let entries = env.entries(1).await.unwrap();
    let display = entries
        .iter()
        .find(|e| {
            e.kind == "pi.assistant"
                && e.data
                    .as_ref()
                    .and_then(|d| d.get("reason"))
                    .and_then(Value::as_str)
                    == Some("error")
        })
        .unwrap();
    assert!(display.model.is_none());
    assert!(display.data.as_ref().unwrap().get("display").is_some());
    let second = env.root.send(input("second", None), ctx()).await.unwrap();
    assert_eq!(settle(&second).await.status, "done");
    let requests = models.requests();
    assert_eq!(requests.len(), 2);
    assert!(!serde_json::to_string(&requests[1])
        .unwrap()
        .contains("fatal"));
    assert!(!requests[1].iter().any(|m| m["role"] == "assistant"
        && (m["stopReason"] == "error" || m["stopReason"] == "aborted")));
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn concurrent_generation_admissions_are_singleton_but_retained_terminals_do_not_block_later_turns(
) {
    let gate = Gate::new();
    let env = open(OpenOptions {
        models: Some(FakeModels::new(Arc::new(echo_script)).with_gate(gate.clone())),
        ..Default::default()
    })
    .await
    .unwrap();
    let first = env.root.send(input("one", None), ctx()).await.unwrap();
    arrived(&gate).await;
    let (second, third) = tokio::join!(
        env.root.send(input("two", None), ctx()),
        env.root.send(input("three", None), ctx())
    );
    let second = second.unwrap();
    let third = third.unwrap();
    let active = env
        .live_tasks(None)
        .await
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == "pi.generation")
        .collect::<Vec<_>>();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].input["inputs"], json!([first.id]));
    gate.open();
    for i in [&first, &second, &third] {
        assert_eq!(settle(i).await.status, "done");
    }
    idle(&env).await;
    let retained = env
        .tasks(None)
        .await
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == "pi.generation")
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 3);
    assert!(retained.iter().all(|t| t.status == TaskStatus::Terminal));
    assert_eq!(
        settle(&env.root.send(input("four", None), ctx()).await.unwrap())
            .await
            .status,
        "done"
    );
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn collapse_overlap_rejects_before_persistence_and_terminal_retention_allows_recreation() {
    let gate = Gate::new();
    let env = open(OpenOptions {
        root_rewindable: Some(
            json!({"model":{"provider":"anthropic","modelId":"fake-1"},"keepRecent":1})
                .as_object()
                .unwrap()
                .clone(),
        ),
        ..Default::default()
    })
    .await
    .unwrap();
    let _off = install_collapse_hooks(
        &env,
        CollapseHandlers {
            before_collapse: Some(Arc::new({
                let gate = gate.clone();
                move |_, _, _, _, c| {
                    let gate = gate.clone();
                    Box::pin(async move {
                        gate.wait(c).await?;
                        Ok(None)
                    })
                }
            })),
        },
    )
    .unwrap();
    for prompt in ["one", "two", "three"] {
        assert_eq!(
            settle(&env.root.send(input(prompt, None), ctx()).await.unwrap())
                .await
                .status,
            "done"
        );
    }
    idle(&env).await;
    let first = env.root.collapse(None, ctx()).await.unwrap();
    arrived(&gate).await;
    let overlap = env.root.collapse(None, ctx()).await.unwrap_err();
    assert!(
        types::is_named(&overlap, types::COLLAPSE_IN_PROGRESS),
        "{overlap:?}"
    );
    assert_eq!(
        env.tasks(None)
            .await
            .unwrap()
            .iter()
            .filter(|t| t.kind == "pi.collapse")
            .count(),
        1
    );
    gate.open();
    let done = until_terminal(&env, first).await.unwrap();
    assert_eq!(outcome_status(&done), "completed", "{done:?}");
    assert_eq!(
        env.h.get_task(first, ctx()).await.unwrap().unwrap().id,
        first
    );
    // The second task may decline because little context remains, but admission
    // must succeed despite the retained first task's identical kind.
    let second = env.root.collapse(None, ctx()).await.unwrap();
    assert_ne!(second, first);
    until_terminal(&env, second).await.unwrap();
    env.close(ctx()).await.unwrap();
}

fn held_kind(label: &'static str, ran: Arc<Mutex<Vec<String>>>) -> Arc<dyn Kind> {
    let initial: PhaseFn = Arc::new(move |_, _, _| {
        ran.lock().unwrap().push(label.to_owned());
        Box::pin(async {
            Ok(Step::Next(Next::Checkpoint(
                json!({"phase":"done"}).as_object().unwrap().clone(),
            )))
        })
    });
    let done: PhaseFn = Arc::new(move |_, _, _| {
        Box::pin(async move {
            Ok(Step::Done(Box::new(move |_, _, _| {
                Box::pin(async move { Ok(Completion::completed(json!(label))) })
            })))
        })
    });
    Arc::new(TaskKind::new(
        "spec.held-kind",
        initial,
        HashMap::from([("done".to_owned(), done)]),
        Arc::new(|_, _, _| {
            Box::pin(async {
                Ok(abort_closure_from(|_, _, _| {
                    Box::pin(async { Ok(Value::Null) })
                }))
            })
        }),
    ))
}
#[tokio::test]
async fn nested_holds_allow_commits_while_pending_dispatch_uses_replacement_registration() {
    let env = open(OpenOptions::default()).await.unwrap();
    assert!(env.h.quiescent());
    let release1 = env.h.hold().unwrap();
    let release2 = env.h.hold().unwrap();
    let ran = Arc::new(Mutex::new(vec![]));
    let remove = env
        .h
        .register_task_kind(held_kind("old", ran.clone()))
        .unwrap();
    let kind = env.h.builtin_kind("spec.held-kind").unwrap();
    let id = env
        .root
        .commit(
            move |tx, _| {
                Box::pin(async move {
                    Ok(tx
                        .create_task_kind(
                            &kind,
                            Value::Null,
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
    env.root
        .write(
            NewEntry {
                kind: "commit.while.held".to_owned(),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    assert_eq!(
        env.h.get_task(id, ctx()).await.unwrap().unwrap().status,
        TaskStatus::Pending
    );
    assert!(ran.lock().unwrap().is_empty());
    remove();
    let _off = env
        .h
        .register_task_kind(held_kind("replacement", ran.clone()))
        .unwrap();
    release1();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        env.h.get_task(id, ctx()).await.unwrap().unwrap().status,
        TaskStatus::Pending
    );
    assert!(ran.lock().unwrap().is_empty());
    release2();
    assert_eq!(
        result_of(&until_terminal(&env, id).await.unwrap()),
        Some(json!("replacement"))
    );
    assert_eq!(*ran.lock().unwrap(), vec!["replacement"]);
    assert!(env.h.quiescent());
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn waiting_approval_is_not_quiescent_and_suspend_unwinds_without_terminalizing() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Gate::new();
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        tools: vec![tool("x", ToolOptions::default()).declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let _off = install_tool_hooks(
        &env,
        ToolHandlers {
            before_tool: Some(Arc::new({
                let gate = gate.clone();
                move |_, _, c| {
                    let gate = gate.clone();
                    Box::pin(async move {
                        gate.wait(c).await?;
                        Ok(None)
                    })
                }
            })),
            ..Default::default()
        },
    )
    .unwrap();
    let i = env.root.send(input("tool:x", None), ctx()).await.unwrap();
    arrived(&gate).await;
    assert!(!env.h.quiescent());
    let live = env.live_tasks(None).await.unwrap();
    let tool = live.iter().find(|t| t.kind == "pi.tool").unwrap();
    env.h.suspend(ctx()).await.unwrap();
    assert!(env.h.quiescent());
    // suspend closes the Session; inspect durable data through a fresh paused
    // harness, not through capabilities belonging to the closed instance.
    let reopened = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        paused: true,
        tools: vec![super::support_runtime::tool("x", ToolOptions::default()).declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let persisted = reopened.h.get_task(tool.id, ctx()).await.unwrap().unwrap();
    assert_eq!(persisted.status, TaskStatus::Running);
    assert!(persisted.outcome.is_none());
    assert!(persisted.abort.is_none());
    assert_eq!(
        reopened.input(i.id).await.unwrap().unwrap().status,
        "placed"
    );
    reopened.h.resume();
    idle(&reopened).await;
    assert_eq!(reopened.input(i.id).await.unwrap().unwrap().status, "done");
    reopened.close(ctx()).await.unwrap();
}
