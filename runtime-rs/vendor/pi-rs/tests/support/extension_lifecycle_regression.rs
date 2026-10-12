//! 原生 extension 的资源生命周期回归；通过公开 bind_core/invalidate 路径，
//! 不启用上游缺失 oracle 的 lib cfg(test)，不访问网络或用户目录。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use pi_rust::coding_agent::core::event_bus::{EventBus, EventBusController};
use pi_rust::coding_agent::extensions::loader::{
    load_extension_from_factory, ExtensionFactory, ExtensionRuntime,
};
use pi_rust::coding_agent::extensions::runner::{ExtensionRunner, ProviderActions};
use pi_rust::coding_agent::extensions::types::{
    CommandFuture, ExtensionActions, ExtensionCommandContextActions, ExtensionContextActions,
    ExtensionError, ExtensionMode, ExtensionUI, NoopProviderRegistry, ThinkingLevel,
    ToolDefinition,
};
use serde_json::{json, Value};

fn actions() -> ExtensionActions {
    ExtensionActions {
        send_message: Arc::new(|_, _| {}),
        send_user_message: Arc::new(|_, _| {}),
        append_entry: Arc::new(|_, _| {}),
        set_session_name: Arc::new(|_| {}),
        get_session_name: Arc::new(|| None),
        set_label: Arc::new(|_, _| {}),
        get_active_tools: Arc::new(|| vec!["retained_fixture".into()]),
        get_all_tools: Arc::new(Vec::new),
        get_settings: Arc::new(|| json!({"isolated":true})),
        set_active_tools: Arc::new(|_| {}),
        refresh_tools: Arc::new(|| {}),
        get_commands: Arc::new(Vec::new),
        set_model: Arc::new(|_| Ok(CommandFuture::resolved(false))),
        get_thinking_level: Arc::new(|| ThinkingLevel::Off),
        set_thinking_level: Arc::new(|_| {}),
    }
}

fn context_actions() -> ExtensionContextActions {
    ExtensionContextActions {
        get_model: Arc::new(|| None),
        get_scoped_models: Arc::new(Vec::new),
        is_idle: Arc::new(|| true),
        is_project_trusted: Arc::new(|| true),
        get_signal: Arc::new(|| None),
        abort: Arc::new(|| {}),
        has_pending_messages: Arc::new(|| false),
        shutdown: Arc::new(|| {}),
        get_context_usage: Arc::new(|| None),
        compact: Arc::new(|_| {}),
        get_system_prompt: Arc::new(String::new),
        get_system_prompt_options: None,
        execute_tool: None,
        get_callable_tools: None,
    }
}

fn bound_tool(resource: Arc<String>) -> (ExtensionRunner, ExtensionRuntime) {
    let (runner, runtime) = unbound_tool(resource);
    runner.bind_core(Arc::new(actions()), Arc::new(context_actions()), None);
    (runner, runtime)
}

fn unbound_tool(resource: Arc<String>) -> (ExtensionRunner, ExtensionRuntime) {
    unbound_tool_with_runtime(resource, ExtensionRuntime::new())
}

fn unbound_tool_with_runtime(
    resource: Arc<String>,
    runtime: ExtensionRuntime,
) -> (ExtensionRunner, ExtensionRuntime) {
    let factory: ExtensionFactory = Arc::new(move |api| {
        let mut tool = ToolDefinition::new(
            "retained_fixture",
            "Retained resource",
            "Owned extension resource lifetime fixture",
            json!({"type":"object","properties":{}}),
        );
        let resource = resource.clone();
        tool.execute = Some(Arc::new(move |_, _, _, _, _| {
            Ok(json!({"content":[{"type":"text","text":resource.as_str()}]}))
        }));
        api.register_tool(tool)
    });
    let extension = load_extension_from_factory(
        factory,
        ".",
        EventBusController::new().bus().clone(),
        &runtime,
        Some("<inline:pisper-lifecycle-fixture>"),
    )
    .unwrap();
    let runner = ExtensionRunner::new(
        vec![extension],
        runtime.clone(),
        ".",
        Arc::new(()),
        Arc::new(NoopProviderRegistry),
    );
    (runner, runtime)
}

