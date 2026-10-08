//! Actual-source runner oracle tests. Native async handlers are collaborators;
//! this does not claim the as-yet-unported RuntimeHost/JS host is implemented.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{future::FutureExt, Future};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::command_future::CommandFuture;
use super::loader::ExtensionRuntime;
use super::runner::ExtensionRunner;
use super::types::{self, Cancelled, ExtensionCommandContext, ExtensionCommandContextActions};

const NAMES: [&str; 6] = [
    "waitForIdle",
    "newSession",
    "fork",
    "navigateTree",
    "switchSession",
    "reload",
];
type Trace = Arc<Mutex<Vec<String>>>;

fn runner() -> ExtensionRunner {
    ExtensionRunner::new(
        Vec::new(),
        ExtensionRuntime::new(),
        "/workspace",
        Arc::new(()),
        Arc::new(types::NoopProviderRegistry),
    )
}
fn push(trace: &Trace, item: &str) {
    trace.lock().unwrap().push(item.to_owned());
}
fn snapshot(trace: &Trace) -> Vec<String> {
    trace.lock().unwrap().clone()
}
fn oracle(name: &str) -> Value {
    let root: Value = serde_json::from_str(include_str!("command_context_oracle.json")).unwrap();
    root["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap()
        .clone()
}
fn check(name: &str, observed: Value) {
    let mut expected = oracle(name);
    expected.as_object_mut().unwrap().remove("name");
    assert_eq!(observed, expected, "{name}");
}
async fn bounded<T>(work: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .expect("command must finish after its gate is released")
}

#[derive(Clone)]
struct Promises {
    unit: CommandFuture<()>,
    cancelled: CommandFuture<Cancelled>,
}
impl Promises {
    fn ready(cancelled: bool) -> Self {
        Self {
            unit: CommandFuture::resolved(()),
            cancelled: CommandFuture::resolved(Cancelled { cancelled }),
        }
    }
}
fn gated() -> (Promises, oneshot::Sender<Result<Cancelled, String>>) {
    let (tx, rx) = oneshot::channel::<Result<Cancelled, String>>();
    let completion = async move { rx.await.expect("gate sender retained") }
        .boxed()
        .shared();
    let unit = completion.clone();
    (
        Promises {
            unit: CommandFuture::spawn(async move { unit.await.map(|_| ()) }).unwrap(),
            cancelled: CommandFuture::spawn(completion).unwrap(),
        },
        tx,
    )
}
fn actions(
    handler: impl Fn() -> Result<Promises, String> + Send + Sync + 'static,
) -> ExtensionCommandContextActions {
    let handler = Arc::new(handler);
    ExtensionCommandContextActions {
        wait_for_idle: {
            let h = handler.clone();
            Arc::new(move || Ok(h()?.unit))
        },
        new_session: {
            let h = handler.clone();
            Arc::new(move |_| Ok(h()?.cancelled))
        },
        fork: {
            let h = handler.clone();
            Arc::new(move |_, _| Ok(h()?.cancelled))
        },
        navigate_tree: {
            let h = handler.clone();
            Arc::new(move |_, _| Ok(h()?.cancelled))
        },
        switch_session: {
            let h = handler.clone();
            Arc::new(move |_, _| Ok(h()?.cancelled))
        },
        reload: Arc::new(move || Ok(handler()?.unit)),
    }
}
fn traced_actions(
    trace: &Trace,
    tag: &'static str,
    response: Result<Promises, String>,
) -> ExtensionCommandContextActions {
    let trace = trace.clone();
    actions(move || {
        push(&trace, tag);
        response.clone()
    })
}
#[derive(Clone)]
enum Returned {
    Unit(CommandFuture<()>),
    Cancelled(CommandFuture<Cancelled>),
}
impl Returned {
    fn same(&self, promises: &Promises) -> bool {
        match self {
            Self::Unit(f) => f.same_promise(&promises.unit),
            Self::Cancelled(f) => f.same_promise(&promises.cancelled),
        }
    }
    async fn value(self) -> Result<Value, String> {
        match self {
            Self::Unit(f) => f.await.map(|()| Value::Null),
            Self::Cancelled(f) => f.await.map(|v| json!({"cancelled": v.cancelled})),
        }
    }
    fn pending(&self) -> bool {
        self.clone().value().now_or_never().is_none()
    }
}
fn invoke(ctx: &ExtensionCommandContext, name: &str) -> Result<Returned, String> {
    match name {
        "waitForIdle" => ctx.wait_for_idle().map(Returned::Unit),
        "newSession" => ctx.new_session(None).map(Returned::Cancelled),
        "fork" => ctx.fork("entry-β", None).map(Returned::Cancelled),
        "navigateTree" => ctx.navigate_tree("entry-β", None).map(Returned::Cancelled),
        "switchSession" => ctx.switch_session("entry-β", None).map(Returned::Cancelled),
        "reload" => ctx.reload().map(Returned::Unit),
        _ => panic!("unknown command"),
    }
}
async fn outcome(returned: Returned) -> Value {
    match bounded(returned.value()).await {
        Ok(v) => json!({"ok": v}),
        Err(e) => json!({"error": e}),
    }
}

async fn pending_cases(reject: bool) {
    for name in NAMES {
        let runner = runner();
        let ctx = runner.create_command_context();
        let trace = Trace::default();
        let (promises, gate) = gated();
        runner.bind_command_context(Some(traced_actions(&trace, "call", Ok(promises.clone()))));
        let returned = invoke(&ctx, name).unwrap();
        let same_promise = returned.same(&promises);
        push(&trace, "returned");
        let pending = returned.pending();
        let before = snapshot(&trace);
        gate.send(if reject {
            Err("async failure".into())
        } else {
            Ok(Cancelled { cancelled: true })
        })
        .unwrap();
        let result = outcome(returned).await;
        push(&trace, if reject { "rejected" } else { "fulfilled" });
        check(
            &format!("{name}_{}", if reject { "reject" } else { "resolve" }),
            json!({
                "samePromise": same_promise, "pending": pending, "before": before, "trace": snapshot(&trace), "result": result,
            }),
        );
    }
}
#[tokio::test]
async fn command_context_pending_resolution_matches_upstream() {
    pending_cases(false).await;
}
#[tokio::test]
async fn command_context_async_rejection_matches_upstream() {
    pending_cases(true).await;
}

#[test]
fn command_context_handler_throw_is_synchronous_not_a_rejected_future() {
    for name in NAMES {
        let runner = runner();
        let ctx = runner.create_command_context();
        let trace = Trace::default();
        runner.bind_command_context(Some(traced_actions(
            &trace,
            "call",
            Err("sync failure".into()),
        )));
        let error = invoke(&ctx, name)
            .err()
            .expect("must throw before returning a future");
        check(
            &format!("{name}_sync_throw"),
            json!({"trace": snapshot(&trace), "error": error}),
        );
    }
}
#[test]
fn command_context_stale_guards_are_immediate_and_getters_remain_lazy() {
    for name in NAMES {
        let runner = runner();
        let ctx = runner.create_command_context();
        let trace = Trace::default();
        runner.bind_command_context(Some(traced_actions(
            &trace,
            "call",
            Ok(Promises::ready(false)),
        )));
        runner.invalidate(Some("old context"));
        runner.invalidate(Some("later message"));
        let fresh = runner.create_command_context();
        let observed = json!({"error": invoke(&ctx, name).err(), "newContextError": invoke(&fresh, name).err(),
            "getterError": ctx.cwd().err(), "optionsError": ctx.get_system_prompt_options().err(), "trace": snapshot(&trace)});
        let mut expected = oracle(&format!("{name}_stale"));
        expected.as_object_mut().unwrap().remove("name");
        // The oracle collaborator counts invalidate calls; the native runtime
        // has no such hook. Check the actual runtime's first-message guard instead.
        expected.as_object_mut().unwrap().remove("invalidations");
        assert_eq!(observed, expected);
        assert_eq!(
            runner.inner.runtime.assert_active(),
            Err("old context".into())
        );
    }
}
#[tokio::test]
async fn command_context_rebind_reset_and_inflight_completion_match_upstream() {
    for name in NAMES {
        let runner = runner();
        let ctx = runner.create_command_context();
        let trace = Trace::default();
        let (promises, gate) = gated();
        runner.bind_command_context(Some(traced_actions(&trace, "A", Ok(promises))));
        let first = invoke(&ctx, name).unwrap();
        runner.bind_command_context(Some(traced_actions(&trace, "B", Ok(Promises::ready(true)))));
        let second = invoke(&ctx, name).unwrap();
        runner.bind_command_context(None);
        let reset = invoke(&ctx, name).unwrap();
        runner.invalidate(Some("replaced"));
        let next_error = invoke(&ctx, name).err();
        gate.send(Ok(Cancelled { cancelled: false })).unwrap();
        check(
            &format!("{name}_rebind_reset_inflight"),
            json!({"trace": snapshot(&trace), "first": bounded(first.value()).await.unwrap(),
            "second": bounded(second.value()).await.unwrap(), "reset": bounded(reset.value()).await.unwrap(), "nextError": next_error}),
        );
    }
}
#[tokio::test]
async fn command_context_discarding_the_promise_does_not_cancel_started_work() {
    for name in NAMES {
        let runner = runner();
        let ctx = runner.create_command_context();
        let trace = Trace::default();
        let (release, wait) = oneshot::channel();
        let wait = Arc::new(Mutex::new(Some(wait)));
        let (finished, done) = oneshot::channel();
        let finished = Arc::new(Mutex::new(Some(finished)));
        let log = trace.clone();
        runner.bind_command_context(Some(actions(move || {
            push(&log, "started");
            let trace = log.clone();
            let wait = wait.lock().unwrap().take().unwrap();
            let finished = finished.lock().unwrap().take().unwrap();
            let cancelled = CommandFuture::spawn(async move {
                wait.await.unwrap();
                push(&trace, "completed");
                finished.send(()).unwrap();
                Ok(Cancelled { cancelled: false })
            })?;
            let map_unit = cancelled.clone();
            Ok(Promises {
                unit: CommandFuture::spawn(async move { map_unit.await.map(|_| ()) })?,
                cancelled,
            })
        })));
        drop(invoke(&ctx, name).unwrap());
        let before = snapshot(&trace);
        release.send(()).unwrap();
        bounded(done).await.unwrap();
        check(
            &format!("{name}_discarded"),
            json!({"before": before, "trace": snapshot(&trace)}),
        );
    }
}
#[tokio::test]
async fn command_context_defaults_and_empty_invalidation_match_upstream() {
    let runner = runner();
    let ctx = runner.create_command_context();
    let mut initial = Vec::new();
    for name in NAMES {
        initial.push(bounded(invoke(&ctx, name).unwrap().value()).await.unwrap());
    }
    runner.invalidate(Some(""));
    let mut after_empty = Vec::new();
    for name in NAMES {
        after_empty.push(bounded(invoke(&ctx, name).unwrap().value()).await.unwrap());
    }
    assert!(runner.inner.runtime.assert_active().is_ok());
    runner.invalidate(Some("after empty"));
    let errors: Vec<_> = NAMES.iter().map(|name| invoke(&ctx, name).err()).collect();
    assert_eq!(
        runner.inner.runtime.assert_active(),
        Err("after empty".into())
    );
    let mut expected = oracle("defaults_and_empty_invalidation");
    expected.as_object_mut().unwrap().remove("name");
    expected.as_object_mut().unwrap().remove("invalidations");
    assert_eq!(
        json!({"initial": initial, "afterEmpty": after_empty, "errors": errors}),
        expected
    );
}

#[tokio::test]
async fn command_context_arguments_and_nested_callback_handles_are_forwarded() {
    let runner = runner();
    let ctx = runner.create_command_context();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let setup: types::SessionSetupHandler = Arc::new(|_| Ok(CommandFuture::resolved(())));
    let with_session: types::WithSessionHandler = Arc::new(|_| Ok(CommandFuture::resolved(())));
    let mut bound = actions(|| Ok(Promises::ready(false)));
    let log = observed.clone();
    let expected_setup = setup.clone();
    let expected_with = with_session.clone();
    bound.new_session = Arc::new(move |options| {
        let options = options.unwrap();
        assert!(Arc::ptr_eq(
            options.setup.as_ref().unwrap(),
            &expected_setup
        ));
        assert!(Arc::ptr_eq(
            options.with_session.as_ref().unwrap(),
            &expected_with
        ));
        log.lock().unwrap().push(json!({"action":"newSession","id":null,
            "options":{"parentSession":options.parent_session,"setup":"function","withSession":"function"}}));
        Ok(CommandFuture::resolved(Cancelled { cancelled: false }))
    });
    let log = observed.clone();
    let expected_with = with_session.clone();
    bound.fork = Arc::new(move |id, options| {
        let options = options.unwrap();
        assert!(Arc::ptr_eq(
            options.with_session.as_ref().unwrap(),
            &expected_with
        ));
        log.lock().unwrap().push(json!({"action":"fork","id":id,"options":{"position":options.position,"withSession":"function"}}));
        Ok(CommandFuture::resolved(Cancelled { cancelled: false }))
    });
    let log = observed.clone();
    bound.navigate_tree = Arc::new(move |id, options| {
        let options = options.unwrap();
        log.lock().unwrap().push(json!({"action":"navigateTree","id":id,
            "options":{"summarize":options.summarize,"customInstructions":options.custom_instructions,
            "replaceInstructions":options.replace_instructions,"label":options.label}}));
        Ok(CommandFuture::resolved(Cancelled { cancelled: false }))
    });
    let log = observed.clone();
    let expected_with = with_session.clone();
    bound.switch_session = Arc::new(move |id, options| {
        let options = options.unwrap();
        assert!(Arc::ptr_eq(
            options.with_session.as_ref().unwrap(),
            &expected_with
        ));
        log.lock()
            .unwrap()
            .push(json!({"action":"switchSession","id":id,"options":{"withSession":"function"}}));
        Ok(CommandFuture::resolved(Cancelled { cancelled: false }))
    });
    runner.bind_command_context(Some(bound));
    bounded(
        ctx.new_session(Some(types::NewSessionOptions {
            parent_session: Some("".into()),
            setup: Some(setup),
            with_session: Some(with_session.clone()),
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    bounded(
        ctx.fork(
            "entry-β",
            Some(types::ForkOptions {
                position: Some(types::TreePosition::At),
                with_session: Some(with_session.clone()),
            }),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    bounded(
        ctx.navigate_tree(
            "entry-β",
            Some(types::NavigateTreeOptions {
                summarize: Some(false),
                custom_instructions: Some("".into()),
                replace_instructions: Some(true),
                label: Some("".into()),
            }),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    bounded(
        ctx.switch_session(
            "entry-β",
            Some(types::SwitchSessionOptions {
                with_session: Some(with_session),
            }),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    let mut expected = oracle("argument_forwarding")["observed"].clone();
    // Owned Rust option structs cannot expose JS object identity. Preserve all
    // values and the exact Arc callback handles; don't claim arbitrary JS identity.
    for row in expected.as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("sameOptions");
    }
    assert_eq!(json!(*observed.lock().unwrap()), expected);
}

fn run_callbacks(
    setup: Option<types::SessionSetupHandler>,
    with_session: types::WithSessionHandler,
    ctx: ExtensionCommandContext,
    trace: Trace,
) -> Result<CommandFuture<Cancelled>, String> {
    push(&trace, "handler:start");
    // Match the synchronous prefix of the oracle's async callback: its first
    // nested callback is invoked before returning the promise to the runner.
    let first = if let Some(setup) = &setup {
        setup(&(Arc::new(()) as types::SessionManagerHandle))
    } else {
        with_session(&ctx)
    };
    CommandFuture::spawn(async move {
        first?.await?;
        if setup.is_some() {
            with_session(&ctx)?.await?;
        }
        push(&trace, "handler:end");
        Ok(Cancelled { cancelled: false })
    })
}
#[tokio::test]
async fn command_context_nested_callbacks_can_suspend_and_reject() {
    for outer in ["newSession", "fork", "switchSession"] {
        let failures = if outer == "newSession" {
            vec![None, Some("withSession"), Some("setup")]
        } else {
            vec![None, Some("withSession")]
        };
        for failure in failures {
            let runner = runner();
            let ctx = runner.create_command_context();
            let trace = Trace::default();
            let (release, wait) = oneshot::channel::<()>();
            let wait = async move { wait.await.unwrap() }.boxed().shared();
            let log = trace.clone();
            let gate = wait.clone();
            let setup: types::SessionSetupHandler = Arc::new(move |_| {
                push(&log, "setup:start");
                let log = log.clone();
                let gate = gate.clone();
                CommandFuture::spawn(async move {
                    gate.await;
                    if failure == Some("setup") {
                        return Err("setup failed".into());
                    }
                    push(&log, "setup:end");
                    Ok(())
                })
            });
            let log = trace.clone();
            let base = ctx.base.inner.clone();
            let with_session: types::WithSessionHandler = Arc::new(move |received| {
                push(&log, "withSession:start");
                let log = log.clone();
                let gate = wait.clone();
                let same_context = Arc::ptr_eq(&received.base.inner, &base);
                CommandFuture::spawn(async move {
                    gate.await;
                    if failure == Some("withSession") {
                        return Err("withSession failed".into());
                    }
                    push(
                        &log,
                        if same_context {
                            "withSession:same-context"
                        } else {
                            "wrong-context"
                        },
                    );
                    Ok(())
                })
            });
            let mut bound = actions(|| Ok(Promises::ready(false)));
            let command_ctx = ctx.clone();
            let log = trace.clone();
            match outer {
                "newSession" => {
                    bound.new_session = Arc::new(move |options| {
                        let options = options.unwrap();
                        run_callbacks(
                            options.setup,
                            options.with_session.unwrap(),
                            command_ctx.clone(),
                            log.clone(),
                        )
                    })
                }
                "fork" => {
                    bound.fork = Arc::new(move |_, options| {
                        run_callbacks(
                            None,
                            options.unwrap().with_session.unwrap(),
                            command_ctx.clone(),
                            log.clone(),
                        )
                    })
                }
                "switchSession" => {
                    bound.switch_session = Arc::new(move |_, options| {
                        run_callbacks(
                            None,
                            options.unwrap().with_session.unwrap(),
                            command_ctx.clone(),
                            log.clone(),
                        )
                    })
                }
                _ => unreachable!(),
            }
            runner.bind_command_context(Some(bound));
            let task = match outer {
                "newSession" => ctx.new_session(Some(types::NewSessionOptions {
                    parent_session: None,
                    setup: Some(setup),
                    with_session: Some(with_session),
                })),
                "fork" => ctx.fork(
                    "entry-β",
                    Some(types::ForkOptions {
                        position: None,
                        with_session: Some(with_session),
                    }),
                ),
                "switchSession" => ctx.switch_session(
                    "entry-β",
                    Some(types::SwitchSessionOptions {
                        with_session: Some(with_session),
                    }),
                ),
                _ => unreachable!(),
            }
            .unwrap();
            assert!(task.clone().now_or_never().is_none());
            let before = snapshot(&trace);
            release.send(()).unwrap();
            let result = outcome(Returned::Cancelled(task)).await;
            check(
                &format!("{outer}_callbacks_{}", failure.unwrap_or("success")),
                json!({"before":before,"trace":snapshot(&trace),"result":result}),
            );
            // Drop context-capturing test actions to break their Arc cycle.
            runner.bind_command_context(None);
        }
    }
}

#[tokio::test]
async fn command_context_reentrant_handlers_do_not_hold_the_actions_mutex() {
    let runner = runner();
    let ctx = runner.create_command_context();
    let promises = Promises::ready(true);
    let selected = promises.clone();
    let rebound = runner.clone();
    runner.bind_command_context(Some(actions(move || {
        rebound.bind_command_context(None);
        rebound.invalidate(Some("inside handler"));
        Ok(promises.clone())
    })));
    let returned = ctx.new_session(None).unwrap();
    assert!(returned.same_promise(&selected.cancelled));
    assert!(bounded(returned).await.unwrap().cancelled);
    assert_eq!(ctx.reload().err(), Some("inside handler".into()));
}
#[tokio::test]
async fn command_context_future_supports_multiple_waiters_without_rerunning() {
    let (promises, release) = gated();
    let saved = promises.cancelled.clone();
    release.send(Err("shared rejection".into())).unwrap();
    let (a, b) =
        bounded(async { tokio::join!(promises.cancelled.clone(), promises.cancelled.clone()) })
            .await;
    assert_eq!(a, Err("shared rejection".into()));
    assert_eq!(a, b);
    assert!(promises.cancelled.same_promise(&saved));
    assert_eq!(bounded(saved).await, a);
    assert_eq!(
        bounded(CommandFuture::<()>::rejected("".into())).await,
        Err("".into())
    );
}
#[test]
fn command_context_settled_defaults_do_not_require_a_runtime() {
    let runner = runner();
    let ctx = runner.create_command_context();
    assert_eq!(ctx.wait_for_idle().unwrap().now_or_never(), Some(Ok(())));
    assert_eq!(
        ctx.new_session(None).unwrap().now_or_never(),
        Some(Ok(Cancelled { cancelled: false }))
    );
    assert_eq!(
        CommandFuture::spawn(async { Ok(()) }).err(),
        Some("Pending extension commands require a Tokio runtime".into())
    );
}
#[tokio::test]
async fn command_context_unexpected_task_panics_reach_the_awaiter() {
    let task = CommandFuture::<()>::spawn(async { panic!("native task panic probe") }).unwrap();
    let error = bounded(task).await.unwrap_err();
    assert!(error.starts_with("Extension command task failed:"));
    assert!(error.contains("native task panic probe"));
}
