//! r12: comparisons against the complete, unchanged upstream event runner.
use super::loader::{load_extension_from_factory, ExtensionRuntime};
use super::runner::ExtensionRunner;
use super::types::{self, HandlerFn, HandlerResult};
use crate::coding_agent::core::event_bus::EventBusController;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

type Trace = Arc<Mutex<Vec<String>>>;
fn push(trace: &Trace, value: &str) {
    trace.lock().unwrap().push(value.into());
}
fn snapshot(trace: &Trace) -> Vec<String> {
    trace.lock().unwrap().clone()
}
fn expected(name: &str) -> Value {
    let fixture: Value = serde_json::from_str(include_str!("async_event_oracle.json")).unwrap();
    fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap()["observed"]
        .clone()
}
fn setup(
    kind: &str,
    groups: Vec<(&str, Vec<HandlerFn>)>,
) -> (ExtensionRunner, Arc<Mutex<Vec<Value>>>) {
    let runtime = ExtensionRuntime::new();
    let extensions = groups
        .into_iter()
        .map(|(path, handlers)| {
            let kind = kind.to_owned();
            load_extension_from_factory(
                Arc::new(move |api| {
                    for handler in &handlers {
                        api.on(&kind, Arc::clone(handler))?;
                    }
                    Ok(())
                }),
                "/workspace",
                EventBusController::new().bus().clone(),
                &runtime,
                Some(path),
            )
            .unwrap()
        })
        .collect();
    let runner = ExtensionRunner::new(
        extensions,
        runtime,
        "/workspace",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    );
    let errors = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&errors);
    runner.on_error(Arc::new(move |error| {
        sink.lock()
            .unwrap()
            .push(json!({"path":error.extension_path,"event":error.event,"error":error.error}))
    }));
    (runner, errors)
}
#[tokio::test]
async fn async_events_generic_truthiness_matches_actual_source() {
    for cancel in [
        json!(true),
        json!(false),
        json!(""),
        json!("stop"),
        json!(0),
        json!(1),
        json!([]),
        json!({}),
        Value::Null,
    ] {
        let name = format!("cancel_{cancel}");
        let trace = Trace::default();
        let a = trace.clone();
        let b = trace.clone();
        let (runner, errors) = setup(
            "session_before_switch",
            vec![(
                "<a>",
                vec![
                    types::sync_handler(move |_, _| {
                        push(&a, "A");
                        Ok(Some(HandlerResult::Json(json!({"cancel":cancel}))))
                    }),
                    types::sync_handler(move |_, _| {
                        push(&b, "B");
                        Ok(Some(HandlerResult::Json(json!({"tail":true}))))
                    }),
                ],
            )],
        );
        let result = runner
            .emit(&mut json!({"type":"session_before_switch"}))
            .await;
        assert_eq!(
            json!({"result":result,"trace":snapshot(&trace),"errors":errors.lock().unwrap().clone()}),
            expected(&name),
            "{name}"
        );
    }
}
#[tokio::test]
async fn async_events_live_event_type_matches_actual_source() {
    for reverse in [false, true] {
        let trace = Trace::default();
        let a = trace.clone();
        let b = trace.clone();
        let kind = if reverse {
            "session_before_switch"
        } else {
            "agent_end"
        };
        let (runner, errors) = setup(
            kind,
            vec![(
                "<a>",
                vec![
                    types::sync_handler(move |event, _| {
                        event["type"] = json!(if reverse {
                            "agent_end"
                        } else {
                            "session_before_switch"
                        });
                        push(&a, "A");
                        Ok(Some(HandlerResult::Json(json!({"cancel":true}))))
                    }),
                    types::sync_handler(move |_, _| {
                        push(&b, "B");
                        Ok(None)
                    }),
                ],
            )],
        );
        let mut event = json!({"type":kind});
        let result = runner.emit(&mut event).await;
        assert_eq!(
            json!({"result":result,"trace":snapshot(&trace),"errors":errors.lock().unwrap().clone(),"event":event}),
            expected(&format!("mutable_event_type_{reverse}"))
        );
    }
}

