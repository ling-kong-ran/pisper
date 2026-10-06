//! Tests for the `events.ts` port: the `HarnessEventBus` oracle block from
//! `packages/agent/test/harness/execution-primitives.test.ts:369-670`, ported
//! against a fixture event carrying the same variants those tests use. Two
//! additional tests pin the resnapshot hold phase and the epoch guard, which
//! the upstream block exercises only implicitly.

use super::*;
use crate::agent_core::harness::{create_context_key, with_context_value, ContextKey};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// The `TestSnapshot` payload used by the watcher tests.
#[derive(Debug, Clone, PartialEq)]
struct TestSnapshot {
    version: String,
}

/// Fixture event mirroring the variants the upstream oracle block uses.
#[derive(Debug, Clone, PartialEq)]
enum TestEvent {
    RunStart {
        run_id: String,
        started_at: i64,
        lane: String,
    },
    NavigationEnd {
        run_id: String,
        status: String,
        lane: String,
    },
    QueueUpdate {
        entry_ids: Vec<String>,
        lane: String,
    },
    ConfigUpdate {
        property: String,
        value: Vec<String>,
        previous: Vec<String>,
        lane: String,
    },
    HandlerError {
        kind: String,
        event: String,
        error: String,
        lane: Option<String>,
    },
}

impl TestEvent {
    fn event_lane(&self) -> &str {
        match self {
            TestEvent::RunStart { lane, .. }
            | TestEvent::NavigationEnd { lane, .. }
            | TestEvent::QueueUpdate { lane, .. }
            | TestEvent::ConfigUpdate { lane, .. } => lane,
            TestEvent::HandlerError { lane, .. } => lane.as_deref().unwrap_or(""),
        }
    }
}

impl BusEvent for TestEvent {
    fn event_type(&self) -> &str {
        match self {
            TestEvent::RunStart { .. } => "run_start",
            TestEvent::NavigationEnd { .. } => "navigation_end",
            TestEvent::QueueUpdate { .. } => "queue_update",
            TestEvent::ConfigUpdate { .. } => "config_update",
            TestEvent::HandlerError { .. } => HANDLER_ERROR_EVENT_TYPE,
        }
    }

    fn lane(&self) -> Option<&str> {
        match self {
            TestEvent::HandlerError { lane, .. } => lane.as_deref(),
            event => Some(event.event_lane()),
        }
    }

    fn handler_error(event_type: String, error: String, lane: Option<String>) -> Self {
        // events.ts:110-118: { type, kind: "event", event, error, lane? }.
        TestEvent::HandlerError {
            kind: "event".into(),
            event: event_type,
            error,
            lane,
        }
    }
}

fn run_start(run_id: &str) -> TestEvent {
    TestEvent::RunStart {
        run_id: run_id.into(),
        started_at: 1,
        lane: "main".into(),
    }
}

/// The upstream tests' `deferred()` helper: a one-shot gate shared between a
/// test and listener closures. Clonable (Rust closures are `Fn`), firing is
/// idempotent, and awaiting after the gate opened returns immediately.
#[derive(Clone)]
struct Gate {
    tx: tokio::sync::watch::Sender<bool>,
    rx: tokio::sync::watch::Receiver<bool>,
}

impl Gate {
    fn new() -> Self {
        let (tx, rx) = tokio::sync::watch::channel(false);
        Gate { tx, rx }
    }

    fn open(&self) {
        let _ = self.tx.send(true);
    }

    async fn wait(&self) {
        let mut rx = self.rx.clone();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }
}

/// Yield enough for every tail-chained job to finish on the current-thread
/// test runtime.
async fn settle() {
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
}