#[test]
fn real_bound_runner_releases_extension_tool_resource_after_invalidation() {
    let resource = Arc::new("isolated retained resource".to_string());
    let weak = Arc::downgrade(&resource);
    let (runner, runtime) = bound_tool(resource);
    assert_eq!(runtime.get_active_tools().unwrap(), ["retained_fixture"]);
    assert!(runtime.create_context().is_ok());
    runner.invalidate(Some("retired fixture"));
    drop(runner);
    drop(runtime);
    assert!(
        weak.upgrade().is_none(),
        "Real bind_core callbacks must not keep the runner and its extension resources alive"
    );
}

#[test]
fn empty_invalidation_keeps_bindings_then_first_nonempty_message_retires_them() {
    let resource = Arc::new("empty invalidation resource".to_string());
    let weak = Arc::downgrade(&resource);
    let (runner, runtime) = bound_tool(resource);
    runner.invalidate(Some(""));
    assert_eq!(runtime.get_active_tools().unwrap(), ["retained_fixture"]);
    assert!(runtime.create_context().unwrap().cwd().is_ok());
    assert!(weak.upgrade().is_some());
    runner.invalidate(Some("first retirement"));
    runner.invalidate(Some("later retirement"));
    assert_eq!(runtime.get_active_tools().unwrap_err(), "first retirement");
    assert_eq!(runtime.create_context().err().unwrap(), "first retirement");
    drop(runner);
    drop(runtime);
    assert!(weak.upgrade().is_none());
}

#[test]
fn default_invalidation_preserves_stale_context_error_and_releases_resources() {
    let resource = Arc::new("default invalidation resource".to_string());
    let weak = Arc::downgrade(&resource);
    let (runner, runtime) = bound_tool(resource);
    runner.invalidate(None);
    let stale = runtime.assert_active().unwrap_err();
    assert!(!stale.is_empty());
    runner.invalidate(Some("replacement message"));
    assert_eq!(runtime.create_context().err().unwrap(), stale);
    drop(runner);
    drop(runtime);
    assert!(weak.upgrade().is_none());
}

struct ReentrantResource {
    observer: Weak<ExtensionRuntime>,
    bus: EventBus,
    dropped: Arc<AtomicBool>,
}
impl Drop for ReentrantResource {
    fn drop(&mut self) {
        let runtime = self
            .observer
            .upgrade()
            .expect("Observer must outlive retirement");
        // 该公开只读操作重新取 runtime state 锁；锁内析构会产生真实死锁。
        assert!(runtime.mcp_servers_list().is_empty());
        self.bus.emit("retirement-probe", &Value::Null);
        self.dropped.store(true, Ordering::Release);
    }
}

#[test]
fn invalidation_drops_bound_resources_outside_state_after_event_bus_unsubscribe() {
    // 外层既有 watchdog 能诊断同步析构死锁，异步 timeout 无法中断同线程 Mutex。
    super::on_current_thread(async {
        let runtime = ExtensionRuntime::new();
        let observer = Arc::new(runtime.clone());
        let bus = EventBusController::new().bus().clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let _tracked = runtime.track_event_bus_subscription(bus.on(
            "retirement-probe",
            Arc::new(move |_| {
                counter.fetch_add(1, Ordering::Relaxed);
            }),
        ));
        bus.emit("retirement-probe", &Value::Null);
        assert_eq!(calls.load(Ordering::Acquire), 1);
        let dropped = Arc::new(AtomicBool::new(false));
        let resource = Arc::new(ReentrantResource {
            observer: Arc::downgrade(&observer),
            bus,
            dropped: dropped.clone(),
        });
        let weak = Arc::downgrade(&resource);
        let mut bound_actions = actions();
        bound_actions.get_settings =
            Arc::new(move || json!({"resourceAlive": !resource.dropped.load(Ordering::Acquire)}));
        let runner = ExtensionRunner::new(
            Vec::new(),
            runtime.clone(),
            ".",
            Arc::new(()),
            Arc::new(NoopProviderRegistry),
        );
        runner.bind_core(Arc::new(bound_actions), Arc::new(context_actions()), None);
        assert_eq!(
            runtime.get_settings().unwrap(),
            json!({"resourceAlive":true})
        );
        runner.invalidate(Some("retirement probe"));
        assert!(
            dropped.load(Ordering::Acquire),
            "Retiring bindings must actually drop owned action resources"
        );
        assert!(weak.upgrade().is_none());
        assert_eq!(
            calls.load(Ordering::Acquire),
            1,
            "Event bus subscriptions must already be cancelled during destruction"
        );
        drop(runner);
        drop(observer);
        drop(runtime);
    });
}

