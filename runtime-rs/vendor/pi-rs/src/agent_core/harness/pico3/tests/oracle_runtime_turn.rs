//! Ports of the runtime oracle halves deferred from Task 8:
//! `turn.test.ts` (the generation turn flows), plus focused cases from
//! `waiters.test.ts`, `tool-bounds.test.ts`, `recovery.test.ts`,
//! `spec-scheduler-process.test.ts`, `busy.test.ts` (§6 phase-map
//! contract), `kinds.test.ts` (collapse/fork/section/plugin flows), and
//! `subagent.test.ts`. See the tests.rs mapping table for the per-case
//! provenance and the disclosed coverage decisions.

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::runtime::{Kind, Step};
use crate::agent_core::harness::pico3::tests::support_runtime::*;
use crate::agent_core::harness::pico3::types::{AnyKind, Completion};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// `turn.test.ts` "one turn: user → generation → 2 tools → post_tools →
/// generation → final".
#[tokio::test]
async fn one_turn_user_generation_two_tools_post_tools_generation_final() {
    let a = tool("a", ToolOptions::default());
    let b = tool("b", ToolOptions::default());
    let env = open(OpenOptions {
        tools: vec![a.declaration(), b.declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let first = env
        .root
        .send(
            crate::agent_core::harness::pico3::types::SendInput {
                content: crate::agent_core::harness::pico3::types::UserInput::Text(
                    "tool:a,b".to_owned(),
                ),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    let r = first.wait(ctx()).await.unwrap();
    env.root.wait_for_idle(ctx()).await.unwrap();

    if r.status != "done" {
        for t in env.tasks(None).await.unwrap() {
            eprintln!(
                "task {} {} {:?} outcome={:?} cp={:?}",
                t.id, t.kind, t.status, t.outcome, t.checkpoint
            );
        }
        for e in env.entries(1).await.unwrap() {
            eprintln!("entry {} {} {}", e.id, e.kind, content_of(&e));
        }
    }
    assert_eq!(r.status, "done");
    assert_eq!(
        kinds(&env.entries(1).await.unwrap()),
        "user system assistant tool_result tool_result assistant"
    );
    let tasks = env.tasks(None).await.unwrap();
    let mut summary: Vec<String> = tasks
        .iter()
        .map(|t| {
            format!(
                "{}:{}:{}",
                t.kind,
                serde_json::to_value(t.status).unwrap(),
                outcome_status(t)
            )
        })
        .collect();
    summary.sort();
    assert_eq!(
        summary,
        vec![
            "pi.generation:\"terminal\":completed",
            "pi.generation:\"terminal\":completed",
            "pi.post_tools:\"terminal\":completed",
            "pi.tool:\"terminal\":completed",
            "pi.tool:\"terminal\":completed",
        ]
    );
    let answer = env
        .entries(1)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.id == r.answer.expect("answer"))
        .expect("answer entry");
    assert_eq!(
        content_of(&answer),
        json!([{ "type": "text", "text": "after tools " }]).to_string()
    );
    let sticky = env.root.sticky(ctx()).await.unwrap();
    assert_eq!(sticky.get("turn"), Some(&json!({ "tools": [] })));
    assert_eq!(sticky.get("tasks"), Some(&json!({})));
    env.close(ctx()).await.unwrap();
}

/// `turn.test.ts` "requestId dedup returns the original input and writes
/// nothing".
#[tokio::test]
async fn request_id_dedup_returns_original_input() {
    let env = open(OpenOptions::default()).await.unwrap();
    let send = |content: &str, request_id: Option<&str>| {
        crate::agent_core::harness::pico3::types::SendInput {
            content: crate::agent_core::harness::pico3::types::UserInput::Text(content.to_owned()),
            request_id: request_id.map(str::to_owned),
            when_busy: None,
        }
    };
    let a = env.root.send(send("hi", Some("k")), ctx()).await.unwrap();
    let b = env
        .root
        .send(send("other", Some("k")), ctx())
        .await
        .unwrap();
    assert_eq!(a.id, b.id);
    a.wait(ctx()).await.unwrap();
    let user_entries = env
        .entries(1)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "pi.user")
        .count();
    assert_eq!(user_entries, 1);
    env.close(ctx()).await.unwrap();
}

/// `turn.test.ts` "no model → failed/no_model, no provider call".
#[tokio::test]
async fn no_model_fails_without_provider_call() {
    let models = FakeModels::new(Arc::new(echo_script));
    let env = open(OpenOptions {
        models: Some(models.clone()),
        root_rewindable: Some(json!({}).as_object().cloned().unwrap()),
        ..Default::default()
    })
    .await
    .unwrap();
    let a = env
        .root
        .send(
            crate::agent_core::harness::pico3::types::SendInput {
                content: crate::agent_core::harness::pico3::types::UserInput::Text("A".to_owned()),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    let r = a.wait(ctx()).await.unwrap();
    assert_eq!(r.status, "unanswered");
    assert_eq!(r.reason.as_deref(), Some("failed"));
    let gen = env
        .tasks(None)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.kind == "pi.generation")
        .expect("generation");
    let failure = failure_of(&gen).expect("failure");
    assert_eq!(
        failure.get("reason").and_then(Value::as_str),
        Some("no_model")
    );
    assert_eq!(models.calls(), 0);
    env.close(ctx()).await.unwrap();
}

/// `turn.test.ts` "retry disabled → failed/provider immediately".
#[tokio::test]
async fn retry_disabled_fails_provider_immediately() {
    let models = FakeModels::new(Arc::new(|_messages, _call| Response {
        error: Some("boom".to_owned()),
        ..Default::default()
    }));
    let env = open(OpenOptions {
        models: Some(models.clone()),
        root_rewindable: Some(
            json!({ "model": { "provider": "anthropic", "modelId": "fake-1" } })
                .as_object()
                .cloned()
                .unwrap(),
        ),
        root_sticky: Some(
            json!({ "retry": { "enabled": false, "maxRetries": 0, "baseDelayMs": 1 } })
                .as_object()
                .cloned()
                .unwrap(),
        ),
        ..Default::default()
    })
    .await
    .unwrap();
    let a = env
        .root
        .send(
            crate::agent_core::harness::pico3::types::SendInput {
                content: crate::agent_core::harness::pico3::types::UserInput::Text("A".to_owned()),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    let r = a.wait(ctx()).await.unwrap();
    assert_eq!(r.status, "unanswered");
    let gen = env
        .tasks(None)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.kind == "pi.generation")
        .expect("generation");
    let failure = failure_of(&gen).expect("failure");
    assert_eq!(
        failure.get("reason").and_then(Value::as_str),
        Some("provider")
    );
    assert_eq!(models.calls(), 1);
    env.close(ctx()).await.unwrap();
}

/// `turn.test.ts` "retryable provider error: pi.usage recorded, retried,
/// then succeeds".
#[tokio::test]
async fn retryable_provider_error_retries_then_succeeds() {
    let counter = std::sync::atomic::AtomicUsize::new(0);
    let counter = std::sync::Arc::new(counter);
    let counter_for_respond = counter.clone();
    let models = FakeModels::new(Arc::new(move |messages, _call| {
        let n = counter_for_respond.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Response {
                error: Some("overloaded".to_owned()),
                ..Default::default()
            }
        } else {
            echo_script(messages, 1)
        }
    }));
    let env = open(OpenOptions {
        models: Some(models.clone()),
        root_rewindable: Some(
            json!({ "model": { "provider": "anthropic", "modelId": "fake-1" } })
                .as_object()
                .cloned()
                .unwrap(),
        ),
        root_sticky: Some(
            json!({ "retry": { "enabled": true, "maxRetries": 3, "baseDelayMs": 1 } })
                .as_object()
                .cloned()
                .unwrap(),
        ),
        ..Default::default()
    })
    .await
    .unwrap();
    let a = env
        .root
        .send(
            crate::agent_core::harness::pico3::types::SendInput {
                content: crate::agent_core::harness::pico3::types::UserInput::Text("A".to_owned()),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    let r = a.wait(ctx()).await.unwrap();
    if r.status != "done" {
        for t in env.tasks(None).await.unwrap() {
            eprintln!(
                "task {} {} {:?} outcome={:?} cp={:?}",
                t.id, t.kind, t.status, t.outcome, t.checkpoint
            );
        }
        for e in env.entries(1).await.unwrap() {
            eprintln!("entry {} {} {}", e.id, e.kind, content_of(&e));
        }
    }
    assert_eq!(r.status, "done");
    assert_eq!(
        kinds(&env.entries(1).await.unwrap()),
        "user system usage assistant"
    );
    assert_eq!(models.calls(), 2);
    let usage = &env.entries(1).await.unwrap()[2];
    assert_eq!(
        usage
            .data
            .as_ref()
            .and_then(|data| data.get("attempt"))
            .and_then(Value::as_i64),
        Some(1)
    );
    env.close(ctx()).await.unwrap();
}

/// `turn.test.ts` "InputHandle.abort: queued → aborted and removed; placed →
/// already_placed; unknown → not_found".
#[tokio::test]
async fn input_abort_lifecycle() {
    let gate = Gate::new();
    let models = FakeModels::new(Arc::new(echo_script)).with_gate(gate.clone());
    let env = open(OpenOptions {
        models: Some(models),
        ..Default::default()
    })
    .await
    .unwrap();
    let a = env
        .root
        .send(
            crate::agent_core::harness::pico3::types::SendInput {
                content: crate::agent_core::harness::pico3::types::UserInput::Text("A".to_owned()),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    gate.arrivals(1).await;
    let f = env
        .root
        .send(
            crate::agent_core::harness::pico3::types::SendInput {
                content: crate::agent_core::harness::pico3::types::UserInput::Text("F".to_owned()),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    assert_eq!(f.abort(ctx()).await.unwrap(), "aborted");
    let input = env.input(f.id).await.unwrap().expect("input");
    assert_eq!(input.status, "unanswered");
    let sticky = env.root.sticky(ctx()).await.unwrap();
    let inbox = sticky
        .get("inbox")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(inbox.is_empty());
    assert_eq!(a.abort(ctx()).await.unwrap(), "already_placed");
    assert!(env.input(99_999).await.unwrap().is_none());
    gate.open();
    a.wait(ctx()).await.unwrap();
    env.close(ctx()).await.unwrap();
}

/// `busy.test.ts` §6 "phase roles: a `next` transition into an in-flight
/// phase is a contract fault → outcome faulted".
#[tokio::test]
async fn transition_into_inflight_phase_faults() {
    use crate::agent_core::harness::pico3::runtime::{Next, PhaseFn, TaskKind};
    use std::collections::HashMap;

    let initial: PhaseFn = Arc::new(|_task, _rt, _ctx| {
        Box::pin(async {
            Ok(Step::Next(Next::Checkpoint(
                json!({ "phase": "fly" }).as_object().cloned().unwrap(),
            )))
        })
    });
    let fly: PhaseFn = Arc::new(|_task, _rt, _ctx| {
        Box::pin(async {
            Ok(Step::Done(Box::new(|_tx, _current, _ctx| {
                Box::pin(async {
                    Ok(Completion {
                        status: "failed".to_owned(),
                        result: None,
                        failure: Some(json!({ "reason": "declared" })),
                    })
                })
            })))
        })
    });
    let mut phases = HashMap::new();
    phases.insert("fly".to_owned(), fly);
    let abort: crate::agent_core::harness::pico3::runtime::AbortFn =
        Arc::new(|_task, _rt, _ctx| {
            Box::pin(async {
                Ok(
                    crate::agent_core::harness::pico3::runtime::abort_closure_from(
                        |_tx, _current, _ctx| Box::pin(async { Ok(Value::Null) }),
                    ),
                )
            })
        });
    let metadata: Arc<dyn AnyKind> = Arc::new(
        crate::agent_core::harness::pico3::types::BasicKind::new("roles")
            .inflight(vec!["fly".to_owned()]),
    );
    let kind: Arc<dyn Kind> =
        Arc::new(TaskKind::new("roles", initial, phases, abort).with_metadata(metadata));
    let env = open(OpenOptions {
        task_kinds: vec![kind],
        ..Default::default()
    })
    .await
    .unwrap();
    let reference = env
        .root
        .commit_kernel(
            {
                let kind = env.h.builtin_kind("roles").expect("registered");
                move |tx: &mut crate::agent_core::harness::pico3::session::Tx, _ctx: Context| {
                    let kind = kind.clone();
                    Box::pin(async move {
                        let reference = tx.create_task_kind(
                            &kind,
                            Value::Null,
                            crate::agent_core::harness::pico3::session::CreateTaskOptions {
                                conversation_id: Some(1),
                                background: true,
                                after: Vec::new(),
                            },
                        )?;
                        Ok(reference.id)
                    })
                }
            },
            ctx(),
        )
        .await
        .unwrap();
    let task = env.h.wait_for_task(reference, ctx()).await.unwrap();
    assert_eq!(outcome_status(&task), "faulted");
    let error = task
        .outcome
        .as_ref()
        .and_then(|outcome| outcome.error.clone())
        .expect("error");
    assert!(error.contains("in-flight phase fly"), "{error}");
    env.close(ctx()).await.unwrap();
}
