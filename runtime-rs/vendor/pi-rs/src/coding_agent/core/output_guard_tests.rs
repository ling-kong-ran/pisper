use super::*;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Trace {
    writes: Vec<Value>,
    sleeps: Vec<u64>,
    exits: Vec<i32>,
}
#[derive(Default)]
struct Action {
    error: Option<Arc<OutputError>>,
    hold: bool,
    throw_before: bool,
    throw_after: bool,
}
#[derive(Default)]
struct SharedState {
    trace: Mutex<Trace>,
    actions: Mutex<VecDeque<Action>>,
    held: Mutex<VecDeque<WriteCallback>>,
    on_sleep: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
struct Stream {
    name: String,
    shared: Arc<SharedState>,
}
impl OutputStream for Stream {
    fn write(
        &self,
        chunk: OutputChunk,
        encoding: Option<String>,
        callback: Option<WriteCallback>,
    ) -> Result<bool, Arc<OutputError>> {
        let (kind, value) = match chunk {
            OutputChunk::Text(s) => ("text", Value::String(s)),
            OutputChunk::Buffer(b) => ("buffer", json!(b)),
            OutputChunk::Uint8Array(b) => ("uint8", json!(b)),
        };
        self.shared
            .trace
            .lock()
            .unwrap()
            .writes
            .push(json!({"sink":self.name,"kind":kind,"value":value,"encoding":encoding}));
        let action = self
            .shared
            .actions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default();
        if action.hold {
            self.shared
                .held
                .lock()
                .unwrap()
                .push_back(callback.expect("held callback"));
            return Ok(false);
        }
        if action.throw_before {
            return Err(action.error.expect("throw error"));
        }
        if let Some(callback) = callback {
            callback(action.error.map_or(Ok(()), Err));
        }
        if action.throw_after {
            return Err(OutputError::new("late throw", Some(json!("EPIPE"))));
        }
        Ok(false)
    }
}
struct Delay(Arc<SharedState>);
impl RetryDelay for Delay {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        self.0
            .trace
            .lock()
            .unwrap()
            .sleeps
            .push(duration.as_millis() as u64);
        let hook = self.0.on_sleep.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        async { tokio::task::yield_now().await }.boxed()
    }
}
fn stream(shared: &Arc<SharedState>, name: &str) -> Arc<dyn OutputStream> {
    Arc::new(Stream {
        name: name.into(),
        shared: Arc::clone(shared),
    })
}
fn setup(actions: Vec<Action>) -> (OutputGuard, Arc<SharedState>) {
    let shared = Arc::new(SharedState {
        actions: Mutex::new(actions.into()),
        ..Default::default()
    });
    let exits = Arc::clone(&shared);
    let guard = OutputGuard::with_delay(
        stream(&shared, "stdout"),
        stream(&shared, "stderr"),
        Arc::new(move |code| exits.trace.lock().unwrap().exits.push(code)),
        Arc::new(Delay(Arc::clone(&shared))),
    );
    (guard, shared)
}
fn outcome(result: WriteResult) -> Value {
    match result {
        Ok(()) => json!({"ok":true}),
        Err(error) => {
            let mut value = json!({"ok":false,"message":error.message});
            if let Some(code) = &error.code {
                value["code"] = code.clone();
            }
            value
        }
    }
}
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}
fn check(name: &str, shared: &Arc<SharedState>, extra: Value) {
    let oracle: Value = serde_json::from_str(include_str!("output_guard_oracle.json")).unwrap();
    let expected = oracle["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap();
    let trace = shared.trace.lock().unwrap();
    let mut actual =
        json!({"name":name,"writes":trace.writes,"sleeps":trace.sleeps,"exits":trace.exits});
    actual
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    assert_eq!(&actual, expected, "upstream output-guard case {name}");
}
fn release(shared: &Arc<SharedState>) {
    let callback = shared
        .held
        .lock()
        .unwrap()
        .pop_front()
        .expect("pending write callback");
    callback(Ok(()));
}
fn progress(shared: &Arc<SharedState>, done: bool) -> Value {
    json!({"done":done,"writes":shared.trace.lock().unwrap().writes.len()})
}

#[tokio::test]
async fn output_guard_oracle_empty_and_flush() {
    let (guard, shared) = setup(vec![]);
    let before = guard.inner.queue.lock().unwrap().identity.clone();
    guard.write_raw_stdout("");
    assert!(Arc::ptr_eq(
        &before,
        &guard.inner.queue.lock().unwrap().identity
    ));
    guard.wait_for_raw_stdout_backpressure().await.unwrap();
    guard.flush_raw_stdout().await.unwrap();
    check("empty_and_flush", &shared, json!({}));
}
#[tokio::test]
async fn output_guard_oracle_exact_retry_codes_and_throw_callback_paths() {
    for code in ["ENOBUFS", "EAGAIN", "EWOULDBLOCK"] {
        let error = OutputError::new("again", Some(json!(code)));
        let (guard, shared) = setup(vec![
            Action {
                error: Some(error.clone()),
                ..Default::default()
            },
            Action {
                error: Some(error),
                throw_before: true,
                ..Default::default()
            },
        ]);
        guard.write_raw_stdout("A");
        guard.write_raw_stdout("B");
        let waited = outcome(guard.wait_for_raw_stdout_backpressure().await);
        let flushed = outcome(guard.flush_raw_stdout().await);
        settle().await;
        check(
            &format!("retry_{code}"),
            &shared,
            json!({"waited":waited,"flushed":flushed}),
        );
    }
}
#[tokio::test]
async fn output_guard_oracle_fatal_tail_stays_rejected() {
    for (label, code) in [
        ("EINTR", Some(json!("EINTR"))),
        ("EPIPE", Some(json!("EPIPE"))),
        ("enobufs", Some(json!("enobufs"))),
        ("null", Some(Value::Null)),
        ("123", Some(json!(123))),
        ("missing", None),
    ] {
        let error = OutputError::new("fatal", code);
        let (guard, shared) = setup(vec![Action {
            error: Some(error.clone()),
            ..Default::default()
        }]);
        guard.write_raw_stdout("A");
        guard.write_raw_stdout("B");
        let failed = guard.wait_for_raw_stdout_backpressure().await;
        assert!(Arc::ptr_eq(failed.as_ref().unwrap_err(), &error));
        let waited = outcome(failed);
        let flushed = outcome(guard.flush_raw_stdout().await);
        guard.write_raw_stdout("C");
        settle().await;
        check(
            &format!("fatal_{label}"),
            &shared,
            json!({"waited":waited,"flushed":flushed}),
        );
    }
}
#[tokio::test]
async fn output_guard_oracle_first_callback_settlement_wins_over_later_throw() {
    for has_error in [false, true] {
        let (guard, shared) = setup(vec![Action {
            error: has_error.then(|| OutputError::new("callback error", Some(json!("EPIPE")))),
            throw_after: true,
            ..Default::default()
        }]);
        guard.write_raw_stdout("A");
        let waited = outcome(guard.wait_for_raw_stdout_backpressure().await);
        settle().await;
        check(
            &format!("callback_then_throw_{has_error}"),
            &shared,
            json!({"waited":waited}),
        );
    }
}
#[tokio::test]
async fn output_guard_oracle_flush_error_does_not_poison_tail_or_exit() {
    let (guard, shared) = setup(vec![Action {
        error: Some(OutputError::new("flush only", Some(json!("EPIPE")))),
        ..Default::default()
    }]);
    let flushed = outcome(guard.flush_raw_stdout().await);
    guard.write_raw_stdout("after");
    let waited = outcome(guard.wait_for_raw_stdout_backpressure().await);
    settle().await;
    check(
        "flush_error_does_not_poison_tail",
        &shared,
        json!({"flushed":flushed,"waited":waited}),
    );
}
#[tokio::test]
async fn output_guard_oracle_backpressure_rechecks_tail_appended_at_callback() {
    let (guard, shared) = setup(vec![
        Action {
            hold: true,
            ..Default::default()
        },
        Action {
            hold: true,
            ..Default::default()
        },
    ]);
    guard.write_raw_stdout("A");
    let done = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    let g = guard.clone();
    let waiter = tokio::spawn(async move {
        g.wait_for_raw_stdout_backpressure().await.unwrap();
        d.store(true, Ordering::SeqCst);
    });
    settle().await;
    let mut checks = vec![progress(&shared, done.load(Ordering::SeqCst))];
    release(&shared);
    guard.write_raw_stdout("B");
    settle().await;
    checks.push(progress(&shared, done.load(Ordering::SeqCst)));
    release(&shared);
    waiter.await.unwrap();
    checks.push(progress(&shared, done.load(Ordering::SeqCst)));
    check(
        "backpressure_rechecks_tail",
        &shared,
        json!({"checks":checks}),
    );
}
#[tokio::test]
async fn output_guard_oracle_flush_waits_for_empty_callback() {
    let (guard, shared) = setup(vec![
        Action {
            hold: true,
            ..Default::default()
        },
        Action {
            hold: true,
            ..Default::default()
        },
    ]);
    guard.write_raw_stdout("A");
    let done = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    let g = guard.clone();
    let waiter = tokio::spawn(async move {
        g.flush_raw_stdout().await.unwrap();
        d.store(true, Ordering::SeqCst);
    });
    settle().await;
    let mut checks = vec![progress(&shared, done.load(Ordering::SeqCst))];
    release(&shared);
    settle().await;
    checks.push(progress(&shared, done.load(Ordering::SeqCst)));
    release(&shared);
    waiter.await.unwrap();
    checks.push(progress(&shared, done.load(Ordering::SeqCst)));
    check(
        "flush_waits_for_empty_callback",
        &shared,
        json!({"checks":checks}),
    );
}
#[tokio::test]
async fn output_guard_oracle_takeover_routes_callbacks_coercion_and_boolean() {
    let (guard, shared) = setup(vec![]);
    let mut states = vec![guard.is_stdout_taken_over()];
    guard.take_over_stdout();
    guard.take_over_stdout();
    states.push(guard.is_stdout_taken_over());
    let callbacks = Arc::new(Mutex::new(Vec::<Value>::new()));
    let mut returns = vec![];
    for (chunk, encoding) in [
        (OutputChunk::Text("log".into()), Some("latin1".into())),
        (OutputChunk::Buffer(vec![65, 255]), None),
        (OutputChunk::Uint8Array(vec![65, 255]), None),
    ] {
        let target = callbacks.clone();
        returns.push(
            guard
                .write_stdout(
                    chunk,
                    encoding,
                    Some(Box::new(move |result| {
                        target.lock().unwrap().push(
                            result
                                .err()
                                .map(|e| json!(e.message))
                                .unwrap_or(Value::Null),
                        );
                    })),
                )
                .unwrap(),
        );
    }
    guard.write_raw_stdout("protocol");
    guard.wait_for_raw_stdout_backpressure().await.unwrap();
    guard.restore_stdout();
    guard.restore_stdout();
    states.push(guard.is_stdout_taken_over());
    guard
        .write_stdout(
            OutputChunk::Text("normal".into()),
            Some("utf8".into()),
            Some(Box::new(|_| {})),
        )
        .unwrap();
    check(
        "takeover_routes_and_restores",
        &shared,
        json!({"states":states,"returns":returns,"callbacks":*callbacks.lock().unwrap()}),
    );
}
#[tokio::test]
async fn output_guard_oracle_takeover_captures_writer_identity() {
    let (guard, shared) = setup(vec![]);
    guard.set_stdout_writer(stream(&shared, "out2"));
    guard.set_stderr_writer(stream(&shared, "err2"));
    guard.take_over_stdout();
    let log = |text: &str| {
        guard
            .write_stdout(OutputChunk::Text(text.into()), None, Some(Box::new(|_| {})))
            .unwrap()
    };
    log("log");
    guard.set_stdout_writer(stream(&shared, "out3"));
    guard.set_stderr_writer(stream(&shared, "err3"));
    guard.take_over_stdout();
    log("overridden");
    guard.write_raw_stdout("raw saved");
    guard.wait_for_raw_stdout_backpressure().await.unwrap();
    guard.restore_stdout();
    log("restored");
    guard.take_over_stdout();
    log("new takeover");
    guard.write_raw_stdout("raw again");
    guard.wait_for_raw_stdout_backpressure().await.unwrap();
    check("takeover_captures_writer_identity", &shared, json!({}));
}
#[tokio::test]
async fn output_guard_oracle_retry_rereads_current_writer() {
    let (guard, shared) = setup(vec![Action {
        error: Some(OutputError::new("retry", Some(json!("EAGAIN")))),
        ..Default::default()
    }]);
    let g = guard.clone();
    let replacement = stream(&shared, "replacement");
    *shared.on_sleep.lock().unwrap() = Some(Box::new(move || g.set_stdout_writer(replacement)));
    guard.write_raw_stdout("A");
    guard.wait_for_raw_stdout_backpressure().await.unwrap();
    check("retry_rereads_current_writer", &shared, json!({}));
}
#[tokio::test(start_paused = true)]
async fn output_guard_real_retry_delay_is_ten_milliseconds() {
    let (_, shared) = setup(vec![Action {
        error: Some(OutputError::new("again", Some(json!("EAGAIN")))),
        ..Default::default()
    }]);
    let guard = OutputGuard::new(
        stream(&shared, "stdout"),
        stream(&shared, "stderr"),
        Arc::new(|_| panic!("unexpected exit")),
    );
    guard.write_raw_stdout("A");
    settle().await;
    assert_eq!(shared.trace.lock().unwrap().writes.len(), 1);
    tokio::time::advance(Duration::from_millis(9)).await;
    settle().await;
    assert_eq!(shared.trace.lock().unwrap().writes.len(), 1);
    tokio::time::advance(Duration::from_millis(1)).await;
    guard.wait_for_raw_stdout_backpressure().await.unwrap();
    assert_eq!(shared.trace.lock().unwrap().writes.len(), 2);
}
#[tokio::test]
async fn output_guard_cancelling_a_waiter_does_not_cancel_queued_io() {
    let (guard, shared) = setup(vec![Action {
        hold: true,
        ..Default::default()
    }]);
    guard.write_raw_stdout("A");
    guard.write_raw_stdout("B");
    let g = guard.clone();
    let waiter = tokio::spawn(async move { g.wait_for_raw_stdout_backpressure().await });
    settle().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    release(&shared);
    guard.flush_raw_stdout().await.unwrap();
    let trace = shared.trace.lock().unwrap();
    assert_eq!(
        trace
            .writes
            .iter()
            .map(|w| w["value"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["A", "B", ""]
    );
}
#[test]
fn output_guard_native_errno_classification_is_narrow() {
    use std::io::{Error, ErrorKind};
    let mapped = process_stream::classify_for_test(Error::from(ErrorKind::WouldBlock)).unwrap_err();
    assert_eq!(mapped.code, Some(json!("EAGAIN")));
    for kind in [
        ErrorKind::Interrupted,
        ErrorKind::BrokenPipe,
        ErrorKind::OutOfMemory,
    ] {
        assert!(!process_stream::classify_for_test(Error::from(kind))
            .unwrap_err()
            .retryable());
    }
    #[cfg(windows)]
    for (code, expected) in [
        (10055, Some("ENOBUFS")),
        (10035, Some("EAGAIN")),
        (232, Some("EAGAIN")),
        (109, None),
        (1450, None),
    ] {
        assert_eq!(
            process_stream::classify_for_test(Error::from_raw_os_error(code))
                .unwrap_err()
                .code,
            expected.map(|s| json!(s))
        );
    }
    #[cfg(unix)]
    for (code, expected) in [
        (libc::ENOBUFS, Some("ENOBUFS")),
        (libc::EAGAIN, Some("EAGAIN")),
        (libc::EWOULDBLOCK, Some("EAGAIN")),
        (libc::EINTR, None),
        (libc::ENOMEM, None),
    ] {
        assert_eq!(
            process_stream::classify_for_test(Error::from_raw_os_error(code))
                .unwrap_err()
                .code,
            expected.map(|s| json!(s))
        );
    }
}
#[test]
fn output_guard_native_subprocess_entry() {
    if std::env::var("PI_RUST_OUTPUT_GUARD_CHILD").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let guard = OutputGuard::process();
            guard.take_over_stdout();
            let (sender, receiver) = oneshot::channel();
            guard
                .write_stdout(
                    OutputChunk::Text("r10-diagnostic-only\n".into()),
                    None,
                    Some(Box::new(move |r| {
                        let _ = sender.send(r);
                    })),
                )
                .unwrap();
            receiver.await.unwrap().unwrap();
            use crate::coding_agent::agent_session::AgentSessionEvent;
            use crate::coding_agent::modes::json_event::to_json_event_string;
            for event in [AgentSessionEvent::AgentStart, AgentSessionEvent::TurnStart] {
                guard.write_raw_stdout(&(to_json_event_string(&event).unwrap() + "\n"));
            }
            guard.flush_raw_stdout().await.unwrap();
            guard.restore_stdout();
        });
}
#[test]
fn output_guard_native_process_output_uses_real_pipes_and_json_writer() {
    let result = output_guard_child(
        "output_guard_native_subprocess_entry",
        "PI_RUST_OUTPUT_GUARD_CHILD",
        "1",
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stdout.contains("{\"type\":\"agent_start\"}\n{\"type\":\"turn_start\"}\n"),
        "{stdout}"
    );
    assert!(!stdout.contains("r10-diagnostic-only"));
    assert_eq!(stderr, "r10-diagnostic-only\n");
}

/// Kept in a subprocess: a regression in tail structure must fail a test, not
/// take down the whole all-targets test runner with a Rust stack overflow.
#[test]
fn output_guard_backlog_subprocess_entry() {
    let Ok(case_name) = std::env::var("PI_RUST_OUTPUT_GUARD_BACKLOG_CASE") else {
        return;
    };
    let fail = case_name == "fatal_backlog";
    assert!(fail || case_name == "success_backlog");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (guard, shared) = setup(if fail {
                vec![Action {
                    error: Some(OutputError::new("backlog failure", Some(json!("EPIPE")))),
                    ..Default::default()
                }]
            } else {
                vec![]
            });
            let count = 4096;
            // Deliberately no yield before waiting: JS Promise chaining is
            // stack safe even when all writes are enqueued in one call stack.
            for index in 0..count {
                guard.write_raw_stdout(&index.to_string());
            }
            let failure = guard.flush_raw_stdout().await.err().map(|error| {
                json!({"message":error.message,"code":error.code})
            });
            settle().await;
            let trace = shared.trace.lock().unwrap();
            let writes: Vec<_> = trace.writes.iter().map(|row| row["value"].as_str().unwrap()).collect();
            let mut exit_codes = trace.exits.clone();
            exit_codes.sort_unstable();
            exit_codes.dedup();
            let actual = json!({
                "name":case_name,"count":count,"writes":writes.len(),
                "ordered":writes.iter().enumerate().all(|(i,text)| *text == if i<count {i.to_string()} else {String::new()}),
                "first":writes.first(),"last":writes.last(),"failure":failure,
                "exits":trace.exits.len(),"exitCodes":exit_codes
            });
            let oracle: Value = serde_json::from_str(include_str!("output_guard_backlog_oracle.json")).unwrap();
            let expected = oracle["cases"].as_array().unwrap().iter().find(|row| row["name"] == case_name).unwrap();
            assert_eq!(&actual, expected);
        });
}

#[test]
fn output_guard_large_backlogs_match_actual_upstream_without_recursive_polling() {
    let mut failures = vec![];
    for case in ["success_backlog", "fatal_backlog"] {
        let result = output_guard_child(
            "output_guard_backlog_subprocess_entry",
            "PI_RUST_OUTPUT_GUARD_BACKLOG_CASE",
            case,
        );
        if !result.status.success() {
            failures.push(format!(
                "{case}: {}\n{}\n{}",
                result.status,
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// Each helper emits only a small test summary. Keep child regressions bounded
// and reap the exact process we started, including a callback deadlock.
fn output_guard_child(entry: &str, variable: &str, value: &str) -> std::process::Output {
    use std::process::{Command, Stdio};
    use std::time::Instant;
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("coding_agent::core::output_guard::tests::{entry}"),
            "--nocapture",
            "--test-threads=1",
        ])
        .env(variable, value)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let result = child.wait_with_output().unwrap();
            panic!(
                "output-guard child {entry}/{value} timed out: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
