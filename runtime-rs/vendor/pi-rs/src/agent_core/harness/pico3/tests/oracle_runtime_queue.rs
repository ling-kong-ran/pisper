//! Remaining turn.test.ts runtime oracles: queue boundaries, controls,
//! retries, continuation, overflow, and the JSONL backend. These exercise
//! the real scheduler rather than Session-only invokers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::support_runtime::*;
use crate::agent_core::harness::pico3::harness::{Harness, HarnessOptions};
use crate::agent_core::harness::pico3::memory::MemoryStorage;
use crate::agent_core::harness::pico3::system::effective_tools;
use crate::agent_core::harness::pico3::types::{Input, NewEntry, SendInput, UserInput};

fn send(content: &str, when_busy: Option<&str>) -> SendInput {
    SendInput {
        content: UserInput::Text(content.to_owned()),
        when_busy: when_busy.map(str::to_owned),
        ..Default::default()
    }
}

async fn settled(input: &RootInput) -> Input {
    tokio::time::timeout(Duration::from_secs(8), input.wait(ctx()))
        .await
        .expect("input waiter must not be stranded")
        .expect("input settlement")
}

async fn idle(env: &Env) {
    tokio::time::timeout(Duration::from_secs(8), env.root.wait_for_idle(ctx()))
        .await
        .expect("conversation must become idle")
        .expect("idle waiter");
}

fn notice(text: &str) -> NewEntry {
    NewEntry {
        kind: "pi.notice".to_owned(),
        model: Some(vec![
            json!({"role": "user", "content": text, "timestamp": 0}),
        ]),
        ..Default::default()
    }
}