fn result_for(name: &str, event: &mut Value) -> Option<HandlerResult> {
    let value = match name {
        "emit" => json!({"note":"A"}),
        "shutdown" => json!({"ignored":true}),
        "message_end" => json!({"message":{"role":"user","content":"changed"}}),
        "tool_result" => {
            json!({"content":[{"type":"text","text":"changed"}],"details":{"changed":true}})
        }
        "tool_call" => {
            event["input"]["changed"] = json!(true);
            json!({"block":true,"reason":"blocked"})
        }
        "user_bash" => {
            json!({"result":{"output":"changed","exitCode":0,"cancelled":false,"truncated":false}})
        }
        "context" => json!({"messages":[{"role":"user","content":"changed"}]}),
        "before_provider_request" => json!({"payload":{"changed":true}}),
        "before_provider_headers" => {
            event["headers"]["changed"] = json!("yes");
            return None;
        }
        "resources_discover" => json!({"skillPaths":["s"],"promptPaths":["p"],"themePaths":["t"]}),
        "input" => json!({"action":"transform","text":"changed"}),
        _ => panic!("unhandled method {name}"),
    };
    Some(HandlerResult::Json(value))
}

async fn invoke(name: &str, runner: &ExtensionRunner, event: &mut Value) -> Result<Value, String> {
    use super::runner::emit_session_shutdown_event;
    use types::{InputEventResult, InputSource, ResourcesDiscoverReason, UserBashEventResult};
    Ok(match name {
        "emit" => runner.emit(event).await.unwrap_or(Value::Null),
        "shutdown" => json!(emit_session_shutdown_event(runner, event).await),
        "message_end" => runner.emit_message_end(event).await.unwrap_or(Value::Null),
        "tool_result" => runner.emit_tool_result(event).await.unwrap_or(Value::Null),
        "tool_call" => runner.emit_tool_call(event).await?.unwrap_or(Value::Null),
        "user_bash" => match runner.emit_user_bash(event).await? {
            None => Value::Null,
            Some(UserBashEventResult::Result(result)) => json!({"result":result}),
            Some(UserBashEventResult::Operations(_)) => panic!("unexpected operations"),
        },
        "context" => json!(
            runner
                .emit_context(&[json!({"role":"user","content":"original"})])
                .await
        ),
        "before_provider_request" => {
            runner
                .emit_before_provider_request(json!({"start":true}))
                .await
        }
        "before_provider_headers" => {
            runner
                .emit_before_provider_headers(json!({"existing":"yes"}))
                .await
        }
        "resources_discover" => {
            let result = runner
                .emit_resources_discover("/workspace", ResourcesDiscoverReason::Startup)
                .await;
            let paths = |entries: Vec<(String, String)>| {
                entries.into_iter().map(|(path,extension_path)| json!({"path":path,"extensionPath":extension_path})).collect::<Vec<_>>()
            };
            json!({"skillPaths":paths(result.skill_paths),"promptPaths":paths(result.prompt_paths),"themePaths":paths(result.theme_paths)})
        }
        "input" => match runner
            .emit_input("original", None, InputSource::Rpc, None)
            .await
        {
            InputEventResult::Continue => json!({"action":"continue"}),
            InputEventResult::Handled => json!({"action":"handled"}),
            InputEventResult::Transform { text, images } => {
                let mut v = json!({"action":"transform","text":text});
                if let Some(images) = images {
                    v["images"] = json!(images);
                }
                v
            }
        },
        _ => panic!("unhandled method {name}"),
    })
}

