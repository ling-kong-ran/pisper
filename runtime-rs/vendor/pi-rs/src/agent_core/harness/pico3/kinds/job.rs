//! Port of `packages/agent/src/harness/pico3/kinds/job.ts` (187 lines): the
//! `pi.job` kind — durable waiting, spawning, polling against a process
//! host, rerun-on-unknown, recurring occurrences, and abort with the
//! TERM/grace/KILL sequence.

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::{json, Map, Value};

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::session::{TaskRef, Tx};
use crate::agent_core::harness::pico3::types::{
    task_contract_fault, AnyKind, BasicKind, Completion, Id, NewEntry, Task, ViewEvent,
};

use crate::agent_core::harness::pico3::runtime::{AbortClosure, ProcessStatus, Runtime, Step};

/// Upstream `jobKind` (`job.ts:35-81`).
pub struct JobKind;

impl JobKind {
    pub fn new() -> Arc<Self> {
        Arc::new(JobKind)
    }
}

/// `task.input.notBefore` / `every` / `notify` / `rerun` accessors
/// (`job.ts:4-9`).
fn input_flag(task: &Task, key: &str) -> Option<bool> {
    task.input.get(key).and_then(Value::as_bool)
}

fn input_ms(task: &Task, key: &str) -> Option<i64> {
    task.input.get(key).and_then(Value::as_i64)
}

impl crate::agent_core::harness::pico3::runtime::Kind for JobKind {
    fn metadata(&self) -> Arc<dyn AnyKind> {
        Arc::new(
            BasicKind::new("pi.job")
                .slot(|_input| Map::new())
                .describe(|task, slot| {
                    // `describe: (task) => ({ stage: …, ...slot })`
                    // (`job.ts:47-50`).
                    let stage = task
                        .checkpoint
                        .as_ref()
                        .and_then(|checkpoint| checkpoint.get("phase"))
                        .and_then(Value::as_str)
                        .unwrap_or("starting");
                    let mut out = Map::new();
                    out.insert("stage".to_owned(), Value::String(stage.to_owned()));
                    if let Some(slot) = slot {
                        for (key, value) in slot {
                            out.insert(key.clone(), value.clone());
                        }
                    }
                    Ok(Value::Object(out))
                })
                .inflight(vec!["spawning".to_owned(), "running".to_owned()]),
        )
    }

    fn initial(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        async move {
            if rt.process_host.is_none() {
                return Ok(done_failed("spawn", "no process host"));
            }
            let now = rt.now();
            let until_ms = input_ms(&task, "notBefore").unwrap_or(now);
            if until_ms > now {
                return Ok(Step::Next(
                    crate::agent_core::harness::pico3::runtime::Next::Checkpoint(object_of(
                        json!({ "phase": "waiting", "untilMs": until_ms, "occurrence": 1 }),
                    )),
                ));
            }
            spawn(task, 1, rt, ctx).await
        }
        .boxed()
    }

    fn phases(&self) -> Vec<String> {
        vec![
            "waiting".to_owned(),
            "spawning".to_owned(),
            "running".to_owned(),
        ]
    }

    fn phase(
        &self,
        phase: &str,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        let phase = phase.to_owned();
        async move {
            match phase.as_str() {
                "waiting" => {
                    let checkpoint = checkpoint_of(&task);
                    let until_ms = checkpoint
                        .get("untilMs")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    rt.sleep(until_ms, ctx.clone()).await?;
                    let occurrence = checkpoint
                        .get("occurrence")
                        .and_then(Value::as_i64)
                        .unwrap_or(1);
                    spawn(task, occurrence, rt, ctx).await
                }
                "spawning" | "running" => {
                    let checkpoint = checkpoint_of(&task);
                    reconcile(task, checkpoint, rt, ctx).await
                }
                other => Err(task_contract_fault(
                    "pi.job",
                    &format!("no handler for phase {other}"),
                )),
            }
        }
        .boxed()
    }