#[test]
fn retired_runner_rebind_cannot_recreate_its_resource_cycle() {
    let resource = Arc::new("stale rebind resource".to_string());
    let weak = Arc::downgrade(&resource);
    let (runner, runtime) = bound_tool(resource);
    runner.invalidate(Some("original retirement"));
    runner.bind_core(Arc::new(actions()), Arc::new(context_actions()), None);
    runner.invalidate(Some("later retirement"));
    assert_eq!(
        runtime.get_active_tools().unwrap_err(),
        "original retirement"
    );
    drop(runner);
    drop(runtime);
    assert!(
        weak.upgrade().is_none(),
        "A stale bind_core must not plant new callbacks that retain the retired runner"
    );
}

struct InvalidateOnDrop(ExtensionRuntime);
impl Drop for InvalidateOnDrop {
    fn drop(&mut self) {
        self.0.invalidate(Some("retired during action replacement"));
    }
}

#[test]
fn invalidation_after_action_commit_rejects_new_core_context() {
    super::on_current_thread(async {
        let resource = Arc::new("late core context resource".to_string());
        let weak = Arc::downgrade(&resource);
        let (runner, runtime) = unbound_tool(resource);
        let retirement = Arc::new(InvalidateOnDrop(runtime.clone()));
        let mut original = actions();
        original.get_settings = Arc::new(move || {
            std::hint::black_box(&retirement);
            Value::Null
        });
        runner.bind_core(Arc::new(original), Arc::new(context_actions()), None);
        let captured_runner = runner.clone();
        let mut next_context = context_actions();
        next_context.get_model = Arc::new(move || {
            std::hint::black_box(&captured_runner);
            None
        });
        // Replacing the old real action owner synchronously invalidates the
        // runtime after the first commit and before the new core context.
        runner.bind_core(Arc::new(actions()), Arc::new(next_context), None);
        assert_eq!(
            runtime.assert_active().unwrap_err(),
            "retired during action replacement"
        );
        drop(runner);
        drop(runtime);
        assert!(
            weak.upgrade().is_none(),
            "Core context must not commit after the runtime action replacement retires it"
        );
    });
}

#[test]
fn invalidation_retires_an_already_bound_core_context_that_owns_its_runner() {
    let resource = Arc::new("bound core context resource".to_string());
    let weak = Arc::downgrade(&resource);
    let (runner, runtime) = unbound_tool(resource);
    let captured_runner = runner.clone();
    let mut context = context_actions();
    context.get_model = Arc::new(move || {
        std::hint::black_box(&captured_runner);
        None
    });
    runner.bind_core(Arc::new(actions()), Arc::new(context), None);
    runner.invalidate(Some("retired core context"));
    drop(runner);
    drop(runtime);
    assert!(
        weak.upgrade().is_none(),
        "Retirement must release core context owners"
    );
}

#[test]
fn invalidation_retires_an_already_bound_command_context_that_owns_its_runner() {
    let resource = Arc::new("bound command context resource".to_string());
    let weak = Arc::downgrade(&resource);
    let (runner, runtime) = bound_tool(resource);
    let captured_runner = runner.clone();
    runner.bind_command_context(Some(ExtensionCommandContextActions {
        wait_for_idle: Arc::new(move || {
            std::hint::black_box(&captured_runner);
            Ok(CommandFuture::resolved(()))
        }),
        new_session: Arc::new(|_| {
            Ok(CommandFuture::resolved(
                pi_rust::coding_agent::extensions::types::Cancelled { cancelled: false },
            ))
        }),
        fork: Arc::new(|_, _| {
            Ok(CommandFuture::resolved(
                pi_rust::coding_agent::extensions::types::Cancelled { cancelled: false },
            ))
        }),
        navigate_tree: Arc::new(|_, _| {
            Ok(CommandFuture::resolved(
                pi_rust::coding_agent::extensions::types::Cancelled { cancelled: false },
            ))
        }),
        switch_session: Arc::new(|_, _| {
            Ok(CommandFuture::resolved(
                pi_rust::coding_agent::extensions::types::Cancelled { cancelled: false },
            ))
        }),
        reload: Arc::new(|| Ok(CommandFuture::resolved(()))),
    }));
    runner.invalidate(Some("retired command context"));
    drop(runner);
    drop(runtime);
    assert!(
        weak.upgrade().is_none(),
        "Retirement must release command context owners"
    );
}

