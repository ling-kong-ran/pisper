//! Real offline AgentSession consumers of the borrowed asynchronous event API.
//! These are native integration regressions, not a substitute for JS-host parity.
use super::*;
use crate::agent_core::types::{AgentEvent, AgentMessage};
use crate::coding_agent::agent_session::{PromptOptions, ReloadOptions};
use crate::coding_agent::extensions::types::{self, HandlerResult};
use std::sync::atomic::Ordering;
use tokio::sync::oneshot;

type Trace = Arc<Mutex<Vec<Value>>>;
fn record(trace: &Trace, value: Value) {
    trace.lock().unwrap().push(value);
}
fn snapshot(trace: &Trace) -> Vec<Value> {
    trace.lock().unwrap().clone()
}
async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("event consumer deadlocked")
}

// Each gate is consumed once. A reloaded factory can reuse the registry without
// manufacturing a fresh blocker, and every captured event/context is borrowed
// until the handler has actually resumed.
fn gated_factory(
    kind: &'static str,
    result: Option<Value>,
    trace: &Trace,
) -> (ExtensionFactory, oneshot::Sender<()>, oneshot::Receiver<()>) {
    let (release, wait) = oneshot::channel();
    let wait = Arc::new(Mutex::new(Some(wait)));
    let (entered, started) = oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered)));
    let trace = trace.clone();
    let factory: ExtensionFactory = Arc::new(move |api| {
        let wait = wait.clone();
        let entered = entered.clone();
        let trace = trace.clone();
        let result = result.clone();
        let handler: HandlerFn = Arc::new(move |event, ctx| {
            let wait = wait.lock().unwrap().take();
            let entered = entered.lock().unwrap().take();
            let trace = trace.clone();
            let result = result.clone();
            Box::pin(async move {
                record(&trace, json!({"phase":"start","event":event}));
                if let Some(entered) = entered {
                    let _ = entered.send(());
                }
                if let Some(wait) = wait {
                    wait.await.unwrap();
                }
                assert!(
                    ctx.cwd().is_ok(),
                    "context invalidated before awaited handler finished"
                );
                record(&trace, json!({"phase":"end","event":event}));
                Ok(result.map(HandlerResult::Json))
            })
        });
        api.on(kind, handler).map(|_| ())
    });
    (factory, release, started)
}
fn capture_api() -> (ExtensionFactory, Arc<Mutex<Option<ExtensionApi>>>) {
    let slot = Arc::new(Mutex::new(None));
    let saved = slot.clone();
    (
        Arc::new(move |api| {
            *saved.lock().unwrap() = Some(api.clone());
            Ok(())
        }),
        slot,
    )
}

