//! Ports of the view oracles: `spec-view-events.test.ts` (snapshot shape,
//! revisions, pre-start buffering, listener isolation), `view.test.ts`
//! (fold == fresh snapshot), and the ViewManager wiring the harness would
//! install (Task 9; here the tests drive `update`/`deliver` at the upstream
//! call positions).

use crate::agent_core::harness::pico3::tests::support::*;
use crate::agent_core::harness::pico3::types::NewEntry;
use crate::agent_core::harness::pico3::view::{apply_envelope, ViewManager, WATCH_CAPACITY};

use futures::FutureExt;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// Wire the ViewManager into a session the way `harness.ts` will at Task 9
/// (line listener: update+deliver; post-line listener: deliver).
fn attach_view(env: &Env) -> Arc<ViewManager> {
    let manager = ViewManager::new(env.session.clone());
    let line_manager = manager.clone();
    env.session.add_line_listener(Arc::new(move |record| {
        line_manager.update(record);
    }));
    let post_manager = manager.clone();
    env.session.add_listener(Arc::new(move |_record| {
        post_manager.deliver();
    }));
    manager
}

/// A watch over the root conversation (upstream `conversation.watch`).
async fn watch_root(
    manager: &ViewManager,
    env: &Env,
) -> Arc<crate::agent_core::harness::pico3::view::Watch> {
    let conversation = env
        .session
        .conversation_records()
        .get(&1)
        .cloned()
        .expect("root conversation");
    let entries = env.entries(1).await.unwrap();
    manager.watch(&conversation, entries).unwrap()
}

/// `spec-view-events.test.ts` "watch snapshot is the single flat rendering
/// document and excludes raw documents, records, checkpoints, and private
/// slot state" (`view.ts:149-164`).
#[tokio::test]
async fn watch_snapshot_is_the_flat_rendering_document() {
    let env = Env::open_memory().await.unwrap();
    let manager = attach_view(&env);
    let watch = watch_root(&manager, &env).await;
    let view = &watch.view;
    let view_json = json!(view);
    let keys: Vec<String> = view_json.as_object().unwrap().keys().cloned().collect();
    for key in [
        "conversation",
        "entries",
        "config",
        "inbox",
        "tasks",
        "plugins",
    ] {
        assert!(
            keys.iter().any(|existing| existing == key),
            "flat view lacks {key}: {keys:?}"
        );
    }
    assert!(
        !keys.iter().any(|existing| existing == "rewindable"),
        "raw documents never render"
    );
    assert!(
        !keys.iter().any(|existing| existing == "sticky"),
        "raw documents never render"
    );
    for task in view.tasks.values() {
        let task_json = json!(task);
        assert!(
            task_json.get("checkpoint").is_none(),
            "checkpoints never render"
        );
        assert!(task_json.get("outcome").is_none(), "outcomes never render");
        assert!(
            task_json.get("memos").is_none(),
            "private slot state never renders"
        );
    }
    watch.stop();
}