struct OwnedRunnerUi {
    _runner: ExtensionRunner,
}
impl ExtensionUI for OwnedRunnerUi {}

#[test]
fn invalidation_retires_ui_and_error_bindings_and_rejects_their_stale_rebinds() {
    let resource = Arc::new("UI and diagnostic resource".to_string());
    let weak = Arc::downgrade(&resource);
    let (runner, runtime) = bound_tool(resource);
    runner.set_ui_context(
        Some(Arc::new(OwnedRunnerUi {
            _runner: runner.clone(),
        })),
        ExtensionMode::Print,
    );
    let captured = runner.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let tracked = runner.on_error(Arc::new(move |_| {
        std::hint::black_box(&captured);
        counter.fetch_add(1, Ordering::Relaxed);
    }));
    runner.invalidate(Some("retired host bindings"));
    assert!(runner.get_ui_context().is_none());
    runner.set_ui_context(
        Some(Arc::new(OwnedRunnerUi {
            _runner: runner.clone(),
        })),
        ExtensionMode::Print,
    );
    let captured = runner.clone();
    let counter = calls.clone();
    let rejected = runner.on_error(Arc::new(move |_| {
        std::hint::black_box(&captured);
        counter.fetch_add(1, Ordering::Relaxed);
    }));
    runner.emit_error(ExtensionError {
        extension_path: "<inline:fixture>".into(),
        event: "probe".into(),
        error: "isolated".into(),
        stack: None,
    });
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert!(runner.get_ui_context().is_none());
    // Handles outlive the runner; retiring their listener must still break
    // the callback's runner ownership, and both handles remain idempotent.
    tracked.unsubscribe();
    rejected.unsubscribe();
    drop(runner);
    drop(runtime);
    assert!(weak.upgrade().is_none());
}

#[test]
fn one_runtime_retires_core_bindings_from_every_real_bound_runner() {
    let runtime = ExtensionRuntime::new();
    let first = Arc::new("first shared runtime tool".to_string());
    let second = Arc::new("second shared runtime tool".to_string());
    let first_weak = Arc::downgrade(&first);
    let second_weak = Arc::downgrade(&second);
    let (first, _) = unbound_tool_with_runtime(first, runtime.clone());
    let (second, _) = unbound_tool_with_runtime(second, runtime.clone());
    for runner in [&first, &second] {
        let captured = runner.clone();
        let mut context = context_actions();
        context.get_model = Arc::new(move || {
            std::hint::black_box(&captured);
            None
        });
        runner.bind_core(Arc::new(actions()), Arc::new(context), None);
    }
    let first_context = first.create_context();
    let second_context = second.create_context();
    runtime.invalidate(Some("retired shared runtime"));
    assert_eq!(first_context.model().unwrap_err(), "retired shared runtime");
    assert_eq!(
        second_context.model().unwrap_err(),
        "retired shared runtime"
    );
    drop(first_context);
    drop(second_context);
    drop(first);
    drop(second);
    drop(runtime);
    assert!(first_weak.upgrade().is_none());
    assert!(second_weak.upgrade().is_none());
}

struct ReentrantRunnerResource {
    observer: Weak<ExtensionRunner>,
    dropped: Arc<AtomicBool>,
}
impl Drop for ReentrantRunnerResource {
    fn drop(&mut self) {
        let runner = self
            .observer
            .upgrade()
            .expect("Runner observer must remain live");
        // create_tool_context takes RunnerInner.actions even after staleness,
        // detecting actual callback destruction under that mutex.
        drop(runner.create_tool_context("retirement-lock-probe", None));
        self.dropped.store(true, Ordering::Release);
    }
}

