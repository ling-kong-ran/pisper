//! Runtime memo and approval oracles from spec-plugins-lifecycle.test.ts.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::support_runtime::*;
use crate::agent_core::harness::pico3::runtime::{ToolDeclaration, ToolExecuteFn, ToolResult};
use crate::agent_core::harness::pico3::types::{SendInput, UserInput};

fn declaration(name: &str, execute: ToolExecuteFn) -> Arc<ToolDeclaration> {
    Arc::new(ToolDeclaration {
        name: name.to_owned(),
        description: String::new(),
        parameters: json!({"type":"object", "properties":{"v":{"type":"string"}}, "required":["v"]}),
        replay: Some("safe".to_owned()),
        output: None,
        execute,
    })
}

async fn send_tool(env: &Env, name: &str) -> RootInput {
    env.root
        .send(
            SendInput {
                content: UserInput::Text(format!("tool:{name}")),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap()
}

async fn settle(input: &RootInput) {
    let result = tokio::time::timeout(Duration::from_secs(8), input.wait(ctx()))
        .await
        .expect("input must settle")
        .unwrap();
    assert_eq!(result.status, "done");
}

#[tokio::test]
async fn tool_memo_read_is_not_a_write_and_null_is_a_durable_winner() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let tool = declaration(
        "memo-tool",
        Arc::new({
            let observed = observed.clone();
            move |_args, api, context| {
                let observed = observed.clone();
                Box::pin(async move {
                    let unset = api.memo("winner", None, context.clone()).await?;
                    let (first, second) = tokio::join!(
                        api.memo("winner", Some(json!("first")), context.clone()),
                        api.memo("winner", Some(json!("second")), context.clone())
                    );
                    let stored = api.memo("winner", None, context.clone()).await?;
                    let null = api.memo("null", Some(Value::Null), context.clone()).await?;
                    let still_null = api
                        .memo("null", Some(json!("later")), context.clone())
                        .await?;
                    let read_null = api.memo("null", None, context).await?;
                    observed.lock().unwrap().push(vec![
                        unset, first?, second?, stored, null, still_null, read_null,
                    ]);
                    Ok(ToolResult {
                        content: Some(vec![json!({"type":"text", "text":"memo done"})]),
                        ..Default::default()
                    })
                })
            }
        }),
    );
    let env = open(OpenOptions {
        tools: vec![tool],
        ..Default::default()
    })
    .await
    .unwrap();
    settle(&send_tool(&env, "memo-tool").await).await;
    settle(&send_tool(&env, "memo-tool").await).await;
    let expected = vec![
        None,
        Some(json!("first")),
        Some(json!("first")),
        Some(json!("first")),
        Some(Value::Null),
        Some(Value::Null),
        Some(Value::Null),
    ];
    assert_eq!(*observed.lock().unwrap(), vec![expected.clone(), expected]);
    let tasks = env.tasks(None).await.unwrap();
    let tools = tasks
        .iter()
        .filter(|task| task.kind == "pi.tool")
        .collect::<Vec<_>>();
    assert_eq!(tools.len(), 2);
    let sticky = env.root.sticky(ctx()).await.unwrap();
    for task in tools {
        assert!(
            sticky
                .get("tasks")
                .and_then(|v| v.get(task.id.to_string()))
                .is_none(),
            "terminal memos retired"
        );
    }
    env.close(ctx()).await.unwrap();
}

async fn arrived(gate: &Gate) {
    tokio::time::timeout(Duration::from_secs(8), gate.arrivals(1))
        .await
        .expect("gate must be reached");
}

async fn idle(env: &Env) {
    tokio::time::timeout(Duration::from_secs(8), env.root.wait_for_idle(ctx()))
        .await
        .expect("idle")
        .unwrap();
}

async fn watch(
    env: &Env,
) -> (
    Arc<crate::agent_core::harness::pico3::view::Watch>,
    Arc<Mutex<Vec<Value>>>,
) {
    let watch = env.root.watch(ctx()).await.unwrap();
    let envelopes = Arc::new(Mutex::new(Vec::new()));
    watch.start(Arc::new({
        let envelopes = envelopes.clone();
        move |e| {
            envelopes.lock().unwrap().push(e.to_value());
        }
    }));
    (watch, envelopes)
}

fn has_event(envelope: &Value, event_type: &str) -> bool {
    envelope["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["type"] == event_type)
}

#[tokio::test]
async fn safe_tool_reads_its_durable_memo_after_jsonl_recovery() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let asks = Arc::new(AtomicUsize::new(0));
    let gate = Gate::new();
    let make_tool = |block: bool| {
        declaration(
            "recover-memo",
            Arc::new({
                let asks = asks.clone();
                let gate = gate.clone();
                move |_args, api, context| {
                    let asks = asks.clone();
                    let gate = gate.clone();
                    Box::pin(async move {
                        let decision = match api.memo("decision", None, context.clone()).await? {
                            Some(decision) => decision,
                            None => {
                                asks.fetch_add(1, Ordering::SeqCst);
                                let decision = api
                                    .memo("decision", Some(json!("approved")), context.clone())
                                    .await?
                                    .unwrap();
                                if block {
                                    gate.wait(context).await?;
                                }
                                decision
                            }
                        };
                        Ok(ToolResult {
                            content: Some(vec![
                                json!({"type":"text", "text": decision.as_str().unwrap()}),
                            ]),
                            ..Default::default()
                        })
                    })
                }
            }),
        )
    };
    let dir = tempfile::tempdir().unwrap();
    let env = open(OpenOptions {
        dir: Some(dir.path().to_path_buf()),
        tools: vec![make_tool(true)],
        ..Default::default()
    })
    .await
    .unwrap();
    let input = send_tool(&env, "recover-memo").await;
    arrived(&gate).await;
    env.close(ctx()).await.unwrap();
    let reopened = open(OpenOptions {
        dir: Some(dir.path().to_path_buf()),
        tools: vec![make_tool(false)],
        ..Default::default()
    })
    .await
    .unwrap();
    idle(&reopened).await;
    assert_eq!(
        asks.load(Ordering::SeqCst),
        1,
        "recovery must not repeat the external decision"
    );
    assert_eq!(
        reopened.input(input.id).await.unwrap().unwrap().status,
        "done"
    );
    let entries = reopened.entries(1).await.unwrap();
    assert!(
        content_of(entries.iter().find(|e| e.kind == "pi.tool_result").unwrap())
            .contains("approved")
    );
    reopened.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn approval_waiting_memo_emit_and_started_share_the_correct_namespace() {
    let approval = Gate::new();
    let executing = Gate::new();
    let tool = declaration(
        "approved-tool",
        Arc::new({
            let executing = executing.clone();
            move |_args, _api, context| {
                let executing = executing.clone();
                Box::pin(async move {
                    executing.wait(context).await?;
                    Ok(ToolResult::default())
                })
            }
        }),
    );
    let env = open(OpenOptions {
        tools: vec![tool],
        ..Default::default()
    })
    .await
    .unwrap();
    let ns = env
        .h
        .namespace("spec.approval", Default::default(), None)
        .unwrap();
    let kind = env.h.builtin_kind("pi.tool").unwrap();
    let off = env
        .h
        .hooks(
            &ns,
            &kind,
            Arc::new(ToolHandlers {
                before_tool: Some(Arc::new({
                    let approval = approval.clone();
                    move |call, api, context| {
                        let api = api.clone();
                        let approval = approval.clone();
                        let call_id = call["id"].clone();
                        Box::pin(async move {
                            assert_eq!(api.kind, "pi.tool");
                            assert_eq!(api.conversation_id, 1);
                            assert!(api.task_id.is_some());
                            assert_eq!(json!(api.call_id), call_id);
                            api.waiting(context.clone()).await?;
                            api.waiting(context.clone()).await?; // repeated waiting is event-idempotent
                            approval.wait(context.clone()).await?;
                            assert_eq!(api.memo("decision", None, context.clone()).await?, None);
                            assert_eq!(
                                api.memo("decision", Some(Value::Null), context.clone())
                                    .await?,
                                Some(Value::Null)
                            );
                            assert_eq!(
                                api.memo("decision", Some(json!("allow")), context.clone())
                                    .await?,
                                Some(Value::Null)
                            );
                            api.emit("approved", json!({"by":"spec"}), context).await?;
                            Ok(None)
                        })
                    }
                })),
                ..Default::default()
            }),
        )
        .unwrap();
    let (watch, envelopes) = watch(&env).await;
    let input = send_tool(&env, "approved-tool").await;
    arrived(&approval).await;
    assert_eq!(
        env.root.sticky(ctx()).await.unwrap()["turn"]["tools"][0]["waitingOn"],
        json!(ns.id)
    );
    approval.open();
    arrived(&executing).await;
    let sticky = env.root.sticky(ctx()).await.unwrap();
    let slot = &sticky["turn"]["tools"][0];
    assert_eq!(slot["status"], "running");
    assert!(slot.get("waitingOn").is_none());
    assert_eq!(slot["memos"]["hook:spec.approval:decision"], Value::Null);
    let snapshot = env.root.watch(ctx()).await.unwrap();
    assert!(!serde_json::to_string(&snapshot.view)
        .unwrap()
        .contains("memos"));
    snapshot.stop();
    {
        let log = envelopes.lock().unwrap();
        assert_eq!(
            log.iter().filter(|e| has_event(e, "tool.waiting")).count(),
            1
        );
        let started = log.iter().find(|e| has_event(e, "tool.started")).unwrap();
        assert!(
            started["ops"].to_string().contains("waitingOn"),
            "wait removal must accompany tool.started"
        );
        assert!(log
            .iter()
            .flat_map(|e| e["events"].as_array().unwrap())
            .any(|e| {
                e["type"] == "plugin.spec.approval.approved" && e["data"] == json!({"by":"spec"})
            }));
    }
    executing.open();
    settle(&input).await;
    idle(&env).await;
    off();
    watch.stop();
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn throwing_approval_clears_waiting_in_the_synthetic_result_envelope() {
    let tool = tool("blocked-after-wait", ToolOptions::default());
    let env = open(OpenOptions {
        tools: vec![tool.declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let ns = env
        .h
        .namespace("spec.throwing-approval", Default::default(), None)
        .unwrap();
    let kind = env.h.builtin_kind("pi.tool").unwrap();
    let _off = env
        .h
        .hooks(
            &ns,
            &kind,
            Arc::new(ToolHandlers {
                before_tool: Some(Arc::new(|_call, api, context| {
                    let api = api.clone();
                    Box::pin(async move {
                        api.waiting(context).await?;
                        anyhow::bail!("approval service failed")
                    })
                })),
                ..Default::default()
            }),
        )
        .unwrap();
    let (watch, envelopes) = watch(&env).await;
    settle(&send_tool(&env, "blocked-after-wait").await).await;
    idle(&env).await;
    assert_eq!(tool.calls(), 0);
    let entries = env.entries(1).await.unwrap();
    // anyhow Display has no JS Error: prefix; preserve the actual error message.
    assert!(
        content_of(entries.iter().find(|e| e.kind == "pi.tool_result").unwrap())
            .contains("hook threw: approval service failed")
    );
    {
        let log = envelopes.lock().unwrap();
        let finished = log.iter().find(|e| has_event(e, "tool.finished")).unwrap();
        assert!(finished["ops"].to_string().contains("waitingOn"));
    }
    assert_eq!(
        env.root.sticky(ctx()).await.unwrap()["turn"]["tools"],
        json!([])
    );
    watch.stop();
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn abort_unwinds_a_waiting_approval_without_executing_the_tool() {
    let gate = Gate::new();
    let tool = tool("wait-forever", ToolOptions::default());
    let env = open(OpenOptions {
        tools: vec![tool.declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let ns = env
        .h
        .namespace("spec.abort-approval", Default::default(), None)
        .unwrap();
    let kind = env.h.builtin_kind("pi.tool").unwrap();
    let _off = env
        .h
        .hooks(
            &ns,
            &kind,
            Arc::new(ToolHandlers {
                before_tool: Some(Arc::new({
                    let gate = gate.clone();
                    move |_call, api, context| {
                        let api = api.clone();
                        let gate = gate.clone();
                        Box::pin(async move {
                            api.waiting(context.clone()).await?;
                            gate.wait(context).await?;
                            Ok(None)
                        })
                    }
                })),
                ..Default::default()
            }),
        )
        .unwrap();
    let input = send_tool(&env, "wait-forever").await;
    arrived(&gate).await;
    assert_eq!(
        env.root.sticky(ctx()).await.unwrap()["turn"]["tools"][0]["waitingOn"],
        json!(ns.id)
    );
    tokio::time::timeout(Duration::from_secs(8), env.root.abort(ctx()))
        .await
        .expect("abort must join hook")
        .unwrap();
    assert_eq!(
        input
            .result(ctx())
            .await
            .unwrap()
            .unwrap()
            .reason
            .as_deref(),
        Some("aborted")
    );
    assert_eq!(
        env.root.sticky(ctx()).await.unwrap()["turn"],
        json!({"tools":[]})
    );
    assert_eq!(tool.calls(), 0);
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn suspend_clears_waiting_without_terminalizing_the_durable_tool() {
    use crate::agent_core::harness::pico3::jsonl::JsonlStorage;
    use crate::agent_core::harness::pico3::types::{DocRef, Storage, TaskStatus};
    let gate = Gate::new();
    let dir = tempfile::tempdir().unwrap();
    let tool = tool("suspended-approval", ToolOptions::default());
    let env = open(OpenOptions {
        dir: Some(dir.path().to_path_buf()),
        tools: vec![tool.declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let ns = env
        .h
        .namespace("spec.suspended-approval", Default::default(), None)
        .unwrap();
    let kind = env.h.builtin_kind("pi.tool").unwrap();
    let _off = env
        .h
        .hooks(
            &ns,
            &kind,
            Arc::new(ToolHandlers {
                before_tool: Some(Arc::new({
                    let gate = gate.clone();
                    move |_call, api, context| {
                        let api = api.clone();
                        let gate = gate.clone();
                        Box::pin(async move {
                            api.waiting(context.clone()).await?;
                            gate.wait(context).await?;
                            Ok(None)
                        })
                    }
                })),
                ..Default::default()
            }),
        )
        .unwrap();
    let _input = send_tool(&env, "suspended-approval").await;
    arrived(&gate).await;
    let task = env
        .tasks(None)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.kind == "pi.tool")
        .unwrap();
    env.h.suspend(ctx()).await.unwrap();
    let storage = JsonlStorage::open(dir.path(), false).await.unwrap();
    let sticky = storage
        .doc(&DocRef::Sticky { conversation_id: 1 }, ctx())
        .await
        .unwrap()
        .unwrap();
    assert!(sticky["turn"]["tools"][0].get("waitingOn").is_none());
    assert_eq!(
        storage.task(task.id, ctx()).await.unwrap().unwrap().status,
        TaskStatus::Running
    );
    assert_eq!(tool.calls(), 0);
    storage.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn hook_memos_are_isolated_from_other_namespaces_and_tool_memos() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let tool = declaration(
        "two-approvals",
        Arc::new({
            let observed = observed.clone();
            move |_args, api, context| {
                let observed = observed.clone();
                Box::pin(async move {
                    assert_eq!(api.memo("decision", None, context.clone()).await?, None);
                    observed.lock().unwrap().push(api.task_id.to_string());
                    assert_eq!(
                        api.memo("decision", Some(json!("tool")), context).await?,
                        Some(json!("tool"))
                    );
                    Ok(ToolResult::default())
                })
            }
        }),
    );
    let env = open(OpenOptions {
        tools: vec![tool],
        ..Default::default()
    })
    .await
    .unwrap();
    let kind = env.h.builtin_kind("pi.tool").unwrap();
    for id in ["spec.approver-a", "spec.approver-b"] {
        let ns = env.h.namespace(id, Default::default(), None).unwrap();
        let _off = env
            .h
            .hooks(
                &ns,
                &kind,
                Arc::new(ToolHandlers {
                    before_tool: Some(Arc::new({
                        let observed = observed.clone();
                        move |_call, api, context| {
                            let api = api.clone();
                            let observed = observed.clone();
                            Box::pin(async move {
                                assert_eq!(
                                    api.memo("decision", None, context.clone()).await?,
                                    None
                                );
                                assert_eq!(
                                    api.memo("decision", Some(json!(id)), context.clone())
                                        .await?,
                                    Some(json!(id))
                                );
                                assert_eq!(
                                    api.memo("decision", None, context).await?,
                                    Some(json!(id))
                                );
                                observed.lock().unwrap().push(id.to_owned());
                                Ok(None)
                            })
                        }
                    })),
                    ..Default::default()
                }),
            )
            .unwrap();
    }
    settle(&send_tool(&env, "two-approvals").await).await;
    idle(&env).await;
    assert_eq!(
        &observed.lock().unwrap()[..2],
        &["spec.approver-a", "spec.approver-b"]
    );
    assert_eq!(observed.lock().unwrap().len(), 3);
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn captured_approval_api_cannot_mutate_after_its_invocation_returns() {
    use crate::agent_core::harness::pico3::runtime::BeforeToolApi;
    let captured: Arc<Mutex<Option<BeforeToolApi>>> = Arc::new(Mutex::new(None));
    let tool = tool("captured-api", ToolOptions::default());
    let env = open(OpenOptions {
        tools: vec![tool.declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let ns = env
        .h
        .namespace("spec.capture", Default::default(), None)
        .unwrap();
    let kind = env.h.builtin_kind("pi.tool").unwrap();
    let _off = env
        .h
        .hooks(
            &ns,
            &kind,
            Arc::new(ToolHandlers {
                before_tool: Some(Arc::new({
                    let captured = captured.clone();
                    move |_call, api, _context| {
                        *captured.lock().unwrap() = Some(api.clone());
                        Box::pin(async { Ok(None) })
                    }
                })),
                ..Default::default()
            }),
        )
        .unwrap();
    let (watch, envelopes) = watch(&env).await;
    settle(&send_tool(&env, "captured-api").await).await;
    idle(&env).await;
    let api = captured.lock().unwrap().clone().unwrap();
    let before = env.root.sticky(ctx()).await.unwrap();
    let event_count = envelopes.lock().unwrap().len();
    assert!(api.waiting(ctx()).await.is_err());
    assert!(api
        .memo("late", Some(json!("forged")), ctx())
        .await
        .is_err());
    assert!(api.emit("late", json!("forged"), ctx()).await.is_err());
    assert_eq!(env.root.sticky(ctx()).await.unwrap(), before);
    assert_eq!(envelopes.lock().unwrap().len(), event_count);
    watch.stop();
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn hook_unsubscribe_uses_registration_identity_and_is_idempotent() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let tool = tool("duplicate-hooks", ToolOptions::default());
    let env = open(OpenOptions {
        tools: vec![tool.declaration()],
        ..Default::default()
    })
    .await
    .unwrap();
    let ns = env
        .h
        .namespace("spec.duplicate-hooks", Default::default(), None)
        .unwrap();
    let kind = env.h.builtin_kind("pi.tool").unwrap();
    let handler = |label: &'static str| {
        Arc::new(ToolHandlers {
            before_tool: Some(Arc::new({
                let observed = observed.clone();
                move |_call, _api, _context| {
                    observed.lock().unwrap().push(label);
                    Box::pin(async { Ok(None) })
                }
            })),
            ..Default::default()
        })
    };
    let a = handler("A");
    let off_a1 = env.h.hooks(&ns, &kind, a.clone()).unwrap();
    let off_b = env.h.hooks(&ns, &kind, handler("B")).unwrap();
    let off_a2 = env.h.hooks(&ns, &kind, a).unwrap();
    off_a2();
    off_a2();
    settle(&send_tool(&env, "duplicate-hooks").await).await;
    idle(&env).await;
    assert_eq!(*observed.lock().unwrap(), vec!["A", "B"]);
    off_a1();
    off_a1();
    off_b();
    off_b();
    observed.lock().unwrap().clear();
    // The conversation-scoped entry point must have the same subscription semantics.
    let off = env.root.hooks(&ns, &kind, handler("C"), false).unwrap();
    off();
    off();
    settle(&send_tool(&env, "duplicate-hooks").await).await;
    idle(&env).await;
    assert!(observed.lock().unwrap().is_empty());
    env.close(ctx()).await.unwrap();
}
