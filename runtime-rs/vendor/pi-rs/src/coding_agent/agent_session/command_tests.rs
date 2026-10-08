//! Consumer coverage for the unmodified upstream AgentSession module oracle.
//! The existing offline session fixture uses in-memory stores and a faux model.
use super::*;
use std::future::Future;
use std::task::Poll;

use crate::coding_agent::agent_session::{PromptOptions, StreamingDelivery};
use crate::coding_agent::extensions::types::{
    Cancelled, CommandFuture, CommandHandler, ExtensionCommandContextActions, HandlerResult,
};
use tokio::sync::oneshot;

type Trace = Arc<Mutex<Vec<Value>>>;
type Gate = Arc<Mutex<Option<oneshot::Receiver<()>>>>;

fn record(trace: &Trace, item: Value) {
    trace.lock().unwrap().push(item);
}
fn snapshot(trace: &Trace) -> Vec<Value> {
    trace.lock().unwrap().clone()
}
async fn bounded<T>(work: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .expect("command consumer should finish after releasing its gate")
}
fn preflight(trace: &Trace) -> PromptOptions {
    let trace = trace.clone();
    PromptOptions {
        preflight_result: Some(Arc::new(move |ready| {
            record(&trace, json!({"phase":"preflight", "value":ready.as_str()}));
        })),
        ..PromptOptions::default()
    }
}
fn scenario_factory(kind: &str, trace: &Trace, gate: Gate) -> ExtensionFactory {
    let kind = kind.to_owned();
    let trace = trace.clone();
    Arc::new(move |api| {
        let mode = kind.clone();
        let log = trace.clone();
        let gate = gate.clone();
        let handler: CommandHandler = Arc::new(move |args, _| {
            record(&log, json!({"phase":"handler", "args":args}));
            match mode.as_str() {
                "sync_throw" => Err("sync failure".into()),
                "reject" => Ok(Some(CommandFuture::rejected("async failure".into()))),
                "empty_reject" => Ok(Some(CommandFuture::rejected(String::new()))),
                "resolved" => Ok(Some(CommandFuture::resolved(()))),
                "pending_resolve" | "pending_reject" => {
                    let wait = gate
                        .lock()
                        .unwrap()
                        .take()
                        .expect("single command invocation");
                    let reject = mode == "pending_reject";
                    CommandFuture::spawn(async move {
                        wait.await.expect("gate sender retained until release");
                        if reject {
                            Err("async failure".into())
                        } else {
                            Ok(())
                        }
                    })
                    .map(Some)
                }
                _ => Ok(None),
            }
        });
        api.register_command("cmd", None, handler)?;
        let log = trace.clone();
        api.on(
            "input",
            crate::coding_agent::extensions::types::sync_handler(move |_, _| {
                record(&log, json!({"phase":"input"}));
                Ok(Some(HandlerResult::Json(json!({"action":"handled"}))))
            }),
        )
        .map(|_| ())
    })
}

#[tokio::test]
async fn command_context_prompt_reports_preflight_once_for_handled_command() {
    let factory: ExtensionFactory =
        Arc::new(|api| api.register_command("cmd", None, Arc::new(|_, _| Ok(None))));
    let test = create_test_session(vec![factory], Vec::new()).await;
    let trace = Trace::default();
    bounded(test.session.prompt("/cmd args", Some(preflight(&trace))))
        .await
        .unwrap();
    assert_eq!(
        snapshot(&trace),
        vec![json!({"phase":"preflight", "value":"handled"})],
        "actual upstream prompt returns immediately after handling a command"
    );
    test.session.dispose();
}

