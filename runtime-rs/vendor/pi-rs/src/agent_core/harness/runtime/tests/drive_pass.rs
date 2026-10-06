use super::super::drive_pass::*;
use crate::agent_core::harness::context::{create_context_key, with_abort_signal, Context};
use crate::agent_core::harness::execution::GateRejection;
use futures::FutureExt;
use serde_json::json;
use std::sync::{Arc, Barrier};
use tokio_util::sync::CancellationToken;

fn options() -> DriveOptions {
    DriveOptions {
        operation_id: "operation".into(),
        wait_for_retry: None,
        poll_deferred: None,
    }
}
fn drive() -> Drive {
    Drive::new(&options(), Context::background())
}
fn waiting(at: i64) -> DriveOutcome {
    DriveOutcome::Waiting {
        operation_id: "operation".into(),
        reason: WaitingReason::Retry { not_before: at },
    }
}
fn error(text: &str) -> DriveError {
    Arc::new(std::io::Error::other(text.to_owned()))
}

#[test]
fn option_defaults_and_context_detachment_preserve_values_and_caller_options() {
    let key = create_context_key::<String>("trace identity");
    let value_context = Context::background().with_value(&key, "trace".to_owned());
    let identity = value_context.get(&key).unwrap();
    for wait_for_retry in [None, Some(false), Some(true)] {
        for poll_deferred in [None, Some(false), Some(true)] {
            let caller_signal = CancellationToken::new();
            let caller = with_abort_signal(caller_signal.clone(), value_context.clone());
            let options = DriveOptions {
                wait_for_retry,
                poll_deferred,
                ..options()
            };
            let before = options.clone();
            let drive = Drive::new(&options, caller.clone());
            assert_eq!(options, before);
            assert_eq!(drive.operation_id(), "operation");
            assert_eq!(drive.wait_for_retry(), wait_for_retry == Some(true));
            assert_eq!(
                drive.deferred_permits(),
                usize::from(poll_deferred == Some(true))
            );
            assert!(Arc::ptr_eq(&identity, &drive.context().get(&key).unwrap()));
            assert!(drive.context().abort_signal().is_none());
            caller_signal.cancel();
            assert!(caller.abort_signal().unwrap().is_cancelled());
            assert!(drive.context().abort_signal().is_none());
            assert!(!drive.gate().signal().is_cancelled());
            assert!(!drive.close_signal().is_cancelled());
            let already_cancelled = Drive::new(&options, caller);
            assert!(already_cancelled.context().abort_signal().is_none());
            assert!(!already_cancelled.gate().signal().is_cancelled());
        }
    }
}

#[test]
fn native_drive_option_and_outcome_wire_shapes_round_trip_all_three_leaves() {
    for wire in [
        json!({"operationId":"op"}),
        json!({"operationId":"op","waitForRetry":false,"pollDeferred":true}),
    ] {
        let parsed: DriveOptions = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), wire);
    }
    for data in [
        None,
        Some(serde_json::Value::Null),
        Some(json!(false)),
        Some(json!({"nested":null})),
    ] {
        let mut wire = json!({"kind":"waiting","operationId":"op","reason":"deferred","deferred":{"provider":"p","modelId":"m","api":"a","id":"d"}});
        if let Some(data) = data {
            wire["deferred"]["data"] = data;
        }
        let parsed: DriveOutcome = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), wire);
    }
    for wire in [
        json!({"kind":"waiting","operationId":"op","reason":"retry","notBefore":42}),
        json!({"kind":"waiting","operationId":"op","reason":"deferred","deferred":{"provider":"p","modelId":"m","api":"a","id":"d"}}),
        json!({"kind":"settled","outcome":{"operationId":"op","kind":"run","status":"completed","fromTipId":null,"tipId":"e","startedAt":1,"endedAt":2}}),
    ] {
        let parsed: DriveOutcome = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), wire);
    }
}

#[tokio::test]
async fn completion_is_first_wins_replayed_to_late_waiters_with_shared_identity() {
    let drive = drive();
    drive.settle(waiting(1)); // Deliberately no receivers yet.
    let first = drive.completion().wait().await.unwrap();
    drive.settle(waiting(2));
    drive.fail(error("too late"));
    for _ in 0..32 {
        let next = drive.completion().wait().await.unwrap();
        assert_eq!(*next, waiting(1));
        assert!(Arc::ptr_eq(&first, &next));
    }
    assert!(!drive.gate().signal().is_cancelled());
    assert!(!drive.close_signal().is_cancelled());
}