#[tokio::test]
async fn async_events_all_pending_resolve_and_reject_match_actual_source() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    for name in [
        "emit",
        "shutdown",
        "message_end",
        "tool_result",
        "tool_call",
        "user_bash",
        "context",
        "before_provider_request",
        "before_provider_headers",
        "resources_discover",
        "input",
    ] {
        for reject in [false, true] {
            let kind = match name {
                "emit" => "session_before_switch",
                "shutdown" => "session_shutdown",
                other => other,
            };
            let trace = Trace::default();
            let same_context = Arc::new(AtomicBool::new(true));
            let first_context = Arc::new(AtomicUsize::new(0));
            let (release, gate) = tokio::sync::oneshot::channel::<()>();
            let gate = Mutex::new(Some(gate));
            let a: HandlerFn = {
                let trace = trace.clone();
                let first_context = first_context.clone();
                Arc::new(move |event, ctx| {
                    // Pointer identity only; no dereference/unsafe. Each dispatch
                    // must borrow one context for all handlers, including waits.
                    first_context.store(ctx as *const _ as usize, Ordering::SeqCst);
                    let trace = trace.clone();
                    let gate = gate.lock().unwrap().take().unwrap();
                    Box::pin(async move {
                        push(&trace, "A:start");
                        gate.await.unwrap();
                        push(&trace, "A:resume");
                        if reject {
                            return Err("async failure".into());
                        }
                        Ok(result_for(name, event))
                    })
                })
            };
            let b = {
                let trace = trace.clone();
                let first_context = first_context.clone();
                let same_context = same_context.clone();
                types::sync_handler(move |_, ctx| {
                    same_context.store(
                        first_context.load(Ordering::SeqCst) == ctx as *const _ as usize,
                        Ordering::SeqCst,
                    );
                    push(&trace, "B");
                    Ok(None)
                })
            };
            let (runner, errors) = setup(kind, vec![("<a>", vec![a]), ("<b>", vec![b])]);
            let mut event = json!({"type":kind,"message":{"role":"user","content":"original"},"content":[{"type":"text","text":"original"}],"details":{"start":true},"isError":false,"input":{"start":true},"command":"echo","cwd":"/workspace"});
            let mut pending = Box::pin(invoke(name, &runner, &mut event));
            let was_pending = futures::poll!(pending.as_mut()).is_pending();
            let before = snapshot(&trace);
            assert!(was_pending, "{name}: handler did not wait");
            release.send(()).unwrap();
            let outcome = match tokio::time::timeout(std::time::Duration::from_secs(5), pending)
                .await
                .expect("dispatch deadlocked")
            {
                Ok(value) => json!({"value":value}),
                Err(error) => json!({"error":error}),
            };
            let case = format!("{name}_{}", if reject { "reject" } else { "resolve" });
            assert_eq!(
                json!({"before":before,"wasPending":was_pending,"trace":snapshot(&trace),"sameContext":same_context.load(Ordering::SeqCst),"outcome":outcome,"errors":errors.lock().unwrap().clone()}),
                expected(&case),
                "{case}"
            );
        }
    }
}

#[tokio::test]
async fn async_events_snapshot_across_await_matches_actual_source() {
    use super::runner::emit_session_shutdown_event;
    let trace = Trace::default();
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    let gate = Mutex::new(Some(gate));
    let a: HandlerFn = {
        let trace = trace.clone();
        Arc::new(move |_, _| {
            let gate = gate.lock().unwrap().take();
            let trace = trace.clone();
            Box::pin(async move {
                push(&trace, "A:start");
                if let Some(gate) = gate {
                    gate.await.unwrap();
                }
                push(&trace, "A:end");
                Ok(None)
            })
        })
    };
    let b = {
        let trace = trace.clone();
        types::sync_handler(move |_, _| {
            push(&trace, "B:old");
            Ok(None)
        })
    };
    let (runner, errors) = setup("session_shutdown", vec![("<a>", vec![a]), ("<b>", vec![b])]);
    let event = json!({"type":"session_shutdown"});
    let mut pending = Box::pin(emit_session_shutdown_event(&runner, &event));
    assert!(futures::poll!(pending.as_mut()).is_pending());
    let replacement = {
        let trace = trace.clone();
        types::sync_handler(move |_, _| {
            push(&trace, "B:new");
            Ok(None)
        })
    };
    runner.inner.extensions[1]
        .handlers
        .set("session_shutdown", vec![replacement]);
    release.send(()).unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), pending)
            .await
            .unwrap()
    );
    let first = snapshot(&trace);
    trace.lock().unwrap().clear();
    assert!(emit_session_shutdown_event(&runner, &event).await);
    assert_eq!(
        json!({"firstTrace":first,"secondTrace":snapshot(&trace),"errors":errors.lock().unwrap().clone()}),
        expected("snapshot_across_await")
    );
}

