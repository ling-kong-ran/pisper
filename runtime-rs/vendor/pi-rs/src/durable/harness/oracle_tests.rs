//! Byte-oracle tests against `tests/fixtures/durable_oracle/durable_oracle.json`
//! for the harness slice scenarios (`harness_docs`, `prompt_plan`,
//! `output_bound`), captured by `capture_durable_oracle.mjs` from the
//! read-only upstream sources. Serialized wire strings must match the Node
//! capture byte-for-byte (through `preserve_order` maps on both sides).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::ai::types::{Message, StringOrBlocks, SystemMessage, Usage};

use crate::agent_core::chord_support::context::Context;

use super::config::{conversation_config, ConversationConfigState};
use super::inbox::{apply_boundary, inbox_doc, prepare_boundary, At};
use super::live::{live_doc, SlotStatus};
use super::output::{bound_output, sanitize_output, OutputLimits};
use super::prompt::{plan_system_entries, replay_sections};
use super::types::{ContextView, OutputRetain};
use super::usage::{usage_doc, UsageState};
use crate::durable::session::session::create_session;
use crate::durable::session::transaction::Transaction;
use crate::durable::storage::memory::MemoryStorage;
use crate::durable::storage::Storage;
use crate::durable::truncate::{format_size, truncate_head, TruncatedBy, TruncationOptions};
use crate::durable::types::{EntryDraft, TaskOwnership};

