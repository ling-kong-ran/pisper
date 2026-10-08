//! Real scheduler ports of waiters.test.ts. CancellationToken intentionally
//! substitutes for AbortSignal: it has no arbitrary JS rejection-reason value.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::support_runtime::*;
use crate::agent_core::harness::context::with_abort_signal;
use crate::agent_core::harness::pico3::runtime::{
    abort_closure_from, AbortFn, Next, PhaseFn, Step, TaskKind,
};
use crate::agent_core::harness::pico3::session::CreateTaskOptions;
use crate::agent_core::harness::pico3::types::{Completion, SendInput, TaskStatus, UserInput};

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(8), future)
        .await
        .expect("waiter must not be stranded")
}

fn send(text: &str) -> SendInput {
    SendInput {
        content: UserInput::Text(text.to_owned()),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn atomic_waiter_registration_never_strands_task_input_or_idle_waiters() {
    let initial: PhaseFn = Arc::new(|_task, _rt, _ctx| {
        Box::pin(async {
            Ok(Step::Next(Next::Checkpoint(
                json!({ "phase": "x" }).as_object().unwrap().clone(),
            )))
        })
    });
    let phase: PhaseFn = Arc::new(|_task, _rt, _ctx| {
        Box::pin(async {
            Ok(Step::Done(Box::new(|_tx, _current, _ctx| {
                Box::pin(async { Ok(Completion::completed(Value::Null)) })
            })))
        })
    });
    let abort: AbortFn = Arc::new(|_task, _rt, _ctx| {
        Box::pin(async {
            Ok(abort_closure_from(|_tx, _current, _ctx| {
                Box::pin(async { Ok(Value::Null) })
            }))
        })
    });
    let quick = Arc::new(TaskKind::new(
        "quick",
        initial,
        HashMap::from([("x".to_owned(), phase)]),
        abort,
    ));
    let env2 = open(OpenOptions {
        task_kinds: vec![quick],
        ..Default::default()
    })
    .await
    .unwrap();
    let kind = env2.h.builtin_kind("quick").unwrap();
    for _ in 0..200 {
        let kind = kind.clone();
        let id = env2
            .root
            .commit(
                move |tx, _ctx| {
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
        let task = bounded(env2.h.wait_for_task(id, ctx())).await.unwrap();
        assert_eq!(task.status, TaskStatus::Terminal);
        assert_eq!(outcome_status(&task), "completed");
    }
    let first = env2.tasks(None).await.unwrap()[0].id;
    assert_eq!(
        bounded(env2.h.wait_for_task(first, ctx()))
            .await
            .unwrap()
            .status,
        TaskStatus::Terminal
    );
    env2.close(ctx()).await.unwrap();

    let env = open(OpenOptions::default()).await.unwrap();
    for i in 0..50 {
        let input = env.root.send(send(&format!("m{i}")), ctx()).await.unwrap();
        let (settled, conversation_idle, global_idle) = bounded(async {
            tokio::join!(
                input.wait(ctx()),
                env.root.wait_for_idle(ctx()),
                env.h.wait_for_idle(ctx())
            )
        })
        .await;
        assert_eq!(settled.unwrap().status, "done");
        conversation_idle.unwrap();
        global_idle.unwrap();
    }
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn cancellation_rejects_only_the_waiter_and_not_durable_work() {
    let gate = Gate::new();
    let env = open(OpenOptions {
        models: Some(FakeModels::new(Arc::new(echo_script)).with_gate(gate.clone())),
        ..Default::default()
    })
    .await
    .unwrap();
    let input = env.root.send(send("A"), ctx()).await.unwrap();
    let token = CancellationToken::new();
    let cctx = with_abort_signal(token.clone(), ctx());
    let generation = env
        .tasks(None)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.kind == "pi.generation")
        .unwrap();
    let cancel = async {
        gate.arrivals(1).await;
        token.cancel();
    };
    let (input_waiter, idle_waiter, task_waiter, global_waiter, ()) = bounded(async {
        tokio::join!(
            input.wait(cctx.clone()),
            env.root.wait_for_idle(cctx.clone()),
            env.h.wait_for_task(generation.id, cctx.clone()),
            env.h.wait_for_idle(cctx),
            cancel
        )
    })
    .await;
    assert!(input_waiter.is_err());
    assert!(idle_waiter.is_err());
    assert!(task_waiter.is_err());
    assert!(global_waiter.is_err());
    let dead = CancellationToken::new();
    dead.cancel();
    assert!(bounded(input.wait(with_abort_signal(dead, ctx())))
        .await
        .is_err());
    gate.open();
    assert_eq!(bounded(input.wait(ctx())).await.unwrap().status, "done");
    bounded(env.h.wait_for_idle(ctx())).await.unwrap();
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn global_idle_includes_a_just_created_task_before_dispatch() {
    let gate = Gate::new();
    let env = open(OpenOptions {
        models: Some(FakeModels::new(Arc::new(echo_script)).with_gate(gate.clone())),
        ..Default::default()
    })
    .await
    .unwrap();
    let input = env.root.send(send("A"), ctx()).await.unwrap();
    let idle = env.h.wait_for_idle(ctx());
    tokio::pin!(idle);
    tokio::select! {
        result = &mut idle => panic!("idle resolved before dispatch completed: {result:?}"),
        _ = gate.arrivals(1) => {},
    }
    assert!(futures::poll!(&mut idle).is_pending());
    gate.open();
    assert_eq!(bounded(input.wait(ctx())).await.unwrap().status, "done");
    bounded(idle).await.unwrap();
    assert!(env
        .tasks(None)
        .await
        .unwrap()
        .iter()
        .all(|t| t.status == TaskStatus::Terminal));
    env.close(ctx()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gate_fixture_cancellation_and_drop_leave_other_waiters_intact() {
    let gate = Gate::new();
    let token = CancellationToken::new();
    let cancelled = tokio::spawn({
        let gate = gate.clone();
        let token = token.clone();
        async move { gate.wait(with_abort_signal(token, ctx())).await }
    });
    let surviving = tokio::spawn({
        let gate = gate.clone();
        async move { gate.wait(ctx()).await }
    });
    let dropped = tokio::spawn({
        let gate = gate.clone();
        async move { gate.wait(ctx()).await }
    });
    gate.arrivals(3).await;
    token.cancel();
    assert!(bounded(cancelled).await.unwrap().is_err());
    dropped.abort();
    assert!(bounded(dropped).await.unwrap_err().is_cancelled());
    assert!(
        !surviving.is_finished(),
        "other callers must still wait for open"
    );
    gate.open();
    bounded(surviving).await.unwrap().unwrap();
    // Exercise the registration/open race on a multithreaded runtime.
    for _ in 0..200 {
        let gate = Gate::new();
        let waiting = tokio::spawn({
            let gate = gate.clone();
            async move { gate.wait(ctx()).await }
        });
        tokio::task::yield_now().await;
        gate.open();
        bounded(waiting).await.unwrap().unwrap();
    }
}