/// Poll until `predicate` holds, yielding between attempts.
async fn until(predicate: impl Fn() -> bool) {
    for _ in 0..200 {
        if predicate() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("condition did not settle");
}

#[tokio::test]
async fn buffers_between_snapshot_and_start_then_delivers_each_event_once_in_order() {
    // execution-primitives.test.ts:370-385.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let watcher = bus
        .watch(
            TestSnapshot {
                version: "tip-null".into(),
            },
            |_| true,
            Context::background(),
        )
        .unwrap();
    bus.emit(run_start("one"), Context::background()).await;
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    watcher.start(move |event, _context| {
        let sink = Arc::clone(&sink);
        Box::pin(async move {
            if let TestEvent::RunStart { run_id, .. } = &*event {
                sink.lock().unwrap().push(run_id.clone());
            }
        })
    });
    bus.emit(run_start("two"), Context::background()).await;
    settle().await;
    assert_eq!(
        watcher.snapshot(),
        TestSnapshot {
            version: "tip-null".into()
        }
    );
    assert_eq!(
        &*seen.lock().unwrap(),
        &["one".to_string(), "two".to_string()]
    );
    watcher.unsubscribe();
    bus.emit(run_start("three"), Context::background()).await;
    settle().await;
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn resnapshot_drops_pre_snapshot_delivery_when_invoked_inside_a_listener() {
    // execution-primitives.test.ts:387-462.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let listener_started = Gate::new();
    let release = Gate::new();
    let resnapshot_done = Gate::new();
    let watcher = bus
        .watch_with_resnapshot(
            TestSnapshot {
                version: "old".into(),
            },
            |_| true,
            Context::background(),
            Some(Arc::new(
                |_context: Context, mark_boundary: MarkBoundary| {
                    Box::pin(async move {
                        let mut mark_boundary = mark_boundary;
                        mark_boundary();
                        Ok(TestSnapshot {
                            version: "fresh".into(),
                        })
                    })
                },
            )),
        )
        .unwrap();
    let queue_events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let queue_sink = Arc::clone(&queue_events);
    let listener_watcher = watcher.clone();
    let (listener_started_for_listener, release_for_listener, done_for_listener) = (
        listener_started.clone(),
        release.clone(),
        resnapshot_done.clone(),
    );
    watcher.start(move |event, context| {
        let queue_sink = Arc::clone(&queue_sink);
        let listener_watcher = listener_watcher.clone();
        let done_for_listener = done_for_listener.clone();
        let release_for_listener = release_for_listener.clone();
        let listener_started = listener_started_for_listener.clone();
        Box::pin(async move {
            match &*event {
                TestEvent::RunStart { run_id, .. } if run_id == "blocking" => {
                    listener_started.open();
                    release_for_listener.wait().await;
                }
                TestEvent::NavigationEnd { .. } => {
                    listener_watcher.resnapshot(context).await.unwrap();
                    done_for_listener.open();
                }
                TestEvent::QueueUpdate { entry_ids, .. } => {
                    queue_sink.lock().unwrap().push(entry_ids[0].clone());
                }
                _ => {}
            }
        })
    });
    bus.emit(run_start("blocking"), Context::background()).await;
    listener_started.wait().await;
    let queued = bus.emit_batch(
        vec![
            TestEvent::NavigationEnd {
                run_id: "navigation".into(),
                status: "completed".into(),
                lane: "main".into(),
            },
            TestEvent::QueueUpdate {
                entry_ids: vec!["stale".into()],
                lane: "main".into(),
            },
        ],
        Context::background(),
    );
    release.open();
    tokio::join!(queued, resnapshot_done.wait());
    assert_eq!(
        watcher.snapshot(),
        TestSnapshot {
            version: "fresh".into()
        }
    );
    assert!(queue_events.lock().unwrap().is_empty());
    bus.emit(
        TestEvent::QueueUpdate {
            entry_ids: vec!["later".into()],
            lane: "main".into(),
        },
        Context::background(),
    )
    .await;
    until(|| *queue_events.lock().unwrap() == ["later".to_string()]).await;
}

#[tokio::test]
async fn resnapshot_holds_events_pushed_after_the_boundary_until_the_snapshot_is_installed() {
    // The holding phase of events.ts:257-262: pushes between the boundary
    // and the resnapshot completion replay after the new snapshot lands.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let capture_started = Gate::new();
    let release = Gate::new();
    let resnapshot_done = Gate::new();
    let (capture_started_for_capture, release_for_capture) =
        (capture_started.clone(), release.clone());
    let watcher = bus
        .watch_with_resnapshot(
            TestSnapshot {
                version: "old".into(),
            },
            |_| true,
            Context::background(),
            Some(Arc::new(
                move |_context: Context, mark_boundary: MarkBoundary| {
                    let capture_started = capture_started_for_capture.clone();
                    let release = release_for_capture.clone();
                    Box::pin(async move {
                        capture_started.open();
                        // Mark the boundary, then hold the capture open until
                        // released so post-boundary events land in `held`.
                        let mut mark_boundary = mark_boundary;
                        mark_boundary();
                        release.wait().await;
                        Ok(TestSnapshot {
                            version: "new".into(),
                        })
                    })
                },
            )),
        )
        .unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let listener_watcher = watcher.clone();
    let done_for_listener = resnapshot_done.clone();
    watcher.start(move |event, context| {
        let sink = Arc::clone(&sink);
        let listener_watcher = listener_watcher.clone();
        let done_for_listener = done_for_listener.clone();
        Box::pin(async move {
            if let TestEvent::NavigationEnd { run_id, .. } = &*event {
                if run_id == "trigger" {
                    listener_watcher.resnapshot(context).await.unwrap();
                    done_for_listener.open();
                } else {
                    sink.lock().unwrap().push(run_id.clone());
                }
            }
        })
    });
    bus.emit(
        TestEvent::NavigationEnd {
            run_id: "trigger".into(),
            status: "completed".into(),
            lane: "main".into(),
        },
        Context::background(),
    )
    .await;
    capture_started.wait().await;
    // Pushed after the boundary: must be held, not delivered yet.
    bus.emit(
        TestEvent::NavigationEnd {
            run_id: "held-1".into(),
            status: "completed".into(),
            lane: "main".into(),
        },
        Context::background(),
    )
    .await;
    settle().await;
    assert!(seen.lock().unwrap().is_empty());
    release.open();
    resnapshot_done.wait().await;
    assert_eq!(
        watcher.snapshot(),
        TestSnapshot {
            version: "new".into()
        }
    );
    until(|| *seen.lock().unwrap() == ["held-1".to_string()]).await;
}

#[tokio::test]
async fn resnapshot_before_start_drops_buffered_events_through_the_epoch_guard() {
    // events.ts:203-206 replay through `enqueue` with the push-time epoch; a
    // resnapshot between watch and start invalidates the buffered batch.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let watcher = bus
        .watch_with_resnapshot(
            TestSnapshot {
                version: "old".into(),
            },
            |_| true,
            Context::background(),
            Some(Arc::new(
                |_context: Context, mark_boundary: MarkBoundary| {
                    Box::pin(async move {
                        let mut mark_boundary = mark_boundary;
                        mark_boundary();
                        Ok(TestSnapshot {
                            version: "fresh".into(),
                        })
                    })
                },
            )),
        )
        .unwrap();
    bus.emit(run_start("stale"), Context::background()).await;
    watcher.resnapshot(Context::background()).await.unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    watcher.start(move |event, _context| {
        let sink = Arc::clone(&sink);
        Box::pin(async move {
            if let TestEvent::RunStart { run_id, .. } = &*event {
                sink.lock().unwrap().push(run_id.clone());
            }
        })
    });
    bus.emit(run_start("fresh"), Context::background()).await;
    settle().await;
    assert_eq!(
        watcher.snapshot(),
        TestSnapshot {
            version: "fresh".into()
        }
    );
    assert_eq!(&*seen.lock().unwrap(), &["fresh".to_string()]);
}

#[tokio::test]
async fn isolates_each_listener_from_payload_mutation() {
    // execution-primitives.test.ts:464-484. Rust events are immutable Arc
    // snapshots, so the isolation property holds by construction; both
    // listeners still observe the same payload.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let first: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let second: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let first_sink = Arc::clone(&first);
    let second_sink = Arc::clone(&second);
    bus.on("config_update", move |event, _context| {
        let sink = Arc::clone(&first_sink);
        Box::pin(async move {
            if let TestEvent::ConfigUpdate { value, .. } = &*event {
                sink.lock().unwrap().push(value.join(","));
            }
        })
    })
    .unwrap();
    bus.on("config_update", move |event, _context| {
        let sink = Arc::clone(&second_sink);
        Box::pin(async move {
            if let TestEvent::ConfigUpdate { value, .. } = &*event {
                sink.lock().unwrap().push(value.join(","));
            }
        })
    })
    .unwrap();
    bus.emit(
        TestEvent::ConfigUpdate {
            property: "activeTools".into(),
            value: vec!["read".into()],
            previous: Vec::new(),
            lane: "main".into(),
        },
        Context::background(),
    )
    .await;
    assert_eq!(&*first.lock().unwrap(), &["read".to_string()]);
    assert_eq!(&*second.lock().unwrap(), &["read".to_string()]);
}

#[tokio::test]
async fn serializes_concurrent_publications_in_process_order() {
    // execution-primitives.test.ts:486-508.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let started = Gate::new();
    let release = Gate::new();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let (started_for_listener, release_for_listener) = (started.clone(), release.clone());
    bus.on("run_start", move |event, _context| {
        let sink = Arc::clone(&sink);
        let started = started_for_listener.clone();
        let release = release_for_listener.clone();
        Box::pin(async move {
            if let TestEvent::RunStart { run_id, .. } = &*event {
                let run_id = run_id.clone();
                sink.lock().unwrap().push(format!("{run_id}:start"));
                if run_id == "one" {
                    started.open();
                    release.wait().await;
                }
                sink.lock().unwrap().push(format!("{run_id}:end"));
            }
        })
    })
    .unwrap();
    let one = bus.emit(run_start("one"), Context::background());
    started.wait().await;
    let two = bus.emit(run_start("two"), Context::background());
    settle().await;
    assert_eq!(&*seen.lock().unwrap(), &["one:start".to_string()]);
    release.open();
    tokio::join!(one, two);
    assert_eq!(
        &*seen.lock().unwrap(),
        &[
            "one:start".to_string(),
            "one:end".to_string(),
            "two:start".to_string(),
            "two:end".to_string(),
        ]
    );
}

#[tokio::test]
async fn keeps_concurrent_batches_contiguous_with_their_emitting_contexts() {
    // execution-primitives.test.ts:510-540.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let source_key: ContextKey<String> = create_context_key("event.batch.source");
    let first_context = with_context_value(&source_key, "first".to_string(), Context::background());
    let second_context =
        with_context_value(&source_key, "second".to_string(), Context::background());
    let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let source_key_for_listener = source_key.clone();
    bus.on("run_start", move |event, context| {
        let sink = Arc::clone(&sink);
        let source_key = source_key_for_listener.clone();
        Box::pin(async move {
            let source = context
                .get(&source_key)
                .map(|value| (*value).clone())
                .unwrap_or_default();
            if let TestEvent::RunStart { run_id, .. } = &*event {
                sink.lock().unwrap().push((run_id.clone(), source));
            }
            // Yield inside the listener so the second batch would interleave
            // if publication were not serialized.
            tokio::task::yield_now().await;
        })
    })
    .unwrap();
    let first = bus.emit_batch(vec![run_start("a1"), run_start("a2")], first_context);
    let second = bus.emit_batch(vec![run_start("b1"), run_start("b2")], second_context);
    tokio::join!(first, second);
    let entries = seen.lock().unwrap().clone();
    let run_ids: Vec<&str> = entries.iter().map(|(run_id, _)| run_id.as_str()).collect();
    assert_eq!(run_ids, ["a1", "a2", "b1", "b2"]);
    for (index, (_, source)) in entries.iter().enumerate() {
        let expected = if index < 2 { "first" } else { "second" };
        assert_eq!(source, expected, "entry {index} ran with the wrong context");
    }
}

#[tokio::test]
async fn binds_listeners_and_watchers_when_a_batch_is_emitted() {
    // execution-primitives.test.ts:542-576.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let started = Gate::new();
    let release = Gate::new();
    let (started_for_listener, release_for_listener) = (started.clone(), release.clone());
    bus.on("run_start", move |event, _context| {
        let started = started_for_listener.clone();
        let release = release_for_listener.clone();
        Box::pin(async move {
            if let TestEvent::RunStart { run_id, .. } = &*event {
                if run_id == "blocking" {
                    started.open();
                    release.wait().await;
                }
            }
        })
    })
    .unwrap();
    let blocking = bus.emit(run_start("blocking"), Context::background());
    started.wait().await;
    let queued = bus.emit(run_start("queued"), Context::background());
    let late_listener_seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let late_sink = Arc::clone(&late_listener_seen);
    bus.on("run_start", move |event, _context| {
        let sink = Arc::clone(&late_sink);
        Box::pin(async move {
            if let TestEvent::RunStart { run_id, .. } = &*event {
                sink.lock().unwrap().push(run_id.clone());
            }
        })
    })
    .unwrap();
    let late_watcher_seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let watcher_sink = Arc::clone(&late_watcher_seen);
    let late_watcher = bus
        .watch(
            TestSnapshot {
                version: "v".into(),
            },
            |event| event.event_type() == "run_start",
            Context::background(),
        )
        .unwrap();
    late_watcher.start(move |event, _context| {
        let sink = Arc::clone(&watcher_sink);
        Box::pin(async move {
            if let TestEvent::RunStart { run_id, .. } = &*event {
                sink.lock().unwrap().push(run_id.clone());
            }
        })
    });
    release.open();
    tokio::join!(blocking, queued);
    settle().await;
    assert!(late_listener_seen.lock().unwrap().is_empty());
    assert!(late_watcher_seen.lock().unwrap().is_empty());
    bus.emit(run_start("later"), Context::background()).await;
    settle().await;
    assert_eq!(&*late_listener_seen.lock().unwrap(), &["later".to_string()]);
    assert_eq!(&*late_watcher_seen.lock().unwrap(), &["later".to_string()]);
}

#[tokio::test]
async fn resolves_an_empty_batch_without_delivery() {
    // execution-primitives.test.ts:578-586.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_sink = Arc::clone(&calls);
    bus.on("run_start", move |_event, _context| {
        let calls = Arc::clone(&calls_sink);
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
        })
    })
    .unwrap();
    bus.emit_batch(Vec::new(), Context::background()).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn drains_emitted_batches_during_close_and_ignores_later_publication() {
    // execution-primitives.test.ts:588-619.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let started = Gate::new();
    let release = Gate::new();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let (started_for_listener, release_for_listener) = (started.clone(), release.clone());
    bus.on("run_start", move |event, _context| {
        let sink = Arc::clone(&sink);
        let started = started_for_listener.clone();
        let release = release_for_listener.clone();
        Box::pin(async move {
            if let TestEvent::RunStart { run_id, .. } = &*event {
                let run_id = run_id.clone();
                sink.lock().unwrap().push(run_id.clone());
                if run_id == "blocking" {
                    started.open();
                    release.wait().await;
                }
            }
        })
    })
    .unwrap();
    let blocking = bus.emit(run_start("blocking"), Context::background());
    started.wait().await;
    let batch = bus.emit_batch(
        vec![run_start("one"), run_start("two")],
        Context::background(),
    );
    bus.close(anyhow!("closed"));
    bus.emit(run_start("late"), Context::background()).await;
    release.open();
    tokio::join!(blocking, batch);
    assert_eq!(
        &*seen.lock().unwrap(),
        &["blocking".to_string(), "one".to_string(), "two".to_string()]
    );
    // Later registrations fail with the stored close error.
    let error = bus
        .on("run_start", |_event, _context| Box::pin(async {}))
        .unwrap_err();
    assert_eq!(error.to_string(), "closed");
    // A second close keeps the first error.
    bus.close(anyhow!("second"));
    let error = bus
        .watch(
            TestSnapshot {
                version: "v".into(),
            },
            |_| true,
            Context::background(),
        )
        .unwrap_err();
    assert_eq!(error.to_string(), "closed");
}

#[tokio::test]
async fn continues_watcher_delivery_after_listener_failure_and_reports_it() {
    // execution-primitives.test.ts:621-640.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let failures: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let failure_sink = Arc::clone(&failures);
    bus.on(HANDLER_ERROR_EVENT_TYPE, move |event, _context| {
        let sink = Arc::clone(&failure_sink);
        Box::pin(async move {
            if let TestEvent::HandlerError { error, .. } = &*event {
                sink.lock().unwrap().push(error.clone());
            }
        })
    })
    .unwrap();
    let watcher = bus
        .watch(
            TestSnapshot {
                version: "v".into(),
            },
            |event| event.event_type() == "run_start",
            Context::background(),
        )
        .unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    watcher.start(move |event, _context| {
        let sink = Arc::clone(&sink);
        Box::pin(async move {
            if let TestEvent::RunStart { run_id, .. } = &*event {
                if run_id == "one" {
                    panic!("watcher failed");
                }
                sink.lock().unwrap().push(run_id.clone());
            }
        })
    });
    bus.emit(run_start("one"), Context::background()).await;
    bus.emit(run_start("two"), Context::background()).await;
    settle().await;
    assert_eq!(&*seen.lock().unwrap(), &["two".to_string()]);
    assert_eq!(&*failures.lock().unwrap(), &["watcher failed".to_string()]);
}

#[tokio::test]
async fn isolates_listener_failures_and_emits_handler_error() {
    // execution-primitives.test.ts:642-653.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let failures: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let failure_sink = Arc::clone(&failures);
    bus.on("run_start", |_event, _context| {
        Box::pin(async {
            panic!("listener failed");
        })
    })
    .unwrap();
    bus.on(HANDLER_ERROR_EVENT_TYPE, move |event, _context| {
        let sink = Arc::clone(&failure_sink);
        Box::pin(async move {
            if let TestEvent::HandlerError { error, .. } = &*event {
                sink.lock().unwrap().push(error.clone());
            }
        })
    })
    .unwrap();
    bus.emit(run_start("run"), Context::background()).await;
    assert_eq!(&*failures.lock().unwrap(), &["listener failed".to_string()]);
}

#[tokio::test]
async fn isolates_synchronous_prologue_listener_failures() {
    // events.ts:145-147 catches the listener call itself, so a failure in
    // the synchronous prologue — before the returned future is ever polled —
    // is isolated and reported like a polled-future failure.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let failures: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let failure_sink = Arc::clone(&failures);
    bus.on("run_start", |_event, _context| -> BoxFuture<'static, ()> {
        panic!("sync listener failed");
    })
    .unwrap();
    bus.on(HANDLER_ERROR_EVENT_TYPE, move |event, _context| {
        let sink = Arc::clone(&failure_sink);
        Box::pin(async move {
            if let TestEvent::HandlerError { error, .. } = &*event {
                sink.lock().unwrap().push(error.clone());
            }
        })
    })
    .unwrap();
    bus.emit(run_start("run"), Context::background()).await;
    assert_eq!(
        &*failures.lock().unwrap(),
        &["sync listener failed".to_string()]
    );
}