#[tokio::test]
async fn async_events_live_error_type_and_empty_shutdown_match_actual_source() {
    let (runner, errors) = setup(
        "agent_end",
        vec![(
            "<a>",
            vec![types::sync_handler(|event, _| {
                event["type"] = json!("changed");
                Err("failure".into())
            })],
        )],
    );
    let mut event = json!({"type":"agent_end"});
    let result = runner.emit(&mut event).await;
    assert_eq!(
        json!({"result":result,"trace":[],"errors":errors.lock().unwrap().clone(),"event":event}),
        expected("mutable_error_type")
    );
    let (runner, errors) = setup("session_shutdown", vec![]);
    assert_eq!(
        json!({"value":super::runner::emit_session_shutdown_event(&runner,&json!({"type":"session_shutdown"})).await,"errors":errors.lock().unwrap().clone()}),
        expected("shutdown_no_handlers")
    );
}

#[tokio::test]
async fn async_events_borrowed_event_mutation_before_reject_is_not_lost() {
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    let gate = Mutex::new(Some(gate));
    let handler: HandlerFn = Arc::new(move |event, ctx| {
        let gate = gate.lock().unwrap().take().unwrap();
        Box::pin(async move {
            gate.await.unwrap();
            assert_eq!(ctx.cwd().unwrap(), "/workspace");
            event["input"]["changed"] = json!(true);
            Err("rejected after mutation".into())
        })
    });
    let (runner, errors) = setup("tool_call", vec![("<a>", vec![handler])]);
    let mut event = json!({"type":"tool_call","input":{}});
    let mut pending = Box::pin(runner.emit_tool_call(&mut event));
    assert!(futures::poll!(pending.as_mut()).is_pending());
    release.send(()).unwrap();
    assert_eq!(pending.await, Err("rejected after mutation".into()));
    assert_eq!(event["input"], json!({"changed":true}));
    assert!(errors.lock().unwrap().is_empty());
}

#[tokio::test]
async fn async_events_before_agent_start_awaits_native_handler() {
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    let gate = Mutex::new(Some(gate));
    let handler: HandlerFn = Arc::new(move |_, ctx| {
        let gate = gate.lock().unwrap().take().unwrap();
        Box::pin(async move {
            gate.await.unwrap();
            assert_eq!(ctx.get_system_prompt().unwrap(), "base");
            Ok(Some(HandlerResult::Json(
                json!({"systemPrompt":"changed","message":{"customType":"note","content":"done","display":false}}),
            )))
        })
    });
    let (runner, errors) = setup("before_agent_start", vec![("<a>", vec![handler])]);
    let options = types::BuildSystemPromptOptions {
        force_system_prompt: Some("base".into()),
        ..Default::default()
    };
    let mut pending = Box::pin(runner.emit_before_agent_start(
        "prompt",
        None,
        &options,
        Arc::new(types::ForcePromptRenderer),
    ));
    assert!(futures::poll!(pending.as_mut()).is_pending());
    release.send(()).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        result.system_prompt_options.force_system_prompt.as_deref(),
        Some("changed")
    );
    assert_eq!(result.messages.len(), 1);
    assert!(errors.lock().unwrap().is_empty());
}