#[test]
fn core_replacement_and_retirement_drop_callbacks_after_runner_actions_unlock() {
    super::on_current_thread(async {
        let resource = Arc::new("reentrant core fixture".to_string());
        let weak = Arc::downgrade(&resource);
        let (runner, runtime) = bound_tool(resource);
        let observer = Arc::new(runner.clone());
        for retiring in [false, true] {
            let dropped = Arc::new(AtomicBool::new(false));
            let resource = Arc::new(ReentrantRunnerResource {
                observer: Arc::downgrade(&observer),
                dropped: dropped.clone(),
            });
            let mut context = context_actions();
            context.get_model = Arc::new(move || {
                std::hint::black_box(&resource);
                None
            });
            runner.bind_core(Arc::new(actions()), Arc::new(context), None);
            if retiring {
                runner.invalidate(Some("reentrant core retirement"));
            } else {
                runner.bind_core(Arc::new(actions()), Arc::new(context_actions()), None);
            }
            assert!(dropped.load(Ordering::Acquire));
        }
        drop(observer);
        drop(runner);
        drop(runtime);
        assert!(weak.upgrade().is_none());
    });
}

#[test]
fn invalidation_during_real_provider_flush_rejects_later_binding_commits() {
    super::on_current_thread(async {
        let resource = Arc::new("interleaved bind resource".to_string());
        let weak = Arc::downgrade(&resource);
        let (runner, runtime) = unbound_tool(resource);
        runtime
            .register_provider("isolated", &Value::Null, "<inline:fixture>")
            .unwrap();
        let release = Arc::new(std::sync::Barrier::new(2));
        let (entered, paused) = std::sync::mpsc::sync_channel(1);
        let callback_runner = runner.clone();
        let callback_release = release.clone();
        let provider_actions = ProviderActions {
            register_provider: Some(Arc::new(move |_, _| {
                // Real bind_core flushes a queued registration before routing
                // is committed. A single entry-time stale check is insufficient.
                std::hint::black_box(&callback_runner);
                entered.send(()).unwrap();
                callback_release.wait();
                Ok(())
            })),
            ..ProviderActions::default()
        };
        let binding = runner.clone();
        let worker = std::thread::spawn(move || {
            binding.bind_core(
                Arc::new(actions()),
                Arc::new(context_actions()),
                Some(provider_actions),
            );
        });
        paused
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("The real pending provider flush must enter its callback");
        runner.invalidate(Some("retired during bind"));
        release.wait();
        worker.join().unwrap();
        assert_eq!(
            runtime.create_context().err().unwrap(),
            "retired during bind"
        );
        drop(runner);
        drop(runtime);
        assert!(
            weak.upgrade().is_none(),
            "A bind already in progress must not reinstall routing that owns its runner"
        );
    });
}

#[test]
fn late_stale_subscription_is_cancelled_and_dropped_outside_runtime_locks() {
    super::on_current_thread(async {
        let runtime = ExtensionRuntime::new();
        let observer = Arc::new(runtime.clone());
        let bus = EventBusController::new().bus().clone();
        runtime.invalidate(Some("retired before tracking"));
        let dropped = Arc::new(AtomicBool::new(false));
        let resource = Arc::new(ReentrantResource {
            observer: Arc::downgrade(&observer),
            // Separate bus: the EventBus owns its own listener-removal mutex.
            bus: EventBusController::new().bus().clone(),
            dropped: dropped.clone(),
        });
        let weak = Arc::downgrade(&resource);
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let tracked = runtime.track_event_bus_subscription(bus.on(
            "late-subscription",
            Arc::new(move |_| {
                std::hint::black_box(&resource);
                counter.fetch_add(1, Ordering::Relaxed);
            }),
        ));
        bus.emit("late-subscription", &Value::Null);
        tracked.unsubscribe();
        assert_eq!(calls.load(Ordering::Acquire), 0);
        assert!(dropped.load(Ordering::Acquire));
        assert!(weak.upgrade().is_none());
        assert_eq!(
            runtime.assert_active().unwrap_err(),
            "retired before tracking"
        );
    });
}

