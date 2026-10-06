//! Real-scheduler job oracles from recovery.test.ts and spec-scheduler-process.test.ts.
//! FakeHost is a process protocol double; these tests do not invoke child processes.
use super::support_process::FakeHost;
use super::support_runtime::*;
use crate::agent_core::harness::pico3::session::CreateTaskOptions;
use crate::agent_core::harness::pico3::types::{Id, Task, TaskStatus};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

async fn create_job(env: &Env, extra: Value) -> Id {
    let kind = env.h.builtin_kind("pi.job").unwrap();
    let mut input = json!({"command":"x","args":[],"cwd":"/","notify":false,"rerun":false});
    input
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    env.root
        .commit(
            move |tx, _| {
                Box::pin(async move {
                    Ok(tx
                        .create_task_kind(
                            &kind,
                            input,
                            CreateTaskOptions {
                                conversation_id: Some(1),
                                background: true,
                                after: vec![],
                            },
                        )?
                        .id)
                })
            },
            ctx(),
        )
        .await
        .unwrap()
}
async fn terminal(env: &Env, id: Id) -> Task {
    tokio::time::timeout(Duration::from_secs(8), env.h.wait_for_task(id, ctx()))
        .await
        .expect("job must settle")
        .unwrap()
}
fn key_of(task: &Task) -> String {
    task.checkpoint.as_ref().unwrap()["key"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn yields_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..20_000 {
        if condition() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("scheduler did not make progress");
}

#[tokio::test]
async fn spawn_failure_retains_declared_failure_and_writes_idle_notice_without_inbox_work() {
    let host = Arc::new(FakeHost {
        start_error: Some("spawn denied"),
        ..Default::default()
    });
    let env = open(OpenOptions {
        process_host: Some(host.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let id = create_job(&env, json!({"notify":true})).await;
    let done = terminal(&env, id).await;
    assert_eq!(
        failure_of(&done),
        Some(json!({"reason":"spawn","detail":"spawn denied"}))
    );
    assert_eq!(host.start_calls(), 1);
    assert_eq!(env.root.sticky(ctx()).await.unwrap()["inbox"], json!([]));
    let entries = env.entries(1).await.unwrap();
    assert_eq!(kinds(&entries), "notice");
    assert!(content_of(&entries[0]).contains("failed to start"));
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn status_failure_is_interrupted_without_rerunning_even_when_rerun_is_true() {
    let host = Arc::new(FakeHost {
        status_error: Some("host unavailable"),
        ..Default::default()
    });
    let env = open(OpenOptions {
        process_host: Some(host.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let id = create_job(&env, json!({"rerun":true})).await;
    let done = terminal(&env, id).await;
    assert_eq!(host.start_calls(), 1);
    assert_eq!(
        failure_of(&done),
        Some(json!({"reason":"interrupted","detail":"host status failed: host unavailable"}))
    );
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn spawning_and_running_reopen_with_unknown_rerun_preserve_key_and_notify_once() {
    for phase in ["spawning", "running"] {
        let dir = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let host = Arc::new(FakeHost {
            start_gate: if phase == "spawning" {
                Some(gate.clone())
            } else {
                None
            },
            ..Default::default()
        });
        let env = open(OpenOptions {
            dir: Some(dir.path().to_owned()),
            process_host: Some(host.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
        let id = create_job(&env, json!({"notify":true,"rerun":true})).await;
        let key = key_of(&until_phase(&env, "pi.job", Some(phase)).await.unwrap());
        if phase == "spawning" {
            tokio::time::timeout(Duration::from_secs(8), gate.arrivals(1))
                .await
                .unwrap();
        }
        assert_eq!(host.start_calls(), 1);
        env.close(ctx()).await.unwrap();
        assert!(
            host.kills.lock().unwrap().is_empty(),
            "suspend must not kill persisted jobs"
        );
        let host2 = Arc::new(FakeHost::default());
        let env = open(OpenOptions {
            dir: Some(dir.path().to_owned()),
            process_host: Some(host2.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
        let running = until_phase(&env, "pi.job", Some("running")).await.unwrap();
        // For an already-running record the checkpoint can predate dispatch.
        yields_until(|| host2.start_calls() == 1).await;
        assert_eq!(key_of(&running), key);
        assert_eq!(host2.keys(), vec![key.clone()]);
        host2.exit(&key, 0);
        let done = terminal(&env, id).await;
        assert_eq!(outcome_status(&done), "completed");
        assert_eq!(
            result_of(&done),
            Some(json!({"exitCode":0,"occurrences":1,"stdout":format!("out {key}"),"stderr":""}))
        );
        assert_eq!(kinds(&env.entries(1).await.unwrap()), "notice");
        env.close(ctx()).await.unwrap();
    }
}

#[tokio::test]
async fn unknown_after_reopen_without_rerun_fails_without_starting_a_process() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::default());
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        process_host: Some(host),
        ..Default::default()
    })
    .await
    .unwrap();
    let id = create_job(&env, json!({})).await;
    until_phase(&env, "pi.job", Some("running")).await.unwrap();
    env.close(ctx()).await.unwrap();
    let host = Arc::new(FakeHost::default());
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        process_host: Some(host.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(
        failure_of(&terminal(&env, id).await),
        Some(json!({"reason":"interrupted","detail":"process outcome unknown"}))
    );
    assert_eq!(host.start_calls(), 0);
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn spawning_reopen_reconciles_existing_exited_process_without_duplicate_start() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Gate::new();
    let host = Arc::new(FakeHost {
        start_gate: Some(gate.clone()),
        ..Default::default()
    });
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        process_host: Some(host.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let id = create_job(&env, json!({"rerun":true})).await;
    let key = key_of(&until_phase(&env, "pi.job", Some("spawning")).await.unwrap());
    tokio::time::timeout(Duration::from_secs(8), gate.arrivals(1))
        .await
        .unwrap();
    env.close(ctx()).await.unwrap();
    host.exit(&key, 7);
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        process_host: Some(host.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let done = terminal(&env, id).await;
    assert_eq!(outcome_status(&done), "completed", "{done:?}");
    assert_eq!(
        result_of(&done),
        Some(json!({"exitCode":7,"occurrences":1,"stdout":format!("out {key}"),"stderr":""}))
    );
    assert_eq!(host.start_calls(), 1);
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn no_host_is_spawn_failure_initially_but_interrupted_after_running_reopen() {
    let env = open(OpenOptions::default()).await.unwrap();
    let id = create_job(&env, json!({})).await;
    assert_eq!(
        failure_of(&terminal(&env, id).await),
        Some(json!({"reason":"spawn","detail":"no process host"}))
    );
    env.close(ctx()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        process_host: Some(Arc::new(FakeHost::default())),
        ..Default::default()
    })
    .await
    .unwrap();
    let id = create_job(&env, json!({"rerun":true})).await;
    until_phase(&env, "pi.job", Some("running")).await.unwrap();
    env.close(ctx()).await.unwrap();
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(
        failure_of(&terminal(&env, id).await),
        Some(json!({"reason":"interrupted","detail":"no process host after restart"}))
    );
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn abort_sends_term_then_waits_five_seconds_before_kill_and_terminalization() {
    let host = Arc::new(FakeHost::default());
    let env = open(OpenOptions {
        process_host: Some(host.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let id = create_job(&env, json!({})).await;
    until_phase(&env, "pi.job", Some("running")).await.unwrap();
    tokio::time::pause();
    env.h.abort_task(id, ctx()).await.unwrap();
    yields_until(|| host.kills.lock().unwrap().len() == 1).await;
    assert_eq!(host.kills.lock().unwrap()[0].1, "SIGTERM");
    // Harness sleep uses a wall-clock absolute deadline; tolerate only the
    // sampling/timer-wheel granularity by advancing 4990 + 11 virtual milliseconds.
    tokio::time::advance(Duration::from_millis(4990)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(host.kills.lock().unwrap().len(), 1);
    assert_ne!(
        env.h.get_task(id, ctx()).await.unwrap().unwrap().status,
        TaskStatus::Terminal
    );
    tokio::time::advance(Duration::from_millis(11)).await;
    yields_until(|| host.kills.lock().unwrap().len() == 2).await;
    let kills = host.kills.lock().unwrap().clone();
    assert_eq!(kills[1].1, "SIGKILL");
    assert_eq!(kills[1].2 - kills[0].2, Duration::from_millis(5001));
    let done = terminal(&env, id).await;
    assert_eq!(outcome_status(&done), "aborted");
    assert_eq!(result_of(&done), Some(json!({"killed":true})));
    tokio::time::resume();
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn waiting_job_is_not_quiescent_and_abort_before_start_never_signals_the_host() {
    let host = Arc::new(FakeHost::default());
    let env = open(OpenOptions {
        process_host: Some(host.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(env.h.quiescent());
    let id = create_job(&env, json!({"notBefore":crate::ai::now_ms()+60_000})).await;
    until_phase(&env, "pi.job", Some("waiting")).await.unwrap();
    assert!(!env.h.quiescent());
    env.h.abort_task(id, ctx()).await.unwrap();
    let done = terminal(&env, id).await;
    assert_eq!(outcome_status(&done), "aborted");
    assert_eq!(result_of(&done), Some(json!({"killed":false})));
    assert_eq!(host.start_calls(), 0);
    assert!(host.kills.lock().unwrap().is_empty());
    assert!(env.h.quiescent());
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn recurring_job_resets_its_own_slot_and_reopen_preserves_next_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::default());
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        process_host: Some(host.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let id = create_job(&env, json!({"every":60_000,"notify":true})).await;
    let key = key_of(&until_phase(&env, "pi.job", Some("running")).await.unwrap());
    host.exit(&key, 3);
    let waiting = until_phase(&env, "pi.job", Some("waiting")).await.unwrap();
    assert_eq!(waiting.checkpoint.as_ref().unwrap()["occurrence"], json!(2));
    let slot = env.root.sticky(ctx()).await.unwrap()["tasks"][id.to_string()].clone();
    assert_eq!(
        slot,
        json!({"stdout":"","stderr":"","droppedStdout":0,"droppedStderr":0,"occurrence":2})
    );
    let entries = env.entries(1).await.unwrap();
    assert_eq!(kinds(&entries), "notice");
    assert!(content_of(&entries[0]).contains("occurrence 1 exited with code 3"));
    env.close(ctx()).await.unwrap();
    let env = open(OpenOptions {
        dir: Some(dir.path().to_owned()),
        process_host: Some(host.clone()),
        paused: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let restored = env.h.get_task(id, ctx()).await.unwrap().unwrap();
    assert_eq!(restored.checkpoint, waiting.checkpoint);
    assert_eq!(
        env.root.sticky(ctx()).await.unwrap()["tasks"][id.to_string()],
        slot
    );
    env.h.resume();
    env.h.abort_task(id, ctx()).await.unwrap();
    let done = terminal(&env, id).await;
    assert_eq!(result_of(&done), Some(json!({"killed":false})));
    assert_eq!(host.start_calls(), 1);
    assert!(host.kills.lock().unwrap().is_empty());
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn unknown_during_live_poll_reruns_the_same_occurrence_key() {
    let host = Arc::new(FakeHost::default());
    let env = open(OpenOptions {
        process_host: Some(host.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let id = create_job(&env, json!({"rerun":true})).await;
    let key = key_of(&until_phase(&env, "pi.job", Some("running")).await.unwrap());
    host.forget(&key);
    tokio::time::timeout(Duration::from_secs(8), async {
        while host.start_calls() < 2 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(host.keys(), vec![key.clone(), key.clone()]);
    host.exit(&key, 0);
    let done = terminal(&env, id).await;
    assert_eq!(outcome_status(&done), "completed");
    assert_eq!(result_of(&done).unwrap()["occurrences"], json!(1));
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn job_exit_notification_is_queued_while_generation_is_busy_then_drained_once() {
    use crate::agent_core::harness::pico3::types::{SendInput, UserInput};
    let gate = Gate::new();
    let host = Arc::new(FakeHost::default());
    let env = open(OpenOptions {
        process_host: Some(host.clone()),
        models: Some(FakeModels::new(Arc::new(echo_script)).with_gate(gate.clone())),
        ..Default::default()
    })
    .await
    .unwrap();
    let input = env
        .root
        .send(
            SendInput {
                content: UserInput::Text("busy".to_owned()),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), gate.arrivals(1))
        .await
        .unwrap();
    let id = create_job(&env, json!({"notify":true})).await;
    let key = key_of(&until_phase(&env, "pi.job", Some("running")).await.unwrap());
    host.exit(&key, 0);
    assert_eq!(outcome_status(&terminal(&env, id).await), "completed");
    assert!(!env
        .entries(1)
        .await
        .unwrap()
        .iter()
        .any(|e| e.kind == "pi.notice"));
    assert_eq!(
        env.root.sticky(ctx()).await.unwrap()["inbox"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    gate.open();
    tokio::time::timeout(Duration::from_secs(8), input.wait(ctx()))
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), env.root.wait_for_idle(ctx()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        kinds(&env.entries(1).await.unwrap()),
        "user system assistant notice"
    );
    assert_eq!(env.root.sticky(ctx()).await.unwrap()["inbox"], json!([]));
    env.close(ctx()).await.unwrap();
}