/// turn.test.ts: the complete two-tool turn on durable storage.
#[tokio::test]
async fn jsonl_turn_preserves_call_order_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let a = tool("a", ToolOptions::default());
    let b = tool("b", ToolOptions::default());
    let env = open(OpenOptions {
        dir: Some(dir.path().to_path_buf()),
        tools: vec![a.declaration(), b.declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let input = env.root.send(send("tool:a,b", None), ctx()).await.unwrap();
    let result = settled(&input).await;
    idle(&env).await;
    assert_eq!(result.status, "done");
    assert_eq!((a.calls(), b.calls()), (1, 1));
    let entries = env.entries(1).await.unwrap();
    assert_eq!(
        kinds(&entries),
        "user system assistant tool_result tool_result assistant"
    );
    let context = env.root.context(ctx()).await.unwrap();
    assert_eq!(
        context
            .messages
            .iter()
            .map(|message| message["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "user",
            "system",
            "assistant",
            "toolResult",
            "toolResult",
            "assistant"
        ]
    );
    assert_eq!(context.messages[3]["toolName"], "a");
    assert_eq!(context.messages[4]["toolName"], "b");
    env.close(ctx()).await.unwrap();

    let reopened = open(OpenOptions {
        dir: Some(dir.path().to_path_buf()),
        tools: vec![a.declaration(), b.declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(reopened.input(input.id).await.unwrap(), Some(result));
    assert_eq!(reopened.entries(1).await.unwrap(), entries);
    assert_eq!(
        (a.calls(), b.calls()),
        (1, 1),
        "completed tools must not replay"
    );
    reopened.close(ctx()).await.unwrap();
}

/// turn.test.ts: whenBusy followUp/steer/reject and successor grouping.
#[tokio::test]
async fn busy_modes_queue_or_reject_and_settle_the_successor_group() {
    let gate = Gate::new();
    let env = open(OpenOptions {
        models: Some(FakeModels::new(Arc::new(echo_script)).with_gate(gate.clone())),
        ..Default::default()
    })
    .await
    .unwrap();
    let first = env.root.send(send("A", None), ctx()).await.unwrap();
    gate.arrivals(1).await;
    let follow = env.root.send(send("F", None), ctx()).await.unwrap();
    let steer = env
        .root
        .send(send("S", Some("steer")), ctx())
        .await
        .unwrap();
    let rejected = env.root.send(send("R", Some("reject")), ctx()).await;
    assert!(rejected
        .err()
        .unwrap()
        .to_string()
        .to_lowercase()
        .contains("busy"));
    assert_eq!(
        env.input(follow.id).await.unwrap().unwrap().status,
        "queued"
    );
    assert_eq!(env.input(steer.id).await.unwrap().unwrap().status, "queued");
    let sticky = env.root.sticky(ctx()).await.unwrap();
    assert_eq!(
        sticky["inbox"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["mode"].clone())
            .collect::<Vec<_>>(),
        vec![json!("followUp"), json!("steer")]
    );
    gate.open();
    assert_eq!(settled(&first).await.status, "done");
    idle(&env).await;
    assert_eq!(settled(&follow).await.status, "done");
    assert_eq!(settled(&steer).await.status, "done");
    assert_eq!(
        kinds(&env.entries(1).await.unwrap()),
        "user system assistant user user assistant"
    );
    env.close(ctx()).await.unwrap();
}

/// turn.test.ts: steer joins at post-tools, followUp waits for a final answer.
#[tokio::test]
async fn steer_joins_active_inputs_but_follow_up_gets_a_separate_answer() {
    let gate = Gate::new();
    let t = tool(
        "a",
        ToolOptions {
            gate: Some(gate.clone()),
            ..Default::default()
        },
    );
    let env = open(OpenOptions {
        tools: vec![t.declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let first = env.root.send(send("tool:a", None), ctx()).await.unwrap();
    gate.arrivals(1).await;
    let steer = env
        .root
        .send(send("S", Some("steer")), ctx())
        .await
        .unwrap();
    let follow = env.root.send(send("F", None), ctx()).await.unwrap();
    gate.open();
    let first = settled(&first).await;
    idle(&env).await;
    let steer = settled(&steer).await;
    let follow = settled(&follow).await;
    assert_eq!(
        kinds(&env.entries(1).await.unwrap()),
        "user system assistant tool_result user assistant user assistant"
    );
    assert_eq!(
        (&steer.status, &follow.status),
        (&"done".to_owned(), &"done".to_owned())
    );
    assert!(steer.entry < follow.entry);
    assert_eq!(first.answer, steer.answer);
    assert_ne!(steer.answer, follow.answer);
    env.close(ctx()).await.unwrap();
}

/// turn.test.ts: one-at-a-time and all followUp policies.
#[tokio::test]
async fn follow_up_mode_controls_generation_input_groups() {
    for mode in ["one-at-a-time", "all"] {
        let gate = Gate::new();
        let env = open(OpenOptions {
            models: Some(FakeModels::new(Arc::new(echo_script)).with_gate(gate.clone())),
            root_sticky: Some(json!({"followUpMode": mode}).as_object().unwrap().clone()),
            ..Default::default()
        })
        .await
        .unwrap();
        let first = env.root.send(send("A", None), ctx()).await.unwrap();
        gate.arrivals(1).await;
        let f1 = env.root.send(send("F1", None), ctx()).await.unwrap();
        let f2 = env.root.send(send("F2", None), ctx()).await.unwrap();
        gate.open();
        settled(&first).await;
        idle(&env).await;
        let tasks = env.tasks(None).await.unwrap();
        let gens: Vec<_> = tasks.iter().filter(|t| t.kind == "pi.generation").collect();
        if mode == "all" {
            assert_eq!(gens.len(), 2);
            assert_eq!(gens[1].input["inputs"], json!([f1.id, f2.id]));
        } else {
            assert_eq!(gens.len(), 3);
            assert_eq!(gens[1].input["inputs"], json!([f1.id]));
            assert_eq!(gens[2].input["inputs"], json!([f2.id]));
        }
        assert_eq!(settled(&f2).await.status, "done");
        env.close(ctx()).await.unwrap();
    }
}

/// turn.test.ts: writes append idle, queue busy, and never own an answer.
#[tokio::test]
async fn writes_place_at_boundaries_without_creating_generation_answers() {
    let gate = Gate::new();
    let env = open(OpenOptions {
        models: Some(FakeModels::new(Arc::new(echo_script)).with_gate(gate.clone())),
        ..Default::default()
    })
    .await
    .unwrap();
    let w0 = env.root.write(notice("idle note"), ctx()).await.unwrap();
    assert_eq!(env.input(w0).await.unwrap().unwrap().status, "done");
    let first = env.root.send(send("A", None), ctx()).await.unwrap();
    gate.arrivals(1).await;
    let w1 = env.root.write(notice("busy note"), ctx()).await.unwrap();
    assert_eq!(env.input(w1).await.unwrap().unwrap().status, "queued");
    gate.open();
    settled(&first).await;
    idle(&env).await;
    let result = env.input(w1).await.unwrap().unwrap();
    assert_eq!(result.status, "done");
    assert_eq!(result.answer, None);
    assert_eq!(
        kinds(&env.entries(1).await.unwrap()),
        "notice user system assistant notice"
    );
    env.close(ctx()).await.unwrap();
}

/// turn.test.ts: reset invalidates earlier queued inputs, not later arrivals.
#[tokio::test]
async fn reset_stales_older_queue_and_starts_later_inputs_in_fresh_context() {
    let gate = Gate::new();
    let env = open(OpenOptions {
        models: Some(FakeModels::new(Arc::new(echo_script)).with_gate(gate.clone())),
        ..Default::default()
    })
    .await
    .unwrap();
    let first = env.root.send(send("A", None), ctx()).await.unwrap();
    gate.arrivals(1).await;
    let b1 = env.root.send(send("B1", None), ctx()).await.unwrap();
    let b2 = env.root.send(send("B2", None), ctx()).await.unwrap();
    env.root.reset(None, ctx()).await.unwrap();
    let c = env.root.send(send("C", None), ctx()).await.unwrap();
    gate.open();
    settled(&first).await;
    assert_eq!(settled(&c).await.status, "done");
    idle(&env).await;
    assert_eq!(settled(&b1).await.reason.as_deref(), Some("stale"));
    assert_eq!(settled(&b2).await.reason.as_deref(), Some("stale"));
    assert_eq!(
        kinds(&env.root.context(ctx()).await.unwrap().entries),
        "reset* user system assistant"
    );
    env.close(ctx()).await.unwrap();
}

/// turn.test.ts: handoff control ends the turn and moves followUp to a new head.
#[tokio::test]
async fn handoff_tool_control_starts_follow_up_after_the_new_head() {
    let gate = Gate::new();
    let hand = tool(
        "hand",
        ToolOptions {
            gate: Some(gate.clone()),
            control: Some(json!({"handoff": "fresh"})),
            ..Default::default()
        },
    );
    let env = open(OpenOptions {
        tools: vec![hand.declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let first = env.root.send(send("tool:hand", None), ctx()).await.unwrap();
    gate.arrivals(1).await;
    let follow = env.root.send(send("F", None), ctx()).await.unwrap();
    gate.open();
    let first_result = settled(&first).await;
    settled(&follow).await;
    idle(&env).await;
    let post = env
        .tasks(None)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.kind == "pi.post_tools")
        .unwrap();
    assert_eq!(result_of(&post).unwrap()["ended"], "handoff");
    assert_eq!(first_result.status, "done");
    assert_eq!(
        first_result.answer,
        env.entries(1)
            .await
            .unwrap()
            .iter()
            .find(|e| e.kind == "pi.assistant")
            .map(|e| e.id)
    );
    assert_eq!(
        kinds(&env.root.context(ctx()).await.unwrap().entries),
        "handoff* user system assistant"
    );
    env.close(ctx()).await.unwrap();
}

/// turn.test.ts: terminate does not create a continuation or a new head.
#[tokio::test]
async fn terminate_tool_control_finishes_without_continuation() {
    let t = tool(
        "t",
        ToolOptions {
            control: Some(json!({"terminate": true})),
            ..Default::default()
        },
    );
    let env = open(OpenOptions {
        tools: vec![t.declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let input = env.root.send(send("tool:t", None), ctx()).await.unwrap();
    assert_eq!(settled(&input).await.status, "done");
    idle(&env).await;
    assert_eq!(
        kinds(&env.entries(1).await.unwrap()),
        "user system assistant tool_result"
    );
    assert_eq!(
        env.tasks(None)
            .await
            .unwrap()
            .iter()
            .filter(|t| t.kind == "pi.generation")
            .count(),
        1
    );
    env.close(ctx()).await.unwrap();
}

/// turn.test.ts: addTools changes the loadout and publishes only the delta.
#[tokio::test]
async fn add_tools_control_extends_loadout_before_continuation() {
    let t = tool(
        "t",
        ToolOptions {
            control: Some(json!({"addTools": ["extra"]})),
            ..Default::default()
        },
    );
    let extra = tool("extra", ToolOptions::default());
    let env = open(OpenOptions {
        tools: vec![t.declaration(), extra.declaration()],
        root_rewindable: Some(
            json!({"model": model(), "selectedTools": ["t"]})
                .as_object()
                .unwrap()
                .clone(),
        ),
        ..Default::default()
    })
    .await
    .unwrap();
    let input = env.root.send(send("tool:t", None), ctx()).await.unwrap();
    assert_eq!(settled(&input).await.status, "done");
    idle(&env).await;
    assert_eq!(
        env.root.rewindable(ctx()).await.unwrap()["selectedTools"],
        json!(["t", "extra"])
    );
    let entries = env.entries(1).await.unwrap();
    let system: Vec<_> = entries.iter().filter(|e| e.kind == "pi.system").collect();
    assert_eq!(system.len(), 2);
    let delta = &system[1].model.as_ref().unwrap()[0];
    assert_eq!(
        delta["toolsAdded"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].clone())
            .collect::<Vec<_>>(),
        vec![json!("extra")]
    );
    assert!(delta.get("toolsRemoved").is_none());
    let context = env.root.context(ctx()).await.unwrap();
    assert_eq!(
        effective_tools(&context.messages)
            .iter()
            .map(|t| t["name"].clone())
            .collect::<Vec<_>>(),
        vec![json!("t"), json!("extra")]
    );
    env.close(ctx()).await.unwrap();
}

/// turn.test.ts: exhausted retries show an error without contaminating context.
#[tokio::test]
async fn retries_exhausted_settle_inputs_and_exclude_display_only_assistant() {
    let models = FakeModels::new(Arc::new(|_, _| Response {
        error: Some("overloaded".to_owned()),
        ..Default::default()
    }));
    let env = open(OpenOptions {
        models: Some(models.clone()),
        root_sticky: Some(
            json!({"retry": {"enabled": true, "maxRetries": 1, "baseDelayMs": 1}})
                .as_object()
                .unwrap()
                .clone(),
        ),
        ..Default::default()
    })
    .await
    .unwrap();
    let input = env.root.send(send("A", None), ctx()).await.unwrap();
    let result = settled(&input).await;
    idle(&env).await;
    assert_eq!(
        (result.status.as_str(), result.reason.as_deref()),
        ("unanswered", Some("failed"))
    );
    let generation = env
        .tasks(None)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.kind == "pi.generation")
        .unwrap();
    assert_eq!(
        failure_of(&generation).unwrap()["reason"],
        "retries_exhausted"
    );
    assert_eq!(models.calls(), 2);
    let entries = env.entries(1).await.unwrap();
    assert_eq!(kinds(&entries), "user system usage assistant");
    assert!(!env
        .root
        .context(ctx())
        .await
        .unwrap()
        .messages
        .iter()
        .any(|m| m["role"] == "assistant"));
    let shown = entries.iter().find(|e| e.kind == "pi.assistant").unwrap();
    assert!(shown.model.is_none());
    assert_eq!(shown.data.as_ref().unwrap()["reason"], "error");
    env.close(ctx()).await.unwrap();
}

/// turn.test.ts: onYield continuation reuses the input group and final answer.
#[tokio::test]
async fn yield_hook_continuation_reuses_the_original_input_group() {
    let yielded = Arc::new(AtomicUsize::new(0));
    let env = open(OpenOptions::default()).await.unwrap();
    let counter = yielded.clone();
    let remove = install_generation_hooks(
        &env,
        GenerationHandlers {
            on_yield: Some(Arc::new(move |_, _, _| {
                let continuation =
                    (counter.fetch_add(1, Ordering::SeqCst) == 0).then(|| "keep going".to_owned());
                Box::pin(async move { Ok(continuation) })
            })),
            ..Default::default()
        },
    )
    .unwrap();
    let input = env.root.send(send("A", None), ctx()).await.unwrap();
    let result = settled(&input).await;
    idle(&env).await;
    let entries = env.entries(1).await.unwrap();
    assert_eq!(kinds(&entries), "user system assistant user assistant");
    assert_eq!(
        entries[3].data,
        Some(
            json!({"continuation": true, "from": entries[2].id})
                .as_object()
                .unwrap()
                .clone()
        )
    );
    let tasks = env.tasks(None).await.unwrap();
    let groups: Vec<_> = tasks
        .iter()
        .filter(|t| t.kind == "pi.generation")
        .map(|t| t.input["inputs"].clone())
        .collect();
    assert_eq!(groups, vec![json!([input.id]), json!([input.id])]);
    assert_eq!(result.answer, Some(entries[4].id));
    assert_eq!(yielded.load(Ordering::SeqCst), 2);
    remove();
    env.close(ctx()).await.unwrap();
}

/// turn.test.ts: overflow inserts collapse and a replacement generation.
#[tokio::test]
async fn overflow_collapses_then_retries_the_same_input_group() {
    let models = FakeModels::new(Arc::new(|messages, call| {
        if messages.iter().any(|m| {
            m["role"] == "user"
                && m["content"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("Summarize"))
        }) {
            Response {
                text: Some("summary text".to_owned()),
                ..Default::default()
            }
        } else {
            echo_script(messages, call)
        }
    }));
    let env = open(OpenOptions {
        models: Some(models.clone()),
        root_rewindable: Some(
            json!({"model": model(), "keepRecent": 100})
                .as_object()
                .unwrap()
                .clone(),
        ),
        ..Default::default()
    })
    .await
    .unwrap();
    for content in ["one", "two", "three"] {
        let input = env.root.send(send(content, None), ctx()).await.unwrap();
        assert_eq!(settled(&input).await.status, "done");
    }
    models.set_window(200, 10);
    let input = env.root.send(send("four", None), ctx()).await.unwrap();
    let result = settled(&input).await;
    idle(&env).await;
    let tasks = env.tasks(None).await.unwrap();
    let collapse = tasks
        .iter()
        .find(|t| t.kind == "pi.collapse")
        .expect("overflow creates collapse");
    assert_eq!(collapse.input["reason"], "overflow");
    let failed = tasks
        .iter()
        .find(|t| t.kind == "pi.generation" && outcome_status(t) == "failed")
        .unwrap();
    assert_eq!(failure_of(failed).unwrap()["reason"], "overflow");
    let replacement = tasks
        .iter()
        .find(|t| t.kind == "pi.generation" && t.after.contains(&collapse.id))
        .unwrap();
    assert_eq!(replacement.input["inputs"], json!([input.id]));
    assert_eq!(result.status, "done");
    assert!(env
        .entries(1)
        .await
        .unwrap()
        .iter()
        .any(|e| e.kind == "pi.summary"));
    env.close(ctx()).await.unwrap();
}

/// harness.ts reset(): the handoff message uses the injected harness clock.
#[tokio::test]
async fn reset_handoff_uses_the_injected_clock() {
    let mut options = HarnessOptions::new(FakeModels::new(Arc::new(echo_script)));
    options.now = Some(Arc::new(|| 123_456_789));
    let harness = Harness::open(Arc::new(MemoryStorage::new()), options, ctx())
        .await
        .unwrap();
    let root = harness.conversation(1, ctx()).await.unwrap().unwrap();
    root.reset(Some("continue here".to_owned()), ctx())
        .await
        .unwrap();
    let context = root.context(ctx()).await.unwrap();
    assert_eq!(kinds(&context.entries), "handoff*");
    assert_eq!(
        context.messages,
        vec![json!({"role": "user", "content": "continue here", "timestamp": 123_456_789})]
    );
    harness.close(ctx()).await.unwrap();
}

/// turn.test.ts: stream abort retains a display-only partial, withdraws
/// pending steering, and leaves queued writes for the next idle send.
#[tokio::test]
async fn abort_mid_stream_preserves_partial_and_queued_writes() {
    let gate = Gate::new();
    let models = FakeModels::new(Arc::new(|messages, call| {
        if call == 0 {
            Response {
                text: Some("partial ".repeat(100)),
                ..Default::default()
            }
        } else {
            echo_script(messages, call)
        }
    }));
    let models =
        Arc::new(Arc::try_unwrap(models).ok().unwrap().with_token_delay(5)).with_gate(gate.clone());
    let env = open(OpenOptions {
        models: Some(models),
        ..Default::default()
    })
    .await
    .unwrap();
    let a = env.root.send(send("A", None), ctx()).await.unwrap();
    gate.arrivals(1).await;
    let s = env
        .root
        .send(send("S", Some("steer")), ctx())
        .await
        .unwrap();
    let w = env.root.write(notice("n"), ctx()).await.unwrap();
    gate.open();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let sticky = env.root.sticky(ctx()).await.unwrap();
            if sticky
                .get("turn")
                .and_then(|v| v.get("message"))
                .and_then(|v| v.get("content"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|parts| {
                    parts.iter().any(|part| {
                        part.get("type") == Some(&json!("text"))
                            && part
                                .get("text")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|s| !s.is_empty())
                    })
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("abort after at least one streamed token");
    tokio::time::timeout(Duration::from_secs(8), env.root.abort(ctx()))
        .await
        .expect("abort settles")
        .unwrap();
    assert_eq!(
        env.input(a.id).await.unwrap().unwrap().reason.as_deref(),
        Some("aborted")
    );
    assert_eq!(
        env.input(s.id).await.unwrap().unwrap().reason.as_deref(),
        Some("aborted")
    );
    assert_eq!(env.input(w).await.unwrap().unwrap().status, "queued");
    let generation = env
        .tasks(None)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.kind == "pi.generation")
        .unwrap();
    assert_eq!(outcome_status(&generation), "aborted");
    let entries = env.entries(1).await.unwrap();
    let partial = entries
        .iter()
        .find(|e| {
            e.kind == "pi.assistant"
                && e.data.as_ref().and_then(|d| d.get("reason")) == Some(&json!("aborted"))
        })
        .expect("partial assistant entry");
    assert!(partial.model.is_none());
    assert!(partial
        .data
        .as_ref()
        .unwrap()
        .get("display")
        .and_then(|v| v.get("content"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|content| !content.is_empty()));
    let sticky = env.root.sticky(ctx()).await.unwrap();
    let modes: Vec<_> = sticky["inbox"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["mode"].clone())
        .collect();
    assert_eq!(modes, vec![json!("write")]);
    let b = env.root.send(send("B", None), ctx()).await.unwrap();
    assert_eq!(settled(&b).await.status, "done");
    assert_eq!(
        kinds(&env.entries(1).await.unwrap()),
        "user system assistant notice user assistant"
    );
    env.close(ctx()).await.unwrap();
}