#[tokio::test]
async fn does_not_recurse_when_a_handler_error_listener_fails() {
    // execution-primitives.test.ts:655-669.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let handler_errors = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&handler_errors);
    bus.on("run_start", |_event, _context| {
        Box::pin(async {
            panic!("listener failed");
        })
    })
    .unwrap();
    bus.on(HANDLER_ERROR_EVENT_TYPE, move |_event, _context| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            panic!("error listener failed");
        })
    })
    .unwrap();
    bus.emit(run_start("run"), Context::background()).await;
    assert_eq!(handler_errors.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn watch_from_snapshot_buffers_events_emitted_during_the_initial_capture() {
    // events.ts:58-76: the watcher installs before the capture runs, so
    // events during the capture buffer and replay after start.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let capture_started = Gate::new();
    let release = Gate::new();
    let (capture_started_for_capture, release_for_capture) =
        (capture_started.clone(), release.clone());
    let capture: SnapshotCapture<TestSnapshot> = Arc::new(move |_context: Context| {
        let capture_started = capture_started_for_capture.clone();
        let release = release_for_capture.clone();
        Box::pin(async move {
            capture_started.open();
            release.wait().await;
            Ok(TestSnapshot {
                version: "initial".into(),
            })
        })
    });
    let capture_fut = bus.watch_from_snapshot(
        capture,
        |event| event.event_type() == "run_start",
        Context::background(),
    );
    let driver = async {
        capture_started.wait().await;
        // Emitted while the capture is in flight: buffered (Buffering state).
        bus.emit(run_start("during-capture"), Context::background())
            .await;
        release.open();
    };
    let (watcher, ()) = tokio::join!(capture_fut, driver);
    let watcher = watcher.unwrap();
    assert_eq!(
        watcher.snapshot(),
        TestSnapshot {
            version: "initial".into()
        }
    );
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    watcher.start(move |event, _context| {
        let sink = Arc::clone(&sink);
        Box::pin(async move {
            if let TestEvent::RunStart { run_id, .. } = &*event {
                sink.lock().unwrap().push(run_id.clone());
            }
        })
    });
    bus.emit(run_start("after-start"), Context::background())
        .await;
    settle().await;
    assert_eq!(
        &*seen.lock().unwrap(),
        &["during-capture".to_string(), "after-start".to_string()]
    );
}

#[tokio::test]
async fn unsubscribe_stops_delivery_and_unregister_is_idempotent() {
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = Arc::clone(&calls);
    let unsubscribe = bus
        .on("run_start", move |_event, _context| {
            let sink = Arc::clone(&sink);
            Box::pin(async move {
                sink.fetch_add(1, Ordering::SeqCst);
            })
        })
        .unwrap();
    bus.emit(run_start("first"), Context::background()).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    unsubscribe.unsubscribe();
    unsubscribe.unsubscribe(); // Idempotent, like Set.delete.
    bus.emit(run_start("second"), Context::background()).await;
    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn handler_error_events_carry_the_failed_type_and_lane() {
    // events.ts:110-118 via the fixture's BusEvent impl.
    let event = TestEvent::handler_error(
        "navigation_end".into(),
        "listener failed".into(),
        Some("worker".into()),
    );
    assert_eq!(event.event_type(), "handler_error");
    match &event {
        TestEvent::HandlerError {
            kind,
            event: failed_type,
            error,
            lane,
        } => {
            assert_eq!(kind, "event");
            assert_eq!(failed_type, "navigation_end");
            assert_eq!(error, "listener failed");
            assert_eq!(lane.as_deref(), Some("worker"));
        }
        other => panic!("expected handler_error variant, got {other:?}"),
    }
    // A laneless source produces a laneless handler_error (events.ts:115).
    let laneless = TestEvent::handler_error("fault".into(), "boom".into(), None);
    assert_eq!(laneless.lane(), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resnapshot_boundary_and_concurrent_publications_do_not_deadlock_or_corrupt() {
    // Contention smoke for the atomic chain: a resnapshot boundary enqueued
    // from inside a watcher listener while four publishers keep the bus tail
    // busy must neither deadlock (the boundary awaits every batch appended
    // before it) nor corrupt later delivery.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    let listener_started = Gate::new();
    let release = Gate::new();
    let resnapshot_done = Gate::new();
    let watcher = bus
        .watch_with_resnapshot(
            TestSnapshot {
                version: "old".into(),
            },
            |_| true,
            Context::background(),
            Some(Arc::new(
                |_context: Context, mark_boundary: MarkBoundary| {
                    Box::pin(async move {
                        let mut mark_boundary = mark_boundary;
                        mark_boundary();
                        Ok(TestSnapshot {
                            version: "fresh".into(),
                        })
                    })
                },
            )),
        )
        .unwrap();
    let listener_watcher = watcher.clone();
    let done_for_listener = resnapshot_done.clone();
    let (listener_started_for_listener, release_for_listener) =
        (listener_started.clone(), release.clone());
    watcher.start(move |event, context| {
        let listener_watcher = listener_watcher.clone();
        let done_for_listener = done_for_listener.clone();
        let listener_started = listener_started_for_listener.clone();
        let release = release_for_listener.clone();
        Box::pin(async move {
            if let TestEvent::NavigationEnd { run_id, .. } = &*event {
                if run_id == "trigger" {
                    listener_started.open();
                    release.wait().await;
                    listener_watcher.resnapshot(context).await.unwrap();
                    done_for_listener.open();
                }
            }
        })
    });
    bus.emit(
        TestEvent::NavigationEnd {
            run_id: "trigger".into(),
            status: "completed".into(),
            lane: "main".into(),
        },
        Context::background(),
    )
    .await;
    listener_started.wait().await;
    // Publishers keep appending to the tail while the trigger's listener is
    // parked; the resnapshot boundary (appended after `release`) must chain
    // behind all of them.
    let mut publishers = Vec::new();
    for index in 0..4 {
        let bus = bus.clone();
        publishers.push(tokio::spawn(async move {
            for step in 0..25 {
                bus.emit(
                    TestEvent::QueueUpdate {
                        entry_ids: vec![format!("p{index}-{step}")],
                        lane: "main".into(),
                    },
                    Context::background(),
                )
                .await;
            }
        }));
    }
    release.open();
    for publisher in publishers {
        publisher.await.unwrap();
    }
    resnapshot_done.wait().await;
    assert_eq!(
        watcher.snapshot(),
        TestSnapshot {
            version: "fresh".into()
        }
    );
    // The bus still delivers and new watchers still work after the
    // contention (a WatchHandle may only start once, so use a fresh one).
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let sentinel_delivered = Gate::new();
    let delivered_for_listener = sentinel_delivered.clone();
    let sentinel_watcher = bus
        .watch(
            TestSnapshot {
                version: "post".into(),
            },
            |event| event.event_type() == "queue_update",
            Context::background(),
        )
        .unwrap();
    sentinel_watcher.start(move |event, _context| {
        let sink = Arc::clone(&sink);
        let delivered = delivered_for_listener.clone();
        Box::pin(async move {
            if let TestEvent::QueueUpdate { entry_ids, .. } = &*event {
                sink.lock().unwrap().push(entry_ids[0].clone());
                delivered.open();
            }
        })
    });
    bus.emit(
        TestEvent::QueueUpdate {
            entry_ids: vec!["sentinel".into()],
            lane: "main".into(),
        },
        Context::background(),
    )
    .await;
    // emit waits for delivery to the watcher, whose listener has its own
    // asynchronous tail. A fixed number of yields on this thread is not a
    // completion barrier for the four-worker runtime under full-suite load.
    tokio::time::timeout(std::time::Duration::from_secs(8), sentinel_delivered.wait())
        .await
        .expect("sentinel watcher delivery must complete");
    assert_eq!(&*seen.lock().unwrap(), &["sentinel".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_publications_never_escape_the_delivery_chain() {
    // Regression for atomic tail chaining. Upstream's bind-and-chain
    // prologue is atomic (JS event loop); the port must take the previous
    // tail receiver and install its own in ONE critical section. With two
    // separate lock acquisitions, two concurrent publishers can both
    // observe an empty slot and both jobs escape the chain — running
    // concurrently and unordered, which breaks the barrier contract ("the
    // boundary runs only after every previously published batch was
    // delivered").
    //
    // Detection: 8 publishers free-run fire-and-forget appends (no
    // serialization between them) while a churner thread hammers the
    // tail-slot mutex, stretching every racy take/install window so other
    // publishers' takes land inside it. Escaped jobs deliver concurrently,
    // which the in-flight counter observes (each listener parks 300us so
    // escaped jobs must overlap in wall time). With the atomic prologue the
    // churner cannot split a take/install, so deliveries stay serialized.
    let bus: HarnessEventBus<TestEvent> = HarnessEventBus::new();
    const PUBLISHERS: usize = 8;
    const BATCHES: usize = 60;

    let delivered: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let overlaps = Arc::new(AtomicUsize::new(0));
    let handler_errors = Arc::new(AtomicUsize::new(0));
    let (sink, in_flight_sink, overlaps_sink) = (
        Arc::clone(&delivered),
        Arc::clone(&in_flight),
        Arc::clone(&overlaps),
    );
    bus.on("queue_update", move |event, _context| {
        let sink = Arc::clone(&sink);
        let in_flight = Arc::clone(&in_flight_sink);
        let overlaps = Arc::clone(&overlaps_sink);
        Box::pin(async move {
            if in_flight.fetch_add(1, Ordering::SeqCst) != 0 {
                overlaps.fetch_add(1, Ordering::SeqCst);
            }
            // Hold the delivery slot with a short busy-spin (not a timer
            // sleep, whose wake granularity dwarfs the hold) so two escaped
            // jobs must overlap in wall time, making non-serialization
            // observable.
            let hold = std::time::Instant::now();
            while hold.elapsed() < std::time::Duration::from_micros(300) {
                std::hint::spin_loop();
            }
            if let TestEvent::QueueUpdate { entry_ids, .. } = &*event {
                sink.lock()
                    .unwrap()
                    .push(entry_ids[0].parse::<u64>().unwrap());
            }
            in_flight.fetch_sub(1, Ordering::SeqCst);
        })
    })
    .unwrap();
    let handler_sink = Arc::clone(&handler_errors);
    bus.on(HANDLER_ERROR_EVENT_TYPE, move |_event, _context| {
        let handler_sink = Arc::clone(&handler_sink);
        Box::pin(async move {
            handler_sink.fetch_add(1, Ordering::SeqCst);
        })
    })
    .unwrap();

    // The churner: keep the tail-slot mutex hot so a racy take/install
    // window is stretched across other publishers' takes.
    let churn_bus = bus.clone();
    let churner = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
        while std::time::Instant::now() < deadline {
            let _guard = lock(&churn_bus.inner.delivery_tail);
            std::hint::spin_loop();
        }
    });

    let next_id = Arc::new(AtomicU64::new(0));
    let mut publishers = Vec::new();
    for _ in 0..PUBLISHERS {
        let bus = bus.clone();
        let next_id = Arc::clone(&next_id);
        publishers.push(tokio::spawn(async move {
            let mut pending = Vec::new();
            for _ in 0..BATCHES {
                let id = next_id.fetch_add(1, Ordering::SeqCst);
                // Fire-and-forget: push the chain job synchronously (inside
                // emit_batch) and collect the completion future.
                pending.push(bus.emit_batch(
                    vec![TestEvent::QueueUpdate {
                        entry_ids: vec![id.to_string()],
                        lane: "main".into(),
                    }],
                    Context::background(),
                ));
            }
            for future in pending {
                future.await;
            }
        }));
    }
    churner.join().unwrap();
    for publisher in publishers {
        publisher.await.unwrap();
    }
    // Drain: deliveries trail the appends (300us each, serialized). Bounded
    // so a lost event fails the assertions below instead of hanging.
    for _ in 0..2000 {
        if delivered.lock().unwrap().len() == PUBLISHERS * BATCHES
            && in_flight.load(Ordering::SeqCst) == 0
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let mut delivered = delivered.lock().unwrap().clone();
    assert_eq!(
        delivered.len(),
        PUBLISHERS * BATCHES,
        "every event delivered exactly once"
    );
    assert_eq!(
        overlaps.load(Ordering::SeqCst),
        0,
        "two batch jobs delivered concurrently — the tail chain leaked"
    );
    assert_eq!(
        handler_errors.load(Ordering::SeqCst),
        0,
        "listener failures during delivery"
    );
    delivered.sort_unstable();
    delivered.dedup();
    assert_eq!(
        delivered.len(),
        PUBLISHERS * BATCHES,
        "every event id delivered exactly once"
    );
}