/// `spec-view-events.test.ts` "watch revisions are contiguous per watch even
/// when unrelated conversation commits create global storage gaps"
/// (`view.ts:120-127`).
#[tokio::test]
async fn watch_revisions_are_contiguous_per_conversation() {
    let env = Env::open_memory().await.unwrap();
    let manager = attach_view(&env);
    let other = env
        .commit_kernel(|tx, _ctx| {
            async move { tx.create_conversation(&Default::default()) }.boxed()
        })
        .await
        .unwrap()
        .value;
    let watch = watch_root(&manager, &env).await;
    let revisions: std::sync::Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = revisions.clone();
    watch.start(Arc::new(move |envelope| {
        sink.lock().unwrap().push(envelope.revision);
    }));
    env.commit_host(|tx, _ctx| {
        async move {
            tx.write(1, NewEntry::new("root.one")).await?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    env.commit_conversation(other, |tx, _ctx| {
        async move {
            tx.write(other, NewEntry::new("other.only")).await?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    env.commit_host(|tx, _ctx| {
        async move {
            tx.write(1, NewEntry::new("root.two")).await?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let revisions = revisions.lock().unwrap().clone();
    assert_eq!(
        revisions,
        vec![watch.revision + 1, watch.revision + 2],
        "unrelated conversation commits create no gaps"
    );
    watch.stop();
}

/// `spec-view-events.test.ts` "late joiner reconstructs streaming generation
/// state without replayed events" (`view.ts:182-199`), at storage level: a
/// live generation task with a `requesting` checkpoint and a sticky turn
/// message reconstructs the streaming turn view.
#[tokio::test]
async fn late_joiner_reconstructs_turn_state_without_replayed_events() {
    let env = Env::open_memory().await.unwrap();
    let generation = generation_kind(&env);
    env.commit_kernel(move |tx, _ctx| {
        let generation = generation.clone();
        async move {
            let reference = tx.create_task_kind(
                &generation,
                json!({ "inputs": [1] }),
                crate::agent_core::harness::pico3::session::CreateTaskOptions {
                    conversation_id: Some(1),
                    ..Default::default()
                },
            )?;
            let mut task = tx.task(reference.id).await?.expect("task");
            task.checkpoint = Some(
                json!({ "phase": "requesting", "attempt": 1 })
                    .as_object()
                    .cloned()
                    .unwrap(),
            );
            tx.set_task(task)?;
            tx.sticky_set(1, "turn", json!({ "message": { "role": "assistant", "content": [{ "type": "text", "text": "partial" }] }, "tools": [] }))?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let manager = attach_view(&env);
    let watch = watch_root(&manager, &env).await;
    let seen: std::sync::Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    watch.start(Arc::new(move |envelope| {
        sink.lock().unwrap().push(envelope.revision);
    }));
    let turn = watch.view.turn.as_ref().expect("the live turn renders");
    match &turn.generation {
        Some(crate::agent_core::harness::pico3::types::GenerationStatus::Streaming { attempt })
        | Some(crate::agent_core::harness::pico3::types::GenerationStatus::Requesting {
            attempt,
        }) => {
            assert_eq!(*attempt, 1);
        }
        other => panic!("expected a requesting/streaming stage, got {other:?}"),
    }
    assert_eq!(
        turn.message
            .as_ref()
            .and_then(|message| message["content"][0]["text"].as_str()),
        Some("partial"),
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "capture does not replay historical events"
    );
    watch.stop();
}

/// `spec-view-events.test.ts` "pre-start delivery is ordered, start is
/// idempotent, and stop has a hard no-callback boundary" (`view.ts:287-315`).
#[tokio::test]
async fn pre_start_delivery_is_ordered_and_stop_is_a_boundary() {
    let env = Env::open_memory().await.unwrap();
    let manager = attach_view(&env);
    let watch = watch_root(&manager, &env).await;
    for note in ["buffered.one", "buffered.two"] {
        env.commit_host(move |tx, _ctx| {
            let note = note;
            async move {
                tx.write(1, NewEntry::new(note)).await?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
    }
    let received: std::sync::Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = received.clone();
        watch.start(Arc::new(move |envelope| {
            sink.lock().unwrap().push(envelope.revision);
        }));
        // Second start is ignored.
        watch.start(Arc::new(|_envelope| {
            panic!("second start must be ignored");
        }));
    }
    let received_after_start = received.lock().unwrap().clone();
    assert_eq!(
        received_after_start.len(),
        2,
        "the pre-start buffer replays in order"
    );
    assert_eq!(received_after_start[1], received_after_start[0] + 1);
    let count_at_stop = received.lock().unwrap().len();
    watch.stop();
    watch.stop(); // idempotent
    env.commit_host(|tx, _ctx| {
        async move {
            tx.write(1, NewEntry::new("after.stop")).await?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    assert_eq!(
        received.lock().unwrap().len(),
        count_at_stop,
        "no callbacks after stop"
    );
}

/// `spec-view-events.test.ts` "listener failure is isolated from persistence
/// and from sibling watches" (`view.ts:317-323`).
#[tokio::test]
async fn listener_failure_is_isolated() {
    let env = Env::open_memory().await.unwrap();
    let manager = attach_view(&env);
    let reports: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let reports = reports.clone();
        env.session
            .set_on_report(Arc::new(move |error: &anyhow::Error| {
                reports.lock().unwrap().push(format!("{error}"));
            }));
    }
    let broken = watch_root(&manager, &env).await;
    let healthy = watch_root(&manager, &env).await;
    let healthy_events: std::sync::Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
    broken.start(Arc::new(|_envelope| {
        panic!("listener failed");
    }));
    {
        let sink = healthy_events.clone();
        healthy.start(Arc::new(move |envelope| {
            sink.lock().unwrap().push(envelope.revision);
        }));
    }
    env.commit_host(|tx, _ctx| {
        async move {
            tx.write(1, NewEntry::new("survives.listener")).await?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    assert!(broken.is_closed(), "the broken watch closed");
    assert!(!healthy.is_closed(), "sibling watches survive");
    assert_eq!(healthy_events.lock().unwrap().len(), 1);
    assert!(
        reports
            .lock()
            .unwrap()
            .iter()
            .any(|report| report.contains("listener failed")),
        "the failure was reported: {:?}",
        reports.lock().unwrap()
    );
    healthy.stop();
}

/// `view.test.ts` "watch: capture + stream == fresh snapshot" at storage
/// level: folding the received envelopes equals a fresh capture
/// (`applyEnvelope`, `view.ts:31-33`).
#[tokio::test]
async fn folded_envelopes_equal_a_fresh_snapshot() {
    let env = Env::open_memory().await.unwrap();
    let manager = attach_view(&env);
    let watch = watch_root(&manager, &env).await;
    let folded: std::sync::Arc<
        Mutex<Option<crate::agent_core::harness::pico3::types::ConversationView>>,
    > = Arc::new(Mutex::new(Some(watch.view.clone())));
    {
        let folded = folded.clone();
        watch.start(Arc::new(move |envelope| {
            let mut guard = folded.lock().unwrap();
            let current = guard.take().expect("seeded");
            *guard = Some(apply_envelope(&current, envelope).expect("envelope applies"));
        }));
    }
    for note in ["one", "two"] {
        env.commit_host(move |tx, _ctx| {
            let note = note;
            async move {
                tx.write(1, NewEntry::new(note)).await?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
    }
    let folded = folded.lock().unwrap().clone().unwrap();
    let fresh = watch_root(&manager, &env).await;
    assert_eq!(
        folded
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        fresh
            .view
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        "folded transcript matches a fresh capture"
    );
    assert_eq!(
        json!(folded.entries),
        json!(fresh.view.entries),
        "the folded view converges to the fresh snapshot"
    );
    fresh.stop();
    watch.stop();
}

/// `spec-view-events.test.ts` "a head commit is represented by transcript
/// splice ops and matching entry/head events" (`view.ts:337-354`).
#[tokio::test]
async fn head_commits_produce_splice_ops_and_head_events() {
    let env = Env::open_memory().await.unwrap();
    let manager = attach_view(&env);
    env.commit_host(|tx, _ctx| {
        async move {
            tx.write(1, NewEntry::new("before")).await?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let watch = watch_root(&manager, &env).await;
    let envelopes: std::sync::Arc<Mutex<Vec<crate::agent_core::harness::pico3::types::Envelope>>> =
        Arc::new(Mutex::new(Vec::new()));
    {
        let envelopes = envelopes.clone();
        watch.start(Arc::new(move |envelope| {
            envelopes.lock().unwrap().push(envelope.clone());
        }));
    }
    // A core append with a head truncates the transcript (the storage-level
    // half of `reset`).
    env.commit_kernel(|tx, _ctx| {
        async move {
            let head = tx
                .newest_entry(1, None, true)
                .await?
                .map(|entry| entry.id)
                .unwrap_or_default();
            tx.append_entry(
                1,
                NewEntry {
                    kind: "pi.summary".to_owned(),
                    head: Some(crate::agent_core::harness::pico3::types::Head::Id(head)),
                    ..NewEntry::default()
                },
            )?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let envelopes = envelopes.lock().unwrap().clone();
    assert!(
        !envelopes.is_empty(),
        "the head commit produced an envelope"
    );
    let envelope = &envelopes[0];
    let splices: Vec<&crate::agent_core::chord_support::delta::Op> = envelope
        .ops
        .iter()
        .filter(|op| {
            matches!(op, crate::agent_core::chord_support::delta::Op::Splice { path, .. }
                if matches!(path.first(), Some(crate::agent_core::chord_support::delta::Seg::Key(key)) if key == "entries"))
        })
        .collect();
    assert!(
        !splices.is_empty(),
        "the head truncation is a transcript splice"
    );
    let event_types: Vec<String> = envelope
        .events
        .iter()
        .map(|event| event.event_type())
        .collect();
    assert!(
        event_types.contains(&"head.moved".to_owned()),
        "{event_types:?}"
    );
    assert!(
        event_types.contains(&"entry.added".to_owned()),
        "{event_types:?}"
    );
    watch.stop();
}

/// `view.ts:55-70` capacity: an un-started watch stops after
/// `WATCH_CAPACITY` buffered envelopes.
#[tokio::test]
async fn unstarted_watches_stop_at_capacity() {
    let env = Env::open_memory().await.unwrap();
    let manager = attach_view(&env);
    let watch = watch_root(&manager, &env).await;
    for index in 0..(WATCH_CAPACITY + 1) {
        env.commit_host(move |tx, _ctx| {
            async move {
                tx.write(1, NewEntry::new(format!("note-{index}"))).await?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
    }
    assert!(
        watch.is_closed(),
        "the watch stopped itself after exceeding capacity before start()"
    );
}

/// Round-1 review fix 2 (view half): upstream renders a stored or declared
/// null config value into `ConversationView.config` (`view.ts:176-178`:
/// `value !== undefined`); only absence is skipped.
#[tokio::test]
async fn view_config_includes_declared_nulls() {
    let mut kinds = stub_kinds();
    kinds.insert(
        "pi.plugin".to_owned(),
        std::sync::Arc::new(
            crate::agent_core::harness::pico3::types::BasicKind::new("pi.plugin").config(
                crate::agent_core::harness::pico3::types::KindConfig {
                    rewindable: Default::default(),
                    sticky: json!({ "note": null }).as_object().cloned().unwrap(),
                    declared_absent: Default::default(),
                },
            ),
        ),
    );
    let env = Env::open_memory_with_kinds(kinds).await.unwrap();
    let manager = attach_view(&env);
    let watch = watch_root(&manager, &env).await;
    let config = json!(watch.view)["config"]
        .as_object()
        .cloned()
        .expect("config object");
    assert!(
        config.contains_key("note"),
        "a declared null config value renders (it is a value, not absence): {config:?}"
    );
    assert_eq!(config.get("note"), Some(&json!(null)));
    watch.stop();
}

/// Round-1 review fix 5: stopping a watcher detaches it from its record, and
/// the record is evicted when its last watcher stops (`view.ts:63-67`), so
/// the manager does not accumulate records or deliver to dead watchers.
#[tokio::test]
async fn watchers_detach_and_records_evict_on_stop() {
    let env = Env::open_memory().await.unwrap();
    let manager = attach_view(&env);
    let first = watch_root(&manager, &env).await;
    let second = watch_root(&manager, &env).await;
    assert_eq!(manager.watcher_counts(), vec![(1, 2)]);
    first.stop();
    assert_eq!(
        manager.watcher_counts(),
        vec![(1, 1)],
        "the stopped watcher is detached"
    );
    second.stop();
    assert!(
        manager.watcher_counts().is_empty(),
        "the record is evicted when its last watcher stops: {:?}",
        manager.watcher_counts()
    );
    // A later watch rebuilds a fresh record.
    let third = watch_root(&manager, &env).await;
    assert_eq!(manager.watcher_counts(), vec![(1, 1)]);
    third.stop();
    assert!(manager.watcher_counts().is_empty());
}

/// `spec-view-events.test.ts` "a namespace projection failure closes its
/// watches without failing the persisted writer" (`view.ts:243-259` +
/// `update`'s record-drop path).
#[tokio::test]
async fn projection_failure_closes_watches_but_not_the_writer() {
    let env = Env::open_memory().await.unwrap();
    let manager = attach_view(&env);
    let reports: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let reports = reports.clone();
        env.session
            .set_on_report(Arc::new(move |error: &anyhow::Error| {
                reports.lock().unwrap().push(format!("{error}"));
            }));
    }
    let failing = TestNamespace::new(
        "spec.bad-projection",
        json!({}),
        json!({ "fail": false }),
        json!({}),
    );
    let token = env.namespace(failing.clone());
    // The watch builds while the projection is healthy.
    let watch = watch_root(&manager, &env).await;
    watch.start(Arc::new(|_envelope| {}));
    // Then the projection is re-registered failing (the harness's
    // re-registration path; the port replaces the registration wholesale).
    env.session.namespaces().write().unwrap().insert(
        token.id.clone(),
        crate::agent_core::harness::pico3::types::NamespaceRegistration {
            project: Some(Arc::new(|_slice| anyhow::bail!("projection failed"))),
            ..failing.registration.clone()
        },
    );
    let persisted = env
        .commit_host(|tx, _ctx| {
            let token = token.clone();
            async move {
                let mut view = tx.plugins(&token)?;
                view.set("fail", json!(true))?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
    assert!(
        persisted.seq.is_some(),
        "the writer is never failed by a projection"
    );
    assert!(watch.is_closed(), "the projection failure closed the watch");
    assert!(
        reports
            .lock()
            .unwrap()
            .iter()
            .any(|report| report.contains("projection failed")),
        "{:?}",
        reports.lock().unwrap()
    );
    assert_eq!(
        env.sticky(1)
            .await
            .unwrap()
            .get("plugins")
            .and_then(|plugins| plugins.get("spec.bad-projection"))
            .and_then(|slice| slice.get("fail"))
            .cloned(),
        Some(json!(true)),
        "the persisted state kept the write"
    );
}