    fn abort(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<AbortClosure>> {
        async move {
            let checkpoint = checkpoint_of(&task);
            let phase = checkpoint
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or("");
            let key = checkpoint.get("key").and_then(Value::as_str).unwrap_or("");
            // `if (checkpoint === undefined || checkpoint.phase ===
            // "waiting" || runtime.processHost === undefined)`
            // (`job.ts:72-74`).
            if checkpoint.is_null() || phase == "waiting" {
                return Ok(null_abort());
            }
            let Some(host) = rt.process_host.clone() else {
                return Ok(null_abort());
            };
            host.kill(key, "SIGTERM", ctx.clone()).await?;
            // `await runtime.sleep(runtime.now() + 5000, ctx)`
            // (`job.ts:76`).
            rt.sleep(rt.now() + 5000, ctx.clone()).await?;
            host.kill(key, "SIGKILL", ctx).await?;
            Ok(
                crate::agent_core::harness::pico3::runtime::abort_closure_from(
                    |_tx: &mut Tx, _current: Task, _ctx: Context| {
                        Box::pin(async move { Ok(json!({ "killed": true })) })
                    },
                ),
            )
        }
        .boxed()
    }
}

fn null_abort() -> AbortClosure {
    crate::agent_core::harness::pico3::runtime::abort_closure_from(
        |_tx: &mut Tx, _current: Task, _ctx: Context| {
            Box::pin(async move { Ok(json!({ "killed": false })) })
        },
    )
}

/// Upstream `spawn` (`job.ts:83-103`).
async fn spawn(
    task: Arc<Task>,
    occurrence: i64,
    rt: Arc<Runtime>,
    ctx: Context,
) -> anyhow::Result<Step> {
    let key = format!("{}:{}", task.id, occurrence);
    write_job_checkpoint(&rt, ctx.clone(), &key, "spawning", occurrence).await?;
    let host = rt.process_host.clone().expect("checked by callers");
    let input = task.input.clone();
    if let Err(error) = host.start(&key, &input, ctx.clone()).await {
        let aborted = ctx
            .abort_signal()
            .is_some_and(|signal| signal.is_cancelled());
        if aborted {
            return Err(error);
        }
        let notify = input_flag(&task, "notify").unwrap_or(false);
        return Ok(Step::Done(Box::new(
            move |tx: &mut Tx, current: Task, _ctx: Context| {
                let error = format!("{error}");
                async move {
                    if notify {
                        tx.write(
                            current.conversation_id,
                            NewEntry {
                                kind: "pi.notice".to_owned(),
                                model: Some(vec![json!({
                                    "role": "user",
                                    "content": format!("job {} failed to start: {error}", current.id),
                                    "timestamp": tx.session().now(),
                                })]),
                                ..Default::default()
                            },
                        )
                        .await?;
                    }
                    Ok(failed("spawn", &error))
                }.boxed()
            },
        )));
    }
    write_job_checkpoint(&rt, ctx.clone(), &key, "running", occurrence).await?;
    poll(task, key, occurrence, rt, ctx).await
}

async fn write_job_checkpoint(
    rt: &Arc<Runtime>,
    ctx: Context,
    key: &str,
    phase: &str,
    occurrence: i64,
) -> anyhow::Result<()> {
    let key = key.to_owned();
    let phase = phase.to_owned();
    rt.commit_erased_with(
        ctx,
        Box::new(move |tx: &mut Tx, _current: Task, _ctx: Context| {
            let key = key.clone();
            let phase = phase.clone();
            async move {
                tx.checkpoint(json!({
                    "phase": phase,
                    "key": key,
                    "occurrence": occurrence,
                }))?;
                Ok(Value::Null)
            }
            .boxed()
        }),
    )
    .await
    .map(|_| ())
}

/// Upstream `reconcile` (`job.ts:105-129`).
async fn reconcile(
    task: Arc<Task>,
    checkpoint: Value,
    rt: Arc<Runtime>,
    ctx: Context,
) -> anyhow::Result<Step> {
    let Some(host) = rt.process_host.clone() else {
        return Ok(done_failed("interrupted", "no process host after restart"));
    };
    let key = checkpoint
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let status = match host.status(&key, ctx.clone()).await {
        Ok(status) => status,
        Err(error) => {
            return Ok(done_failed(
                "interrupted",
                &format!("host status failed: {error}"),
            ));
        }
    };
    if status.stage() == "unknown" {
        if !input_flag(&task, "rerun").unwrap_or(false) {
            return Ok(done_failed("interrupted", "process outcome unknown"));
        }
        let occurrence = checkpoint
            .get("occurrence")
            .and_then(Value::as_i64)
            .unwrap_or(1);
        return spawn(task, occurrence, rt, ctx).await;
    }
    if checkpoint.get("phase").and_then(Value::as_str) == Some("spawning") {
        let occurrence = checkpoint
            .get("occurrence")
            .and_then(Value::as_i64)
            .unwrap_or(1);
        write_job_checkpoint(&rt, ctx.clone(), &key, "running", occurrence).await?;
    }
    poll(
        task,
        key,
        checkpoint
            .get("occurrence")
            .and_then(Value::as_i64)
            .unwrap_or(1),
        rt,
        ctx,
    )
    .await
}

/// Apply `f` to the job's live slot object (`tx.slot({ id, kind })` write
/// paths, `job.ts:144-151, 174-181`): the slot materializes as `{}` on
/// first touch.
fn update_job_slot(tx: &mut Tx, task_id: Id, f: impl FnOnce(&mut Value)) -> anyhow::Result<()> {
    // pi.job is intentionally not core turn machinery. Match upstream
    // tx.slot({ id: current.id, kind: jobKind }), never write the whole
    // sticky.tasks document through a core-only capability. The invoker
    // carries the stable metadata captured by this invocation's lease.
    let kind = tx.invoker().task_kind().cloned().ok_or_else(|| {
        task_contract_fault("pi.job", "job slot update requires a task invocation")
    })?;
    tx.slot_update(&TaskRef { id: task_id, kind }, f)
}

/// Upstream `poll` (`job.ts:131-187`).
async fn poll(
    task: Arc<Task>,
    key: String,
    occurrence: i64,
    rt: Arc<Runtime>,
    ctx: Context,
) -> anyhow::Result<Step> {
    let Some(host) = rt.process_host.clone() else {
        return Ok(done_failed("interrupted", "no process host after restart"));
    };
    for attempt in 0u32.. {
        let status: ProcessStatus = match host.status(&key, ctx.clone()).await {
            Ok(status) => status,
            Err(error) => {
                return Ok(done_failed(
                    "interrupted",
                    &format!("host status failed: {error}"),
                ));
            }
        };
        if status.stage() == "unknown" {
            if !input_flag(&task, "rerun").unwrap_or(false) {
                return Ok(done_failed("interrupted", "process outcome unknown"));
            }
            return Box::pin(spawn(task, occurrence, rt, ctx)).await;
        }
        // Persist the output snapshot (`job.ts:144-151`).
        {
            let snapshot = status.to_value();
            let task_id = task.id;
            rt.commit_erased_with(
                ctx.clone(),
                Box::new(move |tx: &mut Tx, _current: Task, _ctx: Context| {
                    let snapshot = snapshot.clone();
                    async move {
                        update_job_slot(tx, task_id, |slot| {
                            let object = slot.as_object_mut().expect("slot object");
                            for key in ["stdout", "stderr", "droppedStdout", "droppedStderr"] {
                                object.insert(
                                    key.to_owned(),
                                    snapshot.get(key).cloned().unwrap_or(Value::Null),
                                );
                            }
                            if snapshot.get("status").and_then(Value::as_str) == Some("exited") {
                                object.insert(
                                    "exitCode".to_owned(),
                                    snapshot.get("exitCode").cloned().unwrap_or(Value::Null),
                                );
                            }
                        })?;
                        Ok(Value::Null)
                    }
                    .boxed()
                }),
            )
            .await?;
        }
        if status.stage() == "exited" {
            let (exit_code, stdout, stderr) = match &status {
                ProcessStatus::Exited {
                    exit_code,
                    stdout,
                    stderr,
                    ..
                } => (*exit_code, stdout.clone(), stderr.clone()),
                _ => (0, String::new(), String::new()),
            };
            if input_ms(&task, "every").is_none() {
                let notify = input_flag(&task, "notify").unwrap_or(false);
                return Ok(Step::Done(Box::new(
                    move |tx: &mut Tx, current: Task, _ctx: Context| {
                        async move {
                            if notify {
                                tx.write(
                                    current.conversation_id,
                                    NewEntry {
                                        kind: "pi.notice".to_owned(),
                                        model: Some(vec![json!({
                                            "role": "user",
                                            "content": format!(
                                                "job {} exited with code {exit_code}",
                                                current.id
                                            ),
                                            "timestamp": tx.session().now(),
                                        })]),
                                        ..Default::default()
                                    },
                                )
                                .await?;
                            }
                            Ok(Completion::completed(json!({
                                "exitCode": exit_code,
                                "occurrences": occurrence,
                                "stdout": stdout,
                                "stderr": stderr,
                            })))
                        }
                        .boxed()
                    },
                )));
            }
            let every = input_ms(&task, "every").unwrap_or(0);
            let until_ms = rt.now() + every;
            let notify = input_flag(&task, "notify").unwrap_or(false);
            return Ok(Step::Next(
                crate::agent_core::harness::pico3::runtime::Next::Defer(Box::new(
                    move |tx: &mut Tx, current: Task, _ctx: Context| {
                        async move {
                        if notify {
                            tx.write(
                                current.conversation_id,
                                NewEntry {
                                    kind: "pi.notice".to_owned(),
                                    model: Some(vec![json!({
                                        "role": "user",
                                        "content": format!(
                                            "job {} occurrence {occurrence} exited with code {exit_code}",
                                            current.id
                                        ),
                                        "timestamp": tx.session().now(),
                                    })]),
                                    ..Default::default()
                                },
                            )
                            .await?;
                        }
                        // Reset the output slot (`job.ts:174-181`).
                        update_job_slot(tx, current.id, |slot| {
                            let object = slot.as_object_mut().expect("slot object");
                            object.insert("stdout".to_owned(), Value::String(String::new()));
                            object.insert("stderr".to_owned(), Value::String(String::new()));
                            object.insert("droppedStdout".to_owned(), json!(0));
                            object.insert("droppedStderr".to_owned(), json!(0));
                            object.shift_remove("exitCode");
                            object.insert("occurrence".to_owned(), json!(occurrence + 1));
                        })?;
                        Ok(crate::agent_core::harness::pico3::runtime::Next::Checkpoint(
                            object_of(json!({
                                "phase": "waiting",
                                "untilMs": until_ms,
                                "occurrence": occurrence + 1,
                            })),
                        ))
                    }.boxed()
                    },
                )),
            ));
        }
        // Backoff (`job.ts:185`).
        rt.sleep(
            rt.now() + i64::min(1000, 100 * 2u64.pow(attempt.min(4)) as i64),
            ctx.clone(),
        )
        .await?;
    }
    unreachable!("poll loops until a terminal step")
}

/// Upstream `failed(...)` (`job.ts:26-29`).
fn failed(reason: &str, detail: &str) -> Completion {
    Completion {
        status: "failed".to_owned(),
        result: None,
        failure: Some(json!({ "reason": reason, "detail": detail })),
    }
}

fn done_failed(reason: &str, detail: &str) -> Step {
    let completion = failed(reason, detail);
    Step::Done(Box::new(
        move |_tx: &mut Tx, _current: Task, _ctx: Context| {
            let completion = completion.clone();
            async move { Ok(completion) }.boxed()
        },
    ))
}

fn checkpoint_of(task: &Task) -> Value {
    task.checkpoint
        .as_ref()
        .map(|checkpoint| Value::Object(checkpoint.clone()))
        .unwrap_or(Value::Null)
}

fn object_of(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

// The view-event re-emit unused here; kept off the surface.
#[allow(unused_imports)]
use ViewEvent as _ViewEvent;