#[tokio::test]
async fn async_events_trust_snapshot_across_await_matches_actual_source() {
    use super::runner::emit_project_trust_event;
    let trace = Trace::default();
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    let gate = Mutex::new(Some(gate));
    let a: HandlerFn = {
        let trace = trace.clone();
        Arc::new(move |_, _| {
            let gate = gate.lock().unwrap().take();
            let trace = trace.clone();
            Box::pin(async move {
                push(&trace, "A:start");
                if let Some(gate) = gate {
                    gate.await.unwrap();
                }
                push(&trace, "A:end");
                Ok(Some(HandlerResult::Json(json!({"trusted":"undecided"}))))
            })
        })
    };
    let b = {
        let trace = trace.clone();
        types::sync_handler(move |_, _| {
            push(&trace, "B:old");
            Ok(Some(HandlerResult::Json(json!({"trusted":"no"}))))
        })
    };
    let (runner, _) = setup("project_trust", vec![("<a>", vec![a]), ("<b>", vec![b])]);
    let extensions = &runner.inner.extensions;
    let event = types::ProjectTrustEvent {
        event_type: "project_trust".into(),
        cwd: "/workspace".into(),
    };
    let ctx = types::ProjectTrustContext {
        cwd: "/workspace".into(),
        mode: types::ExtensionMode::Print,
        has_ui: false,
        ui: None,
    };
    let mut pending = Box::pin(emit_project_trust_event(extensions, &event, &ctx));
    assert!(futures::poll!(pending.as_mut()).is_pending());
    let b = {
        let trace = trace.clone();
        types::sync_handler(move |_, _| {
            push(&trace, "B:new");
            Ok(Some(HandlerResult::Json(json!({"trusted":"yes"}))))
        })
    };
    extensions[1].handlers.set("project_trust", vec![b]);
    release.send(()).unwrap();
    let (first, errors) = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
        .await
        .unwrap();
    let first_trace = snapshot(&trace);
    trace.lock().unwrap().clear();
    let (second, more_errors) = emit_project_trust_event(extensions, &event, &ctx).await;
    assert!(errors.is_empty() && more_errors.is_empty());
    assert_eq!(
        json!({"firstTrace":first_trace,"secondTrace":snapshot(&trace),"firstResult":first,"secondResult":second,"errors":[]}),
        expected("trust_snapshot_across_await")
    );
}

#[tokio::test]
async fn async_events_headers_mutate_then_reject_matches_actual_source() {
    let trace = Trace::default();
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    let gate = Mutex::new(Some(gate));
    let a: HandlerFn = {
        let trace = trace.clone();
        Arc::new(move |event, _| {
            let trace = trace.clone();
            let gate = gate.lock().unwrap().take().unwrap();
            Box::pin(async move {
                push(&trace, "A:start");
                gate.await.unwrap();
                event["headers"]["changed"] = json!("yes");
                push(&trace, "A:resume");
                Err("after mutation".into())
            })
        })
    };
    let b = {
        let trace = trace.clone();
        types::sync_handler(move |event, _| {
            push(
                &trace,
                &format!(
                    "B:{}",
                    event["headers"]["changed"].as_str().unwrap_or("undefined")
                ),
            );
            Ok(None)
        })
    };
    let (runner, errors) = setup(
        "before_provider_headers",
        vec![("<a>", vec![a]), ("<b>", vec![b])],
    );
    let mut pending = Box::pin(runner.emit_before_provider_headers(json!({"existing":"yes"})));
    assert!(futures::poll!(pending.as_mut()).is_pending());
    release.send(()).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
        .await
        .unwrap();
    assert_eq!(
        json!({"trace":snapshot(&trace),"result":result,"errors":errors.lock().unwrap().clone()}),
        expected("headers_mutate_then_reject")
    );
}

#[test]
fn async_events_detached_without_runtime_reports_explicit_failure() {
    let (runner, errors) = setup(
        "session_info_changed",
        vec![(
            "<a>",
            vec![types::sync_handler(|_, _| {
                panic!("no executor must not run handler")
            })],
        )],
    );
    runner.emit_detached(json!({"type":"session_info_changed"}));
    assert_eq!(
        *errors.lock().unwrap(),
        vec![
            json!({"path":"<host>","event":"session_info_changed","error":"Detached extension events require a Tokio runtime"})
        ]
    );
    let (runner, errors) = setup("session_info_changed", vec![]);
    runner.emit_detached(json!({"type":"session_info_changed"}));
    assert!(errors.lock().unwrap().is_empty());
}