#[tokio::test]
async fn command_context_consumer_matches_actual_source_oracle() {
    let oracle: Value = serde_json::from_str(include_str!("command_oracle.json")).unwrap();
    let rows = oracle["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 17);
    for row in rows {
        let name = row["name"].as_str().unwrap();
        let is_prompt = name.starts_with("prompt_");
        let kind = name.split_once('_').unwrap().1;
        let text = row["text"].as_str().unwrap();
        let trace = Trace::default();
        let (release, wait) = oneshot::channel();
        let gate = Arc::new(Mutex::new(Some(wait)));
        let factory = scenario_factory(kind, &trace, gate);
        let test = create_test_session(vec![factory], Vec::new()).await;
        let messages_before = serde_json::to_value(test.session.messages()).unwrap();
        let log = trace.clone();
        let _listener = test
            .session
            .extension_runner()
            .on_error(Arc::new(move |error| {
                record(
                    &log,
                    json!({"phase":"error", "extensionPath":error.extension_path,
                "event":error.event, "error":error.error}),
                );
            }));
        let task = async {
            if is_prompt {
                match test.session.prompt(text, Some(preflight(&trace))).await {
                    Ok(()) => json!({"ok":null}),
                    Err(error) => json!({"error":error.to_string()}),
                }
            } else {
                json!({"ok":test.session.try_execute_extension_command(text).await})
            }
        };
        let mut task = std::pin::pin!(task);
        let first = futures::poll!(task.as_mut());
        let before = snapshot(&trace);
        let pending = first.is_pending();
        // Settled native handles may complete in the first poll: this asserts
        // the observable pre-gate/final trace, not JS microtask scheduling.
        let _ = release.send(());
        let result = match first {
            Poll::Ready(value) => value,
            Poll::Pending => bounded(task).await,
        };
        assert_eq!(
            json!({"name":name,"text":text,"pending":pending,
            "before":before,"trace":snapshot(&trace),"result":result}),
            *row,
            "{name}"
        );
        assert!(
            serde_json::to_value(test.session.messages()).unwrap() == messages_before,
            "{name}: handled command must not change the constructor-seeded transcript"
        );
        assert!(!test.session.is_streaming(), "{name}");
        assert_eq!(test.faux.state().lock().unwrap().call_count, 0);
        test.session.dispose();
    }
}

#[tokio::test]
async fn command_context_prompt_awaits_reload_action_and_observes_its_rejection() {
    for reject in [false, true] {
        let trace = Trace::default();
        let log = trace.clone();
        let factory: ExtensionFactory = Arc::new(move |api| {
            let log = log.clone();
            api.register_command(
                "cmd",
                None,
                Arc::new(move |_, ctx| {
                    record(&log, json!({"phase":"handler"}));
                    ctx.reload().map(Some)
                }),
            )
        });
        let test = create_test_session(vec![factory], Vec::new()).await;
        let messages_before = serde_json::to_value(test.session.messages()).unwrap();
        let (release, wait) = oneshot::channel();
        let gate = Arc::new(Mutex::new(Some(wait)));
        let log = trace.clone();
        let runner = test.session.extension_runner();
        runner.bind_command_context(Some(ExtensionCommandContextActions {
            wait_for_idle: Arc::new(|| Ok(CommandFuture::resolved(()))),
            new_session: Arc::new(|_| Ok(CommandFuture::resolved(Cancelled { cancelled: false }))),
            fork: Arc::new(|_, _| Ok(CommandFuture::resolved(Cancelled { cancelled: false }))),
            navigate_tree: Arc::new(|_, _| {
                Ok(CommandFuture::resolved(Cancelled { cancelled: false }))
            }),
            switch_session: Arc::new(|_, _| {
                Ok(CommandFuture::resolved(Cancelled { cancelled: false }))
            }),
            reload: Arc::new(move || {
                record(&log, json!({"phase":"reload:start"}));
                let wait = gate.lock().unwrap().take().unwrap();
                let log = log.clone();
                CommandFuture::spawn(async move {
                    wait.await.unwrap();
                    record(&log, json!({"phase":"reload:end"}));
                    if reject {
                        Err("reload failed".into())
                    } else {
                        Ok(())
                    }
                })
            }),
        }));
        let log = trace.clone();
        let _listener = runner.on_error(Arc::new(move |error| {
            record(&log, json!({"phase":"error", "error":error.error}));
        }));
        let mut task = std::pin::pin!(test.session.prompt("/cmd", Some(preflight(&trace))));
        assert!(futures::poll!(task.as_mut()).is_pending());
        assert_eq!(
            snapshot(&trace),
            vec![json!({"phase":"handler"}), json!({"phase":"reload:start"})]
        );
        runner.bind_command_context(None); // inflight Promise keeps its selected action
        release.send(()).unwrap();
        bounded(task).await.unwrap();
        let mut expected = vec![
            json!({"phase":"handler"}),
            json!({"phase":"reload:start"}),
            json!({"phase":"reload:end"}),
        ];
        if reject {
            expected.push(json!({"phase":"error", "error":"reload failed"}));
        }
        expected.push(json!({"phase":"preflight", "value":"handled"}));
        assert_eq!(snapshot(&trace), expected);
        assert_eq!(
            serde_json::to_value(test.session.messages()).unwrap(),
            messages_before
        );
        assert_eq!(test.faux.state().lock().unwrap().call_count, 0);
        test.session.dispose();
    }
}

#[tokio::test]
async fn command_context_consumer_reports_pending_error_to_current_runner() {
    let trace = Trace::default();
    let (release, wait) = oneshot::channel();
    let factory = scenario_factory("pending_reject", &trace, Arc::new(Mutex::new(Some(wait))));
    let test = create_test_session(vec![factory], Vec::new()).await;
    let replacement = create_test_session(Vec::new(), Vec::new()).await;
    let old_errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = old_errors.clone();
    let _old = test
        .session
        .extension_runner()
        .on_error(Arc::new(move |error| {
            log.lock().unwrap().push(error.error.clone());
        }));
    let new_errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = new_errors.clone();
    let _new = replacement
        .session
        .extension_runner()
        .on_error(Arc::new(move |error| {
            log.lock().unwrap().push(error.error.clone());
        }));
    let mut task = std::pin::pin!(test.session.prompt("/cmd", Some(preflight(&trace))));
    assert!(futures::poll!(task.as_mut()).is_pending());
    // Explicit collaborator swap, not a claim that RuntimeHost reload is ported.
    *test.session.extension_runner.lock().unwrap() = Some(replacement.session.extension_runner());
    release.send(()).unwrap();
    bounded(task).await.unwrap();
    assert!(old_errors.lock().unwrap().is_empty());
    assert_eq!(*new_errors.lock().unwrap(), vec!["async failure"]);
    assert_eq!(
        snapshot(&trace),
        vec![
            json!({"phase":"handler","args":""}),
            json!({"phase":"preflight","value":"handled"})
        ]
    );
    test.session.dispose();
    replacement.session.dispose();
}

#[tokio::test]
async fn command_context_prompt_streaming_early_returns_report_preflight_once() {
    let factory: ExtensionFactory =
        Arc::new(|api| api.register_command("cmd", None, Arc::new(|_, _| Ok(None))));
    let test = blocked_session(vec![factory]).await;
    let session = test.session.clone();
    let first = tokio::spawn(async move { session.prompt("first", None).await });
    wait_for(|| test.session.is_streaming()).await;
    let command_trace = Trace::default();
    bounded(test.session.prompt("/cmd", Some(preflight(&command_trace))))
        .await
        .unwrap();
    assert_eq!(
        snapshot(&command_trace),
        vec![json!({"phase":"preflight","value":"handled"})]
    );
    let queue_trace = Trace::default();
    let mut options = preflight(&queue_trace);
    options.streaming_behavior = Some(StreamingDelivery::FollowUp);
    bounded(test.session.prompt("queued", Some(options)))
        .await
        .unwrap();
    assert_eq!(
        snapshot(&queue_trace),
        vec![json!({"phase":"preflight","value":"queued"})]
    );
    test.gate.notify_one();
    bounded(test.session.abort()).await;
    bounded(first).await.unwrap().unwrap();
    test.session.dispose();
}