#[tokio::test]
async fn async_events_session_reload_waits_before_invalidation_and_rebind() {
    let trace = Trace::default();
    let (shutdown, release, started) = gated_factory("session_shutdown", None, &trace);
    let log = trace.clone();
    let startup: ExtensionFactory = Arc::new(move |api| {
        let log = log.clone();
        api.on(
            "session_start",
            types::sync_handler(move |event, _| {
                record(&log, json!({"phase":"startup","event":event}));
                Ok(None)
            }),
        )
        .map(|_| ())
    });
    let test = create_test_session(vec![shutdown, startup], Vec::new()).await;
    bounded(test.session.bind_extensions(ExtensionBindings {
        on_error: Some(Arc::new(|_| {})),
        ..Default::default()
    }))
    .await
    .unwrap();
    trace.lock().unwrap().clear();
    let old = test.session.extension_runner();
    let context = old.create_context();
    let log = trace.clone();
    let mut reload = Box::pin(test.session.reload(Some(ReloadOptions {
        before_session_start: Some(Arc::new(move || {
            record(&log, json!({"phase":"before_start"}))
        })),
    })));
    assert!(futures::poll!(reload.as_mut()).is_pending());
    bounded(started).await.unwrap();
    assert!(context.cwd().is_ok());
    assert_eq!(snapshot(&trace).len(), 1);
    assert!(Arc::ptr_eq(
        &old.inner,
        &test.session.extension_runner().inner
    ));
    release.send(()).unwrap();
    bounded(reload).await.unwrap();
    assert!(context.cwd().is_err());
    assert!(!Arc::ptr_eq(
        &old.inner,
        &test.session.extension_runner().inner
    ));
    let phases = snapshot(&trace)
        .into_iter()
        .map(|v| v["phase"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(phases, ["start", "end", "before_start", "startup"]);
    assert_eq!(test.faux.state().lock().unwrap().call_count, 0);
    test.session.dispose();
}

#[tokio::test]
async fn async_events_session_input_handled_waits_and_reports_preflight_once() {
    let trace = Trace::default();
    let (factory, release, started) =
        gated_factory("input", Some(json!({"action":"handled"})), &trace);
    let test = create_test_session(vec![factory], Vec::new()).await;
    let before = serde_json::to_value(test.session.messages()).unwrap();
    let log = trace.clone();
    let mut prompt = Box::pin(test.session.prompt(
        "input",
        Some(PromptOptions {
            preflight_result: Some(Arc::new(move |ready| {
                record(&log, json!({"phase":"preflight","ready":ready.as_str()}))
            })),
            ..Default::default()
        }),
    ));
    assert!(futures::poll!(prompt.as_mut()).is_pending());
    bounded(started).await.unwrap();
    assert_eq!(snapshot(&trace).len(), 1);
    assert_eq!(test.faux.state().lock().unwrap().call_count, 0);
    release.send(()).unwrap();
    bounded(prompt).await.unwrap();
    assert_eq!(
        snapshot(&trace).last(),
        Some(&json!({"phase":"preflight","ready":"handled"}))
    );
    assert_eq!(snapshot(&trace).len(), 3);
    assert_eq!(test.faux.state().lock().unwrap().call_count, 0);
    assert_eq!(
        serde_json::to_value(test.session.messages()).unwrap(),
        before
    );
    test.session.dispose();
}

#[tokio::test]
async fn async_events_session_message_end_waits_before_listeners_and_persistence() {
    let trace = Trace::default();
    let replacement = json!({"role":"user","content":"changed","timestamp":1});
    let (factory, release, started) =
        gated_factory("message_end", Some(json!({"message":replacement})), &trace);
    let test = create_test_session(vec![factory], Vec::new()).await;
    let message: AgentMessage =
        serde_json::from_value(json!({"role":"user","content":"original","timestamp":1})).unwrap();
    test.session.agent.state().messages.push(message.clone());
    let entries_before = test
        .session
        .session_manager
        .lock()
        .unwrap()
        .get_entries()
        .len();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let sink = observed.clone();
    test.session.subscribe(Arc::new(move |event| {
        sink.lock()
            .unwrap()
            .push(serde_json::to_value(event).unwrap());
    }));
    let mut pending = Box::pin(
        test.session
            .handle_agent_event(AgentEvent::MessageEnd { message }),
    );
    assert!(futures::poll!(pending.as_mut()).is_pending());
    bounded(started).await.unwrap();
    assert!(observed.lock().unwrap().is_empty());
    assert_eq!(
        test.session
            .session_manager
            .lock()
            .unwrap()
            .get_entries()
            .len(),
        entries_before
    );
    release.send(()).unwrap();
    bounded(pending).await;
    assert_eq!(observed.lock().unwrap()[0]["message"], replacement);
    let entries = test.session.session_manager.lock().unwrap().get_entries();
    assert_eq!(entries.len(), entries_before + 1);
    let expected: AgentMessage = serde_json::from_value(replacement).unwrap();
    assert_eq!(test.session.agent.state().messages.last(), Some(&expected));
    match entries.last().unwrap() {
        SessionEntry::Message(entry) => assert_eq!(entry.message, expected),
        _ => panic!("message not persisted"),
    };
    test.session.dispose();
}

#[tokio::test]
async fn async_events_session_steer_and_follow_up_wait_before_queueing() {
    for (steer, streaming) in [(true, false), (false, false), (true, true), (false, true)] {
        let trace = Trace::default();
        let (factory, release, started) = gated_factory(
            "input",
            Some(json!({"action":"transform","text":"changed"})),
            &trace,
        );
        let test = create_test_session(vec![factory], Vec::new()).await;
        // Upstream isStreaming reads the session run flag, not agent.state.
        test.session
            .is_agent_run_active
            .store(streaming, Ordering::SeqCst);
        let mut queued = Box::pin(async {
            if steer {
                test.session.steer("input", None, None).await.unwrap();
            } else {
                test.session.follow_up("input", None, None).await.unwrap();
            }
        });
        assert!(futures::poll!(queued.as_mut()).is_pending());
        bounded(started).await.unwrap();
        assert_eq!(test.session.pending_message_count(), 0);
        release.send(()).unwrap();
        bounded(queued).await;
        let (steering, follow_up) = test.session.clear_queue();
        assert_eq!(if steer { steering } else { follow_up }, ["changed"]);
        let trace = snapshot(&trace);
        let behavior = trace[0]["event"].get("streamingBehavior");
        if streaming {
            assert_eq!(
                behavior,
                Some(&json!(if steer { "steer" } else { "followUp" }))
            );
        } else {
            assert_eq!(behavior, None, "upstream omits behavior while idle");
        }
        assert_eq!(test.faux.state().lock().unwrap().call_count, 0);
        test.session
            .is_agent_run_active
            .store(false, Ordering::SeqCst);
        test.session.dispose();
    }
}

#[tokio::test]
async fn async_events_set_model_api_waits_for_model_select() {
    let trace = Trace::default();
    let (factory, release, started) = gated_factory("model_select", None, &trace);
    let (capture, slot) = capture_api();
    let test = create_test_session(vec![factory, capture], Vec::new()).await;
    test.session
        .model_runtime()
        .set_runtime_api_key("anthropic", "r12-offline-test-key", None)
        .await
        .unwrap();
    let api = slot.lock().unwrap().clone().unwrap();
    let mut model = test.session.model().unwrap();
    model.id = "r12-switch".into();
    let mut pending = Box::pin(
        api.set_model(&serde_json::to_value(&model).unwrap())
            .unwrap(),
    );
    bounded(started).await.unwrap();
    assert!(futures::poll!(pending.as_mut()).is_pending());
    assert_eq!(test.session.model().unwrap().id, "r12-switch");
    release.send(()).unwrap();
    assert!(bounded(pending).await.unwrap());
    let mut missing_model = model.clone();
    missing_model.provider = "r12-unconfigured".into();
    let missing = api
        .set_model(&serde_json::to_value(missing_model).unwrap())
        .unwrap();
    assert!(!bounded(missing).await.unwrap());
    assert_eq!(snapshot(&trace).len(), 2);
    assert_eq!(test.faux.state().lock().unwrap().call_count, 0);
    test.session
        .extension_runner()
        .invalidate(Some("stale r12"));
    assert!(api.set_model(&json!({})).is_err());
    test.session.dispose();
}

#[tokio::test]
async fn async_events_send_user_message_reports_error_after_await_instead_of_dropping_it() {
    let trace = Trace::default();
    let (factory, release, started) = gated_factory("input", None, &trace);
    let (capture, slot) = capture_api();
    let test = create_test_session(vec![factory, capture], Vec::new()).await;
    // Forces the upstream missing-streamingBehavior failure after input. No
    // actual agent task/provider is started; this state is reset below.
    test.session
        .is_agent_run_active
        .store(true, Ordering::SeqCst);
    let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
    test.session
        .extension_runner()
        .on_error(Arc::new(move |error| {
            send.send(error.clone()).unwrap();
        }));
    let api = slot.lock().unwrap().clone().unwrap();
    api.send_user_message(&json!("input"), &SendUserMessageOptions::default())
        .unwrap();
    bounded(started).await.unwrap();
    assert!(receive.try_recv().is_err());
    release.send(()).unwrap();
    let error = bounded(receive.recv()).await.unwrap();
    assert_eq!(error.extension_path, "<runtime>");
    assert_eq!(error.event, "send_user_message");
    assert_eq!(
        error.error,
        "Agent is already processing. Specify streamingBehavior ('steer' or 'followUp') to queue the message."
    );
    assert_eq!(snapshot(&trace).len(), 2);
    assert_eq!(test.faux.state().lock().unwrap().call_count, 0);
    test.session
        .is_agent_run_active
        .store(false, Ordering::SeqCst);
    test.session.dispose();
}

#[tokio::test]
async fn async_events_unawaited_session_notifications_are_scheduled_not_dropped() {
    for kind in ["session_info_changed", "thinking_level_select"] {
        let trace = Trace::default();
        let (factory, release, started) = gated_factory(kind, None, &trace);
        let test = create_test_session(vec![factory], Vec::new()).await;
        let (sent, done) = oneshot::channel();
        let sent = Mutex::new(Some(sent));
        test.session.extension_runner().inner.extensions[0]
            .handlers
            .set(kind, {
                let mut handlers = test.session.extension_runner().inner.extensions[0]
                    .handlers
                    .get_cloned_list(kind);
                handlers.push(types::sync_handler(move |_, _| {
                    sent.lock().unwrap().take().unwrap().send(()).unwrap();
                    Ok(None)
                }));
                handlers
            });
        if kind == "session_info_changed" {
            test.session.set_session_name("r12");
            assert_eq!(test.session.session_name().as_deref(), Some("r12"));
        } else {
            test.session.set_thinking_level(ThinkingLevel::High, None);
            assert_eq!(test.session.thinking_level(), ThinkingLevel::High);
        }
        bounded(started).await.unwrap();
        assert_eq!(snapshot(&trace).len(), 1);
        release.send(()).unwrap();
        bounded(done).await.unwrap();
        assert_eq!(snapshot(&trace).len(), 2);
        test.session.dispose();
    }
}

#[tokio::test]
async fn async_events_compact_action_routes_failure_to_callback() {
    let test = create_test_session(Vec::new(), Vec::new()).await;
    let (send, result) = oneshot::channel();
    let send = Mutex::new(Some(send));
    test.session
        .extension_runner()
        .create_context()
        .compact(Some(types::CompactOptions {
            on_complete: Some(Arc::new(|_| panic!("empty session cannot compact"))),
            on_error: Some(Arc::new(move |error| {
                send.lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(error.to_owned())
                    .unwrap();
            })),
            ..Default::default()
        }))
        .unwrap();
    assert_eq!(
        bounded(result).await.unwrap(),
        "Nothing to compact (session too small)"
    );
    assert_eq!(test.faux.state().lock().unwrap().call_count, 0);
    test.session.dispose();
}

#[test]
fn async_events_action_without_runtime_reports_failure_and_does_not_poll() {
    let result = Arc::new(Mutex::new(None));
    let sink = result.clone();
    super::super::spawn_extension_action::<()>(
        async { panic!("work must not be polled without runtime") },
        move |outcome| {
            *sink.lock().unwrap() = Some(outcome);
        },
    );
    assert_eq!(
        *result.lock().unwrap(),
        Some(Err("Extension async actions require a Tokio runtime".into()))
    );
}

// These helper tests pin native Tokio behavior, not JavaScript microtasks or
// exceptions thrown from the completion callback itself.
#[tokio::test]
async fn async_events_action_completion_waits_for_work_and_survives_detached_caller() {
    let (release, gate) = oneshot::channel();
    let (started, entered) = oneshot::channel();
    let (complete, mut result) = oneshot::channel();
    super::super::spawn_extension_action(
        async move {
            started.send(()).unwrap();
            gate.await.unwrap();
            Ok(42)
        },
        move |value| {
            complete.send(value).unwrap();
        },
    );
    bounded(entered).await.unwrap();
    assert!(result.try_recv().is_err());
    release.send(()).unwrap();
    assert_eq!(bounded(result).await.unwrap(), Ok(42));
}

#[tokio::test]
async fn async_events_action_panic_after_await_reaches_error_sink() {
    let (release, gate) = oneshot::channel();
    let (started, entered) = oneshot::channel();
    let (complete, mut result) = oneshot::channel();
    super::super::spawn_extension_action::<()>(
        async move {
            started.send(()).unwrap();
            gate.await.unwrap();
            panic!("r12 work panic");
        },
        move |value| {
            complete.send(value).unwrap();
        },
    );
    bounded(entered).await.unwrap();
    assert!(result.try_recv().is_err());
    release.send(()).unwrap();
    assert_eq!(bounded(result).await.unwrap(), Err("r12 work panic".into()));
}