fn oracle() -> Value {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir.join("tests/fixtures/durable_oracle/durable_oracle.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Canonical wire string of a fixture value (order-preserving on both sides).
fn wire(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

fn harness_docs() -> Value {
    oracle()["harness_docs"].clone()
}

/// The `Usage` values of scenario 4, in capture order.
fn scenario4_usages() -> (Usage, Usage) {
    let first: Usage = serde_json::from_value(serde_json::json!({
        "input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 2, "totalTokens": 15,
        "cost": {"input": 0.5, "output": 0.25, "cacheRead": 0, "cacheWrite": 0, "total": 0.75},
    }))
    .unwrap();
    let second: Usage = serde_json::from_value(serde_json::json!({
        "input": 7, "output": 3, "cacheRead": 1, "cacheWrite": 0, "cacheWrite1h": 4, "reasoning": 2,
        "totalTokens": 10,
        "cost": {"input": 0.25, "output": 0.25, "cacheRead": 0.1, "cacheWrite": 0, "total": 0.6},
    }))
    .unwrap();
    (first, second)
}

#[test]
fn oracle_config_document_surface() {
    let expected = harness_docs();
    // Initial state: `{ thinkingLevel: "off", activeTools: [] }`.
    let initial = ConversationConfigState::initial().into_json().unwrap();
    assert_eq!(
        wire(&Value::Object(initial)),
        wire(&expected["configInitial"]),
        "config initial wire bytes"
    );
    // The doc definition surface.
    let definition = &conversation_config().definition;
    let surface = serde_json::json!({
        "kind": definition.kind,
        "version": definition.version,
        "scope": match definition.scope {
            super::super::documents::DefinitionScope::Conversation => "conversation",
            _ => "other",
        },
        "history": match definition.history.unwrap() {
            super::super::types::DocumentHistory::Rewindable => "rewindable",
            super::super::types::DocumentHistory::Latest => "latest",
        },
        "fork": match definition.fork.unwrap() {
            super::super::types::DocumentFork::AsOf => "asOf",
            super::super::types::DocumentFork::Current => "current",
            super::super::types::DocumentFork::Initial => "initial",
        },
    });
    assert_eq!(
        wire(&surface),
        wire(&expected["configDocSurface"]),
        "config doc surface"
    );
}

#[test]
fn oracle_usage_initial_and_totals() {
    let expected = harness_docs();
    // Initial: `{ models: {}, tools: {} }`.
    assert_eq!(
        wire(&Value::Object(super::usage::initial_json())),
        wire(&expected["usageInitial"])
    );

    // Totals: strict JSON first entry, then `addUsage` for the second.
    let (first, second) = scenario4_usages();
    let mut total = serde_json::to_value(first)
        .unwrap()
        .as_object()
        .cloned()
        .unwrap();
    super::usage::add_usage(&mut total, &second);
    let expected_total = expected["usageTotal"]["gpt-x/gpt-5"].clone();
    assert_eq!(
        wire(&Value::Object(total)),
        wire(&expected_total),
        "usage totals wire bytes"
    );
}

#[test]
fn oracle_live_state_surface() {
    let expected = harness_docs();
    let live_initial = (live_doc().definition.initial)(None);
    assert_eq!(
        wire(&Value::Object(live_initial)),
        wire(&expected["liveInitial"])
    );
    // `checkpointWhen`: a running tool slot forces a base whenever nothing
    // else does; done slots do not.
    let running = serde_json::json!({
        "generation": null,
        "tools": [{"callId": "a", "name": "bash", "status": "running"}],
    });
    let idle = serde_json::json!({
        "generation": null,
        "tools": [{"callId": "a", "name": "bash", "status": "done"}],
    });
    let checkpoint = {
        let live = live_doc();
        live.definition.checkpoint_when.clone().unwrap()
    };
    assert_eq!(
        checkpoint(
            running.as_object().unwrap(),
            &[],
            super::super::types::CheckpointInfo {
                deltas_since_base: 0
            }
        ),
        expected["liveCheckpointRunningIsNotBase"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(
        checkpoint(
            idle.as_object().unwrap(),
            &[],
            super::super::types::CheckpointInfo {
                deltas_since_base: 0
            }
        ),
        expected["liveCheckpointIdleIsBase"].as_bool().unwrap()
    );
    let _ = SlotStatus::Pending;
}
/// Scenario 4's `harness_docs` session flow: root conversation, then the
/// boundary commit with three submissions (write, steer, follow-up) and the
/// usage/config documents.
#[tokio::test]
async fn oracle_harness_boundary_documents() {
    let expected = harness_docs();
    let storage = Arc::new(MemoryStorage::new());
    let session = create_session(Arc::clone(&storage) as Arc<dyn Storage>);
    session
        .commit(
            |tx: Arc<Transaction>| {
                Box::pin(async move { tx.create_root_conversation().map(|record| record.id) })
            },
            Context::background(),
        )
        .await
        .unwrap();

    let (first, second) = scenario4_usages();
    let outcome = session
        .commit(
            move |tx: Arc<Transaction>| {
                let first = first;
                let second = second;
                Box::pin(async move {
                    let mut boundary = prepare_boundary(&tx, 1)?;
                    let write =
                        tx.create_submission(crate::durable::types::SubmissionRecord::queued(
                            1,
                            None,
                            crate::durable::types::SubmissionType::Write,
                            0,
                        ))?;
                    let steer =
                        tx.create_submission(crate::durable::types::SubmissionRecord::queued(
                            1,
                            None,
                            crate::durable::types::SubmissionType::Input,
                            0,
                        ))?;
                    let follow_up =
                        tx.create_submission(crate::durable::types::SubmissionRecord::queued(
                            1,
                            None,
                            crate::durable::types::SubmissionType::Input,
                            0,
                        ))?;
                    let inbox = inbox_doc();
                    let inbox_draft = tx.doc(&inbox.definition, Some(1), None, None)?;
                    let items = [
                        super::inbox::InboxItem::Write {
                            id: write.id,
                            entry: serde_json::json!({"kind": "note", "data": {"text": "w"}})
                                .as_object()
                                .cloned()
                                .unwrap(),
                        },
                        super::inbox::InboxItem::Input {
                            id: steer.id,
                            mode: super::inbox::InputMode::Steer,
                            content: Value::from("s"),
                        },
                        super::inbox::InboxItem::Input {
                            id: follow_up.id,
                            mode: super::inbox::InputMode::FollowUp,
                            content: Value::from("f"),
                        },
                    ];
                    inbox_draft
                        .set(
                            &[String::from("items").into()],
                            Value::Array(items.iter().map(|item| item.to_json()).collect()),
                        )
                        .map_err(|error| {
                            crate::durable::errors::PlainError::new(error.message().to_string())
                        })?;
                    let result =
                        apply_boundary(&tx, &mut boundary, At::Final, 1_758_240_001_000.0)?;

                    // `pi.usage`: strict JSON first entry, then the totals.
                    let usage = usage_doc();
                    let usage_draft = tx.doc(&usage.definition, Some(1), None, None)?;
                    let mut models = serde_json::Map::new();
                    models.insert(String::from("gpt-x/gpt-5"), {
                        let mut total = serde_json::to_value(first)
                            .unwrap()
                            .as_object()
                            .cloned()
                            .unwrap();
                        super::usage::add_usage(&mut total, &second);
                        Value::Object(total)
                    });
                    // `Object.assign(usageDraft.models, usageTotal)` — a
                    // bucket-level set, like the capture.
                    usage_draft
                        .set(&[String::from("models").into()], Value::Object(models))
                        .map_err(|error| {
                            crate::durable::errors::PlainError::new(error.message().to_string())
                        })?;

                    // `pi.inbox` and `pi.conversation.config` post-boundary.
                    let config = conversation_config();
                    let config_draft = tx.doc(&config.definition, Some(1), None, None)?;
                    // `configDraft.activeTools = [...]; configDraft.steeringMode
                    // = "all"` — property sets, like the capture.
                    config_draft
                        .set(
                            &[String::from("activeTools").into()],
                            serde_json::json!(["bash", "read"]),
                        )
                        .map_err(|error| {
                            crate::durable::errors::PlainError::new(error.message().to_string())
                        })?;
                    config_draft
                        .set(&[String::from("steeringMode").into()], Value::from("all"))
                        .map_err(|error| {
                            crate::durable::errors::PlainError::new(error.message().to_string())
                        })?;
                    let inbox_after = inbox_draft
                        .read(&[])
                        .map_err(|error: crate::chord::delta::TrackerError| {
                            crate::durable::errors::PlainError::new(error.message().to_string())
                        })?
                        .unwrap_or(Value::Null);
                    let usage_after = usage_draft
                        .read(&[])
                        .map_err(|error: crate::chord::delta::TrackerError| {
                            crate::durable::errors::PlainError::new(error.message().to_string())
                        })?
                        .unwrap_or(Value::Null);
                    let config_after = config_draft
                        .read(&[])
                        .map_err(|error: crate::chord::delta::TrackerError| {
                            crate::durable::errors::PlainError::new(error.message().to_string())
                        })?
                        .unwrap_or(Value::Null);
                    Ok::<_, crate::durable::errors::PlainError>((
                        result,
                        write.id,
                        steer.id,
                        follow_up.id,
                        inbox_after,
                        usage_after,
                        config_after,
                    ))
                })
            },
            Context::background(),
        )
        .await
        .unwrap();

    let (result, write_id, steer_id, follow_up_id, inbox_after, usage_after, config_after) =
        outcome;
    // Boundary result.
    let expected_result = serde_json::json!({
        "users": expected["boundaryResult"]["users"].as_array().unwrap().iter().map(|id| id.as_i64().unwrap()).collect::<Vec<_>>(),
        "reset": expected["boundaryResult"]["reset"].as_bool().unwrap(),
    });
    assert_eq!(
        serde_json::json!({"users": result.users, "reset": result.reset}),
        expected_result,
        "boundary result"
    );

    // Inbox items after the boundary: empty (the capture reads `.items`).
    assert_eq!(
        wire(&inbox_after.get("items").cloned().unwrap_or(Value::Null)),
        wire(&expected["inboxItemsAfter"])
    );

    // Usage doc.
    assert_eq!(wire(&usage_after), wire(&expected["usageDocAfter"]));

    // Config doc.
    assert_eq!(wire(&config_after), wire(&expected["configDocAfter"]));

    // The submission records, in creation order (write, steer, follow-up).
    let submissions = expected["submissions"].as_array().unwrap();
    assert_eq!(submissions.len(), 3);
    let context = Context::background();
    for (index, id) in [write_id, steer_id, follow_up_id].into_iter().enumerate() {
        let record = storage
            .submission(id, &context)
            .unwrap()
            .expect("submission exists");
        assert_eq!(
            wire(&serde_json::to_value(&record).unwrap()),
            wire(&submissions[index]),
            "submission {index} wire bytes"
        );
    }
    let _ = follow_up_id;
    let _ = TaskOwnership::Conversation;
    let _ = EntryDraft::new("unused");
    let _ = UsageState::initial();
}

#[test]
fn oracle_prompt_plan() {
    let expected = oracle()["prompt_plan"].clone();

    // Replay of two system messages: set in place, `null` deletes, re-add.
    let make_system = |sections: Value, timestamp: i64| {
        Message::System(SystemMessage {
            content: StringOrBlocks::Text(String::new()),
            sections: Some(serde_json::from_value(sections).unwrap()),
            tools_added: None,
            tools_removed: None,
            timestamp,
        })
    };
    let replayed = replay_sections(&[
        make_system(
            serde_json::json!({"tools": "old", "skills": null, "extra": "keep"}),
            1,
        ),
        make_system(serde_json::json!({"tools": "new"}), 2),
    ]);
    let replayed_map: serde_json::Map<String, Value> = replayed
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    assert_eq!(
        wire(&Value::Object(replayed_map)),
        wire(&expected["replayed"])
    );
    let order: Vec<String> = replayed.iter().map(|(key, _)| key.clone()).collect();
    assert_eq!(
        wire(&Value::Array(
            order.into_iter().map(Value::String).collect()
        )),
        wire(&expected["replayedOrder"])
    );

    // Baseline plan: head marker, no later pi.system entry.
    let system_message = |content: &str,
                          sections: Value,
                          tools_added: Value,
                          tools_removed: Value,
                          timestamp: i64| {
        Message::System(SystemMessage {
            content: StringOrBlocks::Text(content.to_string()),
            sections: if sections.is_null() {
                None
            } else {
                Some(serde_json::from_value(sections).unwrap())
            },
            tools_added: if tools_added.is_null() {
                None
            } else {
                Some(serde_json::from_value(tools_added).unwrap())
            },
            tools_removed: if tools_removed.is_null() {
                None
            } else {
                Some(serde_json::from_value(tools_removed).unwrap())
            },
            timestamp,
        })
    };
    let head_entry: crate::durable::types::EntryRecord =
        serde_json::from_value(serde_json::json!({
            "kind": "pi.system", "id": 7, "conversationId": 1, "head": 7
        }))
        .unwrap();
    let view = ContextView {
        head: Some(head_entry.clone()),
        entries: vec![head_entry],
        contributions: Vec::new(),
        messages: Vec::new(),
    };
    let desired = vec![
        (String::from("tools"), String::from("Use tools carefully.")),
        (String::from("safety"), String::from("Be safe")),
    ];
    let tools: Vec<crate::ai::types::Tool> = serde_json::from_value(serde_json::json!([
        {"name": "bash", "description": "Run a shell command", "parameters": {"type": "object", "properties": {"command": {"type": "string"}}}}
    ]))
    .unwrap();
    let planned = plan_system_entries(&view, &desired, &tools, 1_758_240_002_000.0);
    assert_eq!(planned.len(), 1);
    assert_eq!(
        wire(&serde_json::to_value(planned[0].model.as_ref().unwrap()[0].clone()).unwrap()),
        wire(&expected["plannedBaseline"][0]["model"][0]),
        "planned baseline message wire bytes"
    );

    // Patch plan: minimal section patch over a replayed context.
    let patch_view = ContextView {
        head: None,
        entries: Vec::new(),
        contributions: Vec::new(),
        messages: vec![system_message(
            "",
            serde_json::json!({"tools": "Use tools carefully.", "skills": null}),
            Value::Null,
            Value::Null,
            3,
        )],
    };
    let planned_patch = plan_system_entries(
        &patch_view,
        &[(
            String::from("tools"),
            String::from("Use tools carefully v2"),
        )],
        &[],
        1_758_240_003_000.0,
    );
    assert_eq!(planned_patch.len(), 1);
    assert_eq!(
        wire(&serde_json::to_value(planned_patch[0].model.as_ref().unwrap()[0].clone()).unwrap()),
        wire(&expected["plannedPatch"][0]["model"][0]),
        "planned patch message wire bytes"
    );
}

#[test]
fn oracle_output_bound() {
    let expected = oracle()["output_bound"].clone();
    let long_text = "one\ntwo\nthree\nfour\nfive\n";

    let case =
        |name: &str, text: &str, max_bytes: usize, max_lines: usize, retain: OutputRetain| {
            let slice = bound_output(
                text,
                &OutputLimits {
                    max_bytes,
                    max_lines,
                    retain,
                },
            );
            let expected_value = &expected[name];
            assert_eq!(
                slice.text,
                expected_value["text"].as_str().unwrap(),
                "{name} text"
            );
            assert_eq!(
                slice.bytes,
                expected_value["bytes"].as_u64().unwrap() as usize,
                "{name} bytes"
            );
            assert_eq!(
                slice.dropped_bytes,
                expected_value["droppedBytes"].as_u64().unwrap() as usize,
                "{name} droppedBytes"
            );
            assert_eq!(
                slice.dropped_lines,
                expected_value["droppedLines"].as_u64().unwrap() as usize,
                "{name} droppedLines"
            );
        };
    case("headLines", long_text, 1024, 2, OutputRetain::Head);
    case("tailLines", long_text, 1024, 2, OutputRetain::Tail);
    case("headBytesCut", long_text, 8, 100, OutputRetain::Head);
    case("tailBytesCut", long_text, 8, 100, OutputRetain::Tail);
    case("unicodeCut", "é\n한\n", 3, 10, OutputRetain::Head);

    assert_eq!(
        sanitize_output("a\u{0000}b\u{0007}c\td\ne\u{FFFA}f"),
        expected["sanitized"].as_str().unwrap()
    );

    // Truncation results.
    let head = truncate_head(
        "a\nb\nc\nd\n",
        TruncationOptions {
            max_lines: Some(2),
            max_bytes: Some(100),
        },
    );
    let expected_head = &expected["truncateHead"];
    assert_eq!(head.content, expected_head["content"].as_str().unwrap());
    assert_eq!(
        head.truncated_by == Some(TruncatedBy::Lines),
        expected_head["truncatedBy"] == "lines"
    );
    assert_eq!(
        head.total_lines,
        expected_head["totalLines"].as_u64().unwrap() as usize
    );
    assert_eq!(
        head.output_lines,
        expected_head["outputLines"].as_u64().unwrap() as usize
    );

    let head_bytes = truncate_head(
        "abc\ndef\nghi",
        TruncationOptions {
            max_lines: Some(100),
            max_bytes: Some(7),
        },
    );
    let expected_bytes = &expected["truncateHeadBytes"];
    assert_eq!(
        head_bytes.content,
        expected_bytes["content"].as_str().unwrap()
    );
    assert_eq!(
        head_bytes.first_line_exceeds_limit,
        expected_bytes["firstLineExceedsLimit"].as_bool().unwrap()
    );
    assert_eq!(
        head_bytes.truncated_by == Some(TruncatedBy::Bytes),
        expected_bytes["truncatedBy"] == "bytes"
    );

    assert_eq!(
        crate::durable::truncate::utf8_byte_length("héllo"),
        expected["utf8ByteLength"].as_u64().unwrap() as usize
    );
    let sizes: Vec<String> = expected["formatSize"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        vec![
            format_size(512),
            format_size(2048),
            format_size(3 * 1024 * 1024),
        ],
        sizes
    );
}

// Scenario 7/8 oracle tests: scheduler decision grids and the submissions
// state machine, captured from the upstream TS
// (`tests/fixtures/durable_oracle/capture_durable_oracle.mjs` scenarios 7-8)
// and asserted byte-for-byte on the shared fixture.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::scheduler::{record_checkpoint, TaskScheduler, TaskSchedulerOptions};
use super::submissions::Submissions;
use super::types::{
    ConversationSetupEntry, HookRegistration, PromptSection, RegistryFailure, RegistryReaderLike,
    RegistrySnapshotLike, SchedulingState, SubmissionDraft, TaskInspectionState, ToolRegistration,
    WhenBusy,
};
use crate::durable::errors::PlainError;
use crate::durable::tasks::{
    define_task, NextTaskState, PhaseArgs, PhaseFn, PlainFailure, TaskDefinition, TaskToken,
};

// ─── Scenario 7: scheduler decision grids ───────────────────────────────────

fn oracle_grid() -> serde_json::Value {
    oracle()["scheduler_grids"].clone()
}

/// The probe/oldish definition registry of scenario 7: a mutable task map
/// behind the reader surface, so definitions can be swapped between commits
/// exactly like the capture's `probes` overlay.
struct GridRegistry {
    tasks: Mutex<BTreeMap<String, TaskToken>>,
}

#[derive(Clone)]
struct GridSnapshot {
    tasks: BTreeMap<String, TaskToken>,
}

impl RegistrySnapshotLike for GridSnapshot {
    fn tools(&self) -> Vec<Arc<ToolRegistration>> {
        Vec::new()
    }

    fn tool(&self, _name: &str) -> Option<Arc<ToolRegistration>> {
        None
    }

    fn tool_names(&self) -> Vec<String> {
        Vec::new()
    }

    fn task(&self, name: &str) -> Option<TaskToken> {
        self.tasks.get(name).cloned()
    }

    fn hooks(&self, _task_name: &str) -> Vec<HookRegistration> {
        Vec::new()
    }

    fn sections(&self) -> Vec<PromptSection> {
        Vec::new()
    }

    fn failures(&self) -> Vec<RegistryFailure> {
        Vec::new()
    }

    fn conversation_setups(&self) -> Vec<ConversationSetupEntry> {
        Vec::new()
    }
}

impl RegistryReaderLike for GridRegistry {
    fn snapshot(&self) -> Arc<dyn RegistrySnapshotLike> {
        Arc::new(GridSnapshot {
            tasks: self
                .tasks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        })
    }

    fn subscribe(&self, _listener: Box<dyn Fn() + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        Box::new(|| {})
    }
}

fn grid_task(name: &str, version: i64, phases: &str) -> TaskToken {
    let mut map = BTreeMap::new();
    if phases == "probe-run" {
        map.insert(String::from("run"), Arc::new(probe_run_phase) as PhaseFn);
    }
    define_task(TaskDefinition {
        name: String::from(name),
        version,
        initial: Arc::new(|| {
            serde_json::json!({"phase": "run", "steps": 0})
                .as_object()
                .cloned()
                .unwrap()
        }),
        phases: map,
        abort: Some(Arc::new(|_args: PhaseArgs| Box::pin(async { Ok(()) }))),
        migrate: None,
    })
}

fn probe_run_phase(
    args: PhaseArgs,
) -> Pin<Box<dyn Future<Output = Result<(), PlainFailure>> + Send>> {
    let runtime = Arc::clone(&args.runtime);
    let context = args.context.clone();
    let steps = record_checkpoint(&args.record)
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default()
        .get("steps")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    Box::pin(async move {
        runtime
            .commit(
                Box::new(move |_tx, _current| {
                    if steps + 1 >= 2 {
                        Ok(Some(NextTaskState::Terminal {
                            outcome: crate::durable::types::TaskOutcome::Completed {
                                result: serde_json::json!({ "steps": steps + 1 }),
                            },
                        }))
                    } else {
                        Ok(Some(NextTaskState::Running {
                            checkpoint: serde_json::json!({"phase": "run", "steps": steps + 1}),
                        }))
                    }
                }),
                context,
            )
            .await
            .map_err(PlainFailure::from)
    })
}

#[tokio::test]
async fn oracle_scheduler_decision_grids() {
    let expected = oracle_grid();
    let storage = Arc::new(MemoryStorage::new());
    let session = create_session(Arc::clone(&storage) as Arc<dyn Storage>);
    let registry = Arc::new(GridRegistry {
        tasks: Mutex::new(BTreeMap::new()),
    });
    registry
        .tasks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(String::from("probe"), grid_task("probe", 1, "probe-run"));
    registry
        .tasks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(String::from("oldish"), grid_task("oldish", 1, "none"));

    let report_log: Arc<Mutex<Vec<String>>> = Arc::default();
    let scheduler = TaskScheduler::new(TaskSchedulerOptions {
        session: Arc::clone(&session),
        storage: Arc::clone(&storage) as Arc<dyn Storage>,
        registry: Arc::clone(&registry) as Arc<dyn RegistryReaderLike>,
        models: None,
        env: None,
        now: Arc::new(|| 1_758_240_010_000.0),
        report: {
            let report_log = Arc::clone(&report_log);
            Arc::new(move |error: &PlainError| {
                report_log
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(error.message.clone());
            })
        },
        settle_outcome: Arc::new(|_tx, _record, _outcome| Ok(())),
        withdraw_inputs: Arc::new(|_tx, _conversation_id| Ok(())),
        conversation: Arc::new(|_id, _binding, _context| Box::pin(async { Ok(None) })),
        context: Context::background(),
    });
    scheduler.open(Context::background()).await.unwrap();

    session
        .commit(
            |tx: Arc<Transaction>| {
                Box::pin(async move { tx.create_root_conversation().map(|record| record.id) })
            },
            Context::background(),
        )
        .await
        .unwrap();
    let probe_definition = registry.snapshot().task("probe").expect("probe registered");
    let probe_id = session
        .commit(
            move |tx: Arc<Transaction>| {
                Box::pin(async move {
                    tx.create_task(
                        &probe_definition,
                        serde_json::json!({ "n": 1 }),
                        crate::durable::types::TaskOptions {
                            ownership: TaskOwnership::Conversation,
                            conversation_id: Some(1),
                            background: None,
                        },
                    )
                })
            },
            Context::background(),
        )
        .await
        .unwrap();
    scheduler.resume();
    let probe_settled = scheduler
        .wait_for_task(probe_id, Context::background())
        .await
        .unwrap();

    let expected_probe = &expected["probeSettled"];
    assert_eq!(probe_settled.id, expected_probe["id"].as_i64().unwrap());
    assert_eq!(probe_settled.kind, expected_probe["kind"].as_str().unwrap());
    let state_wire = wire(&serde_json::to_value(&probe_settled.state).unwrap());
    assert_eq!(
        state_wire,
        wire(&expected_probe["state"]),
        "probe settled state bytes"
    );

    // Oldish task; then swap to a newer definition without `migrate`.
    let oldish_definition = registry
        .snapshot()
        .task("oldish")
        .expect("oldish registered");
    let oldish_id = session
        .commit(
            move |tx: Arc<Transaction>| {
                Box::pin(async move {
                    tx.create_task(
                        &oldish_definition,
                        serde_json::json!({}),
                        crate::durable::types::TaskOptions {
                            ownership: TaskOwnership::Conversation,
                            conversation_id: Some(1),
                            background: None,
                        },
                    )
                })
            },
            Context::background(),
        )
        .await
        .unwrap();
    registry
        .tasks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(String::from("oldish"), grid_task("oldish", 2, "none"));
    let (scheduling, tasks) = scheduler.inspect(registry.snapshot()).await.unwrap();
    assert_eq!(scheduling, SchedulingState::Running);
    let oldish = tasks
        .iter()
        .find(|entry| entry.record.kind == "oldish")
        .expect("oldish inspected");
    let expected_oldish = &expected["inspectionGrid"][0];
    assert_eq!(
        oldish.record.kind,
        expected_oldish["kind"].as_str().unwrap()
    );
    match &oldish.state {
        TaskInspectionState::Blocked { reason, .. } => {
            assert_eq!(
                reason.as_str(),
                expected_oldish["state"]["reason"].as_str().unwrap()
            );
        }
        other => panic!("expected blocked oldish, got {other:?}"),
    }

    // Orphan grid: no definition can take the oldish task.
    registry
        .tasks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove("oldish");
    let orphan = scheduler
        .abort(oldish_id, Context::background())
        .await
        .unwrap();
    assert!(
        format!("{:?}", orphan).contains("Marked"),
        "orphan abort marks"
    );
    let orphan_record = storage
        .task(oldish_id, &Context::background())
        .unwrap()
        .unwrap();
    let orphan_wire = wire(&serde_json::to_value(&orphan_record.state).unwrap());
    assert_eq!(
        orphan_wire,
        wire(&expected["orphan"]["state"]),
        "orphan state bytes"
    );

    assert!(
        report_log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty(),
        "report log must stay empty"
    );
}

// ─── Scenario 8: submissions state machine ──────────────────────────────────

#[tokio::test]
async fn oracle_submissions_state_machine() {
    let expected = oracle()["submissions_state"].clone();
    let storage = Arc::new(MemoryStorage::new());
    let session = create_session(Arc::clone(&storage) as Arc<dyn Storage>);
    session
        .commit(
            |tx: Arc<Transaction>| {
                Box::pin(async move { tx.create_root_conversation().map(|record| record.id) })
            },
            Context::background(),
        )
        .await
        .unwrap();

    let resume_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let submissions = Submissions::new(
        Arc::clone(&session),
        Arc::clone(&storage) as Arc<dyn Storage>,
        Arc::new(|| 1_758_240_020_000.0),
        {
            let resume_count = Arc::clone(&resume_count);
            Arc::new(move || {
                resume_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        },
    )
    .unwrap();

    fn read(
        storage: &Arc<dyn Storage>,
        id: i64,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Option<crate::durable::types::SubmissionRecord>>
                + Send
                + '_,
        >,
    > {
        let storage = Arc::clone(storage);
        Box::pin(async move { storage.submission(id, &Context::background()).unwrap() })
    }

    // Idle input: places a user entry, creates a placed submission, starts a
    // run.
    let idle_input = submissions
        .submit(
            1,
            SubmissionDraft::Input {
                request_id: None,
                content: StringOrBlocks::Text(String::from("hello")),
                when_busy: None,
            },
            Context::background(),
        )
        .await
        .unwrap();
    let idle_record = read(&(Arc::clone(&storage) as Arc<dyn Storage>), idle_input)
        .await
        .unwrap();
    assert_eq!(
        wire(&serde_json::to_value(&idle_record).unwrap()),
        wire(&expected["idleInput"]),
        "idle input bytes"
    );

    let live = live_doc();
    let live_state = session
        .snapshot(&live.definition, Some(1), None, Context::background())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        wire(
            &live_state
                .get("run")
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        ),
        wire(&expected["liveRun"]),
        "live run bytes"
    );

    // Busy steer and follow-up queue.
    let steer = submissions
        .submit(
            1,
            SubmissionDraft::Input {
                request_id: None,
                content: StringOrBlocks::Text(String::from("steer me")),
                when_busy: Some(WhenBusy::Steer),
            },
            Context::background(),
        )
        .await
        .unwrap();
    let steer_record = read(&(Arc::clone(&storage) as Arc<dyn Storage>), steer)
        .await
        .unwrap();
    assert_eq!(
        wire(&serde_json::to_value(&steer_record).unwrap()),
        wire(&expected["steer"])
    );
    let follow_up = submissions
        .submit(
            1,
            SubmissionDraft::Input {
                request_id: None,
                content: StringOrBlocks::Text(String::from("follow up")),
                when_busy: Some(WhenBusy::FollowUp),
            },
            Context::background(),
        )
        .await
        .unwrap();
    let follow_up_record = read(&(Arc::clone(&storage) as Arc<dyn Storage>), follow_up)
        .await
        .unwrap();
    assert_eq!(
        wire(&serde_json::to_value(&follow_up_record).unwrap()),
        wire(&expected["followUp"])
    );

    // Busy reject fails without writing.
    let busy_error = submissions
        .submit(
            1,
            SubmissionDraft::Input {
                request_id: None,
                content: StringOrBlocks::Text(String::from("nope")),
                when_busy: Some(WhenBusy::Reject),
            },
            Context::background(),
        )
        .await;
    assert_eq!(
        busy_error.unwrap_err().message,
        expected["busyReject"]["message"].as_str().unwrap()
    );

    // Request-id dedup resolves to the first submission.
    let dedup_first = submissions
        .submit(
            1,
            SubmissionDraft::Input {
                request_id: Some(String::from("req-1")),
                content: StringOrBlocks::Text(String::from("once")),
                when_busy: None,
            },
            Context::background(),
        )
        .await
        .unwrap();
    let dedup_second = submissions
        .submit(
            1,
            SubmissionDraft::Input {
                request_id: Some(String::from("req-1")),
                content: StringOrBlocks::Text(String::from("once")),
                when_busy: None,
            },
            Context::background(),
        )
        .await
        .unwrap();
    assert_eq!(
        dedup_first == dedup_second,
        expected["dedup"]["same"].as_bool().unwrap()
    );
    assert_eq!(
        dedup_first,
        expected["dedup"]["first"].as_i64().unwrap(),
        "dedup id alignment"
    );

    // Queued write behind the run.
    let queued_write = submissions
        .submit(
            1,
            SubmissionDraft::Write {
                request_id: None,
                entry: serde_json::from_value::<EntryDraft>(serde_json::json!(
                    { "kind": "note", "data": { "text": "w" } }
                ))
                .unwrap(),
            },
            Context::background(),
        )
        .await
        .unwrap();
    let queued_record = read(&(Arc::clone(&storage) as Arc<dyn Storage>), queued_write)
        .await
        .unwrap();
    assert_eq!(
        wire(&serde_json::to_value(&queued_record).unwrap()),
        wire(&expected["queuedWrite"])
    );

    // Withdraw the steer.
    let steer_abort = submissions
        .abort(steer, Context::background(), Some(1))
        .await
        .unwrap();
    assert!(matches!(
        steer_abort,
        crate::durable::harness::types::AbortSubmissionResult::Found(
            crate::durable::harness::types::AbortResult::Aborted
        )
    ));
    let steer_after = read(&(Arc::clone(&storage) as Arc<dyn Storage>), steer)
        .await
        .unwrap();
    assert_eq!(
        wire(&serde_json::to_value(&steer_after).unwrap()),
        wire(&expected["steerAfterAbort"]),
        "steer after abort bytes"
    );

    let placed = submissions
        .status(idle_input, Context::background())
        .await
        .unwrap();
    assert_eq!(
        format!("{:?}", placed.status).to_lowercase(),
        expected["placedStatus"].as_str().unwrap()
    );

    assert_eq!(
        resume_count.load(std::sync::atomic::Ordering::SeqCst),
        expected["resumeCount"].as_u64().unwrap() as usize,
        "resume call count"
    );
}

// Silence unused-import warnings for the surface the tests reference.
#[allow(unused_imports)]
use crate::ai::types::UserMessage;

#[allow(unused_imports)]
fn _unused_context_view(_view: &ContextView) {}

#[allow(unused_imports)]
fn _unused_conversation_config() -> ConversationConfigState {
    ConversationConfigState::initial()
}

#[allow(unused_imports)]
fn _unused_conversation_config_doc() -> crate::durable::documents::DocToken {
    conversation_config()
}

#[allow(unused_imports)]
fn _unused_system_message() -> SystemMessage {
    SystemMessage {
        content: StringOrBlocks::Text(String::new()),
        sections: None,
        tools_added: None,
        tools_removed: None,
        timestamp: 0,
    }
}

#[allow(unused_imports)]
fn _unused_usage() -> Usage {
    Usage::default()
}

#[allow(unused_imports)]
fn _unused_message() -> Option<Message> {
    None
}

use std::future::Future;
use std::pin::Pin;