#[tokio::test]
async fn dropping_one_waiter_or_drive_owner_does_not_reject_shared_completion() {
    let drive = drive();
    let completion = drive.completion();
    let mut cancelled_waiter = Box::pin(completion.wait());
    assert!(futures::poll!(&mut cancelled_waiter).is_pending());
    let mut retained_waiter = Box::pin(completion.wait());
    assert!(futures::poll!(&mut retained_waiter).is_pending());
    drop(cancelled_waiter);
    drive.settle(waiting(7));
    assert_eq!(*retained_waiter.await.unwrap(), waiting(7));
    let orphan = self::drive().completion();
    assert!(orphan.wait().now_or_never().is_none());
}

#[tokio::test]
async fn fail_and_repeated_close_keep_separate_first_error_identities() {
    let drive = drive();
    let failed = error("failed first");
    let closed = error("closed later");
    drive.fail(failed.clone());
    assert!(!drive.close_signal().is_cancelled());
    drive.close_gate(closed.clone());
    drive.close_gate(error("later close"));
    drive.settle(waiting(100));
    assert!(Arc::ptr_eq(
        &drive.completion().wait().await.unwrap_err(),
        &failed
    ));
    assert!(Arc::ptr_eq(&drive.close_reason().unwrap(), &closed));
    assert!(drive.close_signal().is_cancelled());
    assert!(drive.gate().signal().is_cancelled());
    match drive.gate().admit(|| ()) {
        Err(GateRejection::Closed(actual)) => assert!(Arc::ptr_eq(&actual, &closed)),
        _ => panic!("closed gate must retain its first close error"),
    }
    let pending = self::drive();
    pending.close_gate(closed.clone());
    assert!(Arc::ptr_eq(
        &pending.completion().wait().await.unwrap_err(),
        &closed
    ));
    let settled = self::drive();
    settled.settle(waiting(3));
    settled.close_gate(closed);
    assert_eq!(*settled.completion().wait().await.unwrap(), waiting(3));
    assert!(settled.close_signal().is_cancelled());
}

#[tokio::test]
async fn begin_abort_denies_admission_before_signal_without_settling_or_closing() {
    let drive = drive();
    drive.signal_abort(); // Open signalAbort is a no-op.
    assert!(!drive.gate().signal().is_cancelled());
    let committed = CancellationToken::new();
    let ignored = CancellationToken::new();
    drive.begin_abort(committed.clone());
    drive.begin_abort(ignored.clone());
    assert!(!drive.gate().signal().is_cancelled());
    match drive.gate().admit(|| panic!("effect must not be admitted")) {
        Err(GateRejection::AbortRequested(abort)) => {
            committed.cancel();
            assert!(abort.cancellation.is_cancelled());
            assert!(!ignored.is_cancelled());
        }
        _ => panic!("expected abort refusal"),
    }
    drive.signal_abort();
    assert!(drive.gate().signal().is_cancelled());
    assert!(!drive.close_signal().is_cancelled());
    assert!(drive.completion().wait().now_or_never().is_none());
    drive.close_gate(error("close after abort"));
    assert!(drive.close_signal().is_cancelled());
    assert_eq!(
        drive.completion().wait().await.unwrap_err().to_string(),
        "close after abort"
    );
}

#[test]
fn deferred_permit_reads_do_not_consume_and_parallel_consumption_cannot_underflow() {
    let options = DriveOptions {
        poll_deferred: Some(true),
        ..options()
    };
    let drive = Drive::new(&options, Context::background());
    assert_eq!(drive.deferred_permits(), 1);
    assert_eq!(drive.deferred_permits(), 1);
    let barrier = &Barrier::new(16);
    let consumed = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    usize::from(drive.consume_deferred_permit())
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .sum::<usize>()
    });
    assert_eq!(consumed, 1);
    assert_eq!(drive.deferred_permits(), 0);
    assert!(!drive.consume_deferred_permit());
    assert_eq!(options.poll_deferred, Some(true));
    assert!(!self::drive().consume_deferred_permit());
}

#[tokio::test]
async fn competing_cross_thread_completions_choose_one_result_for_every_waiter() {
    let drive = drive();
    let barrier = &Barrier::new(16);
    std::thread::scope(|scope| {
        for i in 0..16 {
            let drive = &drive;
            scope.spawn(move || {
                barrier.wait();
                if i % 2 == 0 {
                    drive.settle(waiting(i));
                } else {
                    drive.fail(error(&format!("failure {i}")));
                }
            });
        }
    });
    let first = drive.completion().wait().await;
    for _ in 0..32 {
        let next = drive.completion().wait().await;
        match (&first, next) {
            (Ok(a), Ok(b)) => assert!(Arc::ptr_eq(a, &b)),
            (Err(a), Err(b)) => assert!(Arc::ptr_eq(a, &b)),
            _ => panic!("first outcome changed"),
        }
    }
}