struct PausedSubscriptionDrop {
    entered: std::sync::mpsc::SyncSender<()>,
    release: Arc<std::sync::Barrier>,
}
impl Drop for PausedSubscriptionDrop {
    fn drop(&mut self) {
        self.entered.send(()).unwrap();
        self.release.wait();
    }
}

#[test]
fn nonempty_retirement_waits_for_an_earlier_empty_cancellation_batch() {
    cancellation_batch_fixture(false);
}

#[test]
fn nonempty_retirement_waits_for_an_earlier_direct_tracked_unsubscribe() {
    cancellation_batch_fixture(true);
}

fn cancellation_batch_fixture(direct_unsubscribe: bool) {
    super::on_current_thread(async move {
        let runtime = ExtensionRuntime::new();
        let observer = Arc::new(runtime.clone());
        let first_bus = EventBusController::new().bus().clone();
        let second_bus = EventBusController::new().bus().clone();
        let release = Arc::new(std::sync::Barrier::new(2));
        let (entered, paused) = std::sync::mpsc::sync_channel(1);
        let pause = Arc::new(PausedSubscriptionDrop {
            entered,
            release: release.clone(),
        });
        let first = runtime.track_event_bus_subscription(first_bus.on(
            "pause-unsubscribe",
            Arc::new(move |_| {
                std::hint::black_box(&pause);
            }),
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let _second = runtime.track_event_bus_subscription(second_bus.on(
            "retirement-probe",
            Arc::new(move |_| {
                counter.fetch_add(1, Ordering::Relaxed);
            }),
        ));
        let dropped = Arc::new(AtomicBool::new(false));
        let resource = Arc::new(ReentrantResource {
            observer: Arc::downgrade(&observer),
            bus: second_bus,
            dropped: dropped.clone(),
        });
        let weak = Arc::downgrade(&resource);
        let mut bound_actions = actions();
        bound_actions.get_settings =
            Arc::new(move || json!({"resourceAlive": !resource.dropped.load(Ordering::Acquire)}));
        let runner = ExtensionRunner::new(
            Vec::new(),
            runtime.clone(),
            ".",
            Arc::new(()),
            Arc::new(NoopProviderRegistry),
        );
        runner.bind_core(Arc::new(bound_actions), Arc::new(context_actions()), None);
        let earlier = runtime.clone();
        let captured_context = runner.create_context();
        let worker = std::thread::spawn(move || {
            if direct_unsubscribe {
                first.unsubscribe();
            } else {
                earlier.invalidate(Some(""));
            }
        });
        paused
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("Real first unsubscribe must pause after the batch was drained");
        runtime.invalidate(Some("first retirement"));
        assert_eq!(runtime.assert_active().unwrap_err(), "first retirement");
        assert_eq!(captured_context.model().unwrap_err(), "first retirement");
        let before_release = (
            calls.load(Ordering::Acquire),
            dropped.load(Ordering::Acquire),
        );
        release.wait();
        worker.join().unwrap();
        assert_eq!(
            before_release,
            (0, false),
            "Callbacks must remain owned until the earlier cancellation batch finishes"
        );
        assert_eq!(
            calls.load(Ordering::Acquire),
            0,
            "Resource destruction must not notify an uncancelled old subscription"
        );
        assert!(dropped.load(Ordering::Acquire));
        assert!(weak.upgrade().is_none());
        drop(captured_context);
        drop(runner);
        drop(observer);
        drop(runtime);
    });
}

struct PanickingSubscriptionDrop;
impl Drop for PanickingSubscriptionDrop {
    fn drop(&mut self) {
        panic!("original unsubscribe panic");
    }
}
struct PanickingBoundResource(ReentrantResource);
impl Drop for PanickingBoundResource {
    fn drop(&mut self) {
        panic!("secondary bound resource panic");
    }
}

#[test]
fn invalidation_panics_preserve_first_error_after_cancel_and_retirement_complete() {
    super::on_current_thread(async {
        let runtime = ExtensionRuntime::new();
        let observer = Arc::new(runtime.clone());
        let first_bus = EventBusController::new().bus().clone();
        let second_bus = EventBusController::new().bus().clone();
        let panic_resource = Arc::new(PanickingSubscriptionDrop);
        let _first = runtime.track_event_bus_subscription(first_bus.on(
            "panic-unsubscribe",
            Arc::new(move |_| {
                std::hint::black_box(&panic_resource);
            }),
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let _second = runtime.track_event_bus_subscription(second_bus.on(
            "retirement-probe",
            Arc::new(move |_| {
                counter.fetch_add(1, Ordering::Relaxed);
            }),
        ));
        let dropped = Arc::new(AtomicBool::new(false));
        let resource = Arc::new(PanickingBoundResource(ReentrantResource {
            observer: Arc::downgrade(&observer),
            bus: second_bus.clone(),
            dropped: dropped.clone(),
        }));
        let weak = Arc::downgrade(&resource);
        let mut bound_actions = actions();
        bound_actions.get_settings =
            Arc::new(move || json!({"resourceAlive": !resource.0.dropped.load(Ordering::Acquire)}));
        let core_dropped = Arc::new(AtomicBool::new(false));
        let core_resource = Arc::new(PanickingBoundResource(ReentrantResource {
            observer: Arc::downgrade(&observer),
            bus: second_bus,
            dropped: core_dropped.clone(),
        }));
        let core_weak = Arc::downgrade(&core_resource);
        let mut context = context_actions();
        context.get_model = Arc::new(move || {
            std::hint::black_box(&core_resource);
            None
        });
        let runner = ExtensionRunner::new(
            Vec::new(),
            runtime.clone(),
            ".",
            Arc::new(()),
            Arc::new(NoopProviderRegistry),
        );
        runner.bind_core(Arc::new(bound_actions), Arc::new(context), None);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runner.invalidate(Some("panic retirement"))
        }))
        .expect_err("Original unsubscribe error must propagate");
        assert_eq!(
            panic.downcast_ref::<&str>().copied(),
            Some("original unsubscribe panic")
        );
        assert!(
            dropped.load(Ordering::Acquire),
            "An unsubscribe panic must not strand the remaining binding resource"
        );
        assert!(weak.upgrade().is_none());
        assert!(core_dropped.load(Ordering::Acquire));
        assert!(core_weak.upgrade().is_none());
        assert_eq!(
            calls.load(Ordering::Acquire),
            0,
            "Every old subscription must be cancelled before any binding resource is dropped"
        );
        runner.bind_core(Arc::new(actions()), Arc::new(context_actions()), None);
        assert_eq!(runtime.create_context().err().unwrap(), "panic retirement");
        drop(runner);
        drop(observer);
        drop(runtime);
    });
}

struct IndependentPanickingResource {
    message: &'static str,
    dropped: Arc<AtomicBool>,
}
impl Drop for IndependentPanickingResource {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
        panic!("{}", self.message);
    }
}

#[test]
fn multiple_panicking_callback_retirement_isolated() {
    const CHILD_ENV: &str = "PISPER_LIFECYCLE_PANIC_CHILD";
    if let Ok(scenario) = std::env::var(CHILD_ENV) {
        let runtime = ExtensionRuntime::new();
        let runner = ExtensionRunner::new(
            Vec::new(),
            runtime.clone(),
            ".",
            Arc::new(()),
            Arc::new(NoopProviderRegistry),
        );
        let bus = EventBusController::new().bus().clone();
        let subscription_resource = Arc::new(PanickingSubscriptionDrop);
        let _tracked = runtime.track_event_bus_subscription(bus.on(
            "panic-unsubscribe",
            Arc::new(move |_| {
                std::hint::black_box(&subscription_resource);
            }),
        ));
        let first_dropped = Arc::new(AtomicBool::new(false));
        let second_dropped = Arc::new(AtomicBool::new(false));
        let first = Arc::new(IndependentPanickingResource {
            message: "first callback panic",
            dropped: first_dropped.clone(),
        });
        let second = Arc::new(IndependentPanickingResource {
            message: "second callback panic",
            dropped: second_dropped.clone(),
        });
        let first_weak = Arc::downgrade(&first);
        let second_weak = Arc::downgrade(&second);
        let mut bound_actions = actions();
        let mut provider_actions = ProviderActions::default();
        match scenario.as_str() {
            "actions" => {
                bound_actions.get_session_name = Arc::new(move || {
                    std::hint::black_box(&first);
                    None
                });
                bound_actions.get_settings = Arc::new(move || {
                    std::hint::black_box(&second);
                    Value::Null
                });
            }
            "provider" => {
                provider_actions.register_provider = Some(Arc::new(move |_, _| {
                    std::hint::black_box(&first);
                    Ok(())
                }));
                provider_actions.unregister_provider = Some(Arc::new(move |_| {
                    std::hint::black_box(&second);
                    Ok(())
                }));
            }
            "virtual" => {
                provider_actions.register_virtual_model = Some(Arc::new(move |_| {
                    std::hint::black_box(&first);
                    Ok(())
                }));
                provider_actions.unregister_virtual_model = Some(Arc::new(move |_, _| {
                    std::hint::black_box(&second);
                    Ok(())
                }));
            }
            _ => panic!("Unknown isolated panic fixture"),
        }
        runner.bind_core(
            Arc::new(bound_actions),
            Arc::new(context_actions()),
            Some(provider_actions),
        );
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runner.invalidate(Some("multiple panic retirement"))
        }))
        .expect_err("The original unsubscribe panic must propagate after all cleanup");
        assert_eq!(
            panic.downcast_ref::<&str>().copied(),
            Some("original unsubscribe panic")
        );
        assert!(first_dropped.load(Ordering::Acquire));
        assert!(second_dropped.load(Ordering::Acquire));
        assert!(first_weak.upgrade().is_none());
        assert!(second_weak.upgrade().is_none());
        assert_eq!(
            runtime.assert_active().unwrap_err(),
            "multiple panic retirement"
        );
        println!("multiple-retirement-cleanup-complete:{scenario}");
        return;
    }
    // Actual double-panic abort is isolated to the selected test executable.
    // No process-wide env mutation, oracle fixtures, host providers or files.
    let mut failures = Vec::new();
    for scenario in ["actions", "provider", "virtual"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "extension_lifecycle_regressions::multiple_panicking_callback_retirement_isolated",
                "--nocapture",
            ])
            .env(CHILD_ENV, scenario)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let panic_evidence = stderr
            .lines()
            .filter(|line| {
                line.contains("callback panic")
                    || line.contains("unsubscribe panic")
                    || line.contains("panic in a destructor")
                    || line.contains("non-unwinding panic")
            })
            .collect::<Vec<_>>()
            .join(" | ");
        eprintln!("isolated {scenario}: {}; {panic_evidence}", output.status);
        if !output.status.success()
            || !stdout.contains(&format!("multiple-retirement-cleanup-complete:{scenario}"))
        {
            failures.push(format!("{scenario}: {}", output.status));
        }
    }
    assert!(
        failures.is_empty(),
        "Every SDK-owned aggregate must finish individual callback cleanup: {failures:?}"
    );
}

#[test]
fn invalidation_keeps_external_shared_action_owners_until_their_last_release() {
    let runtime = ExtensionRuntime::new();
    let runner = ExtensionRunner::new(
        Vec::new(),
        runtime.clone(),
        ".",
        Arc::new(()),
        Arc::new(NoopProviderRegistry),
    );
    let resource = Arc::new("externally shared action resource".to_string());
    let weak = Arc::downgrade(&resource);
    let mut bound_actions = actions();
    bound_actions.get_settings = Arc::new(move || json!({"resource": resource.as_str()}));
    let shared = Arc::new(bound_actions);
    runner.bind_core(shared.clone(), Arc::new(context_actions()), None);
    runner.invalidate(Some("shared action retirement"));
    assert!(weak.upgrade().is_some());
    assert_eq!(
        runtime.get_settings().unwrap_err(),
        "shared action retirement"
    );
    drop(runner);
    drop(runtime);
    assert!(
        weak.upgrade().is_some(),
        "SDK retirement must preserve an external action owner"
    );
    drop(shared);
    assert!(weak.upgrade().is_none());
}
