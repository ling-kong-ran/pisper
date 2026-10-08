//! Ports of the storage-level oracles: `spec-storage-history.test.ts`
//! (atomic staging, snapshot isolation, JSONL recovery), `hardening.test.ts`
//! (read isolation, membrane nested-assignment), and the storage halves of
//! `atomicity.test.ts` (chop/replay, torn tails, stale sidecars).

use std::sync::Arc;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::jsonl::JsonlStorage;
use crate::agent_core::harness::pico3::memory::MemoryStorage;
use crate::agent_core::harness::pico3::tests::support::*;
use crate::agent_core::harness::pico3::types::{
    Conversation, DocRef, Entry, Input, Storage, Task, Write,
};

use futures::FutureExt;
use serde_json::{json, Value};

fn conversation(id: i64) -> Write {
    Write::Conversation {
        conversation: Conversation {
            id,
            parent: None,
            owner: None,
            sections: None,
        },
    }
}

fn rewindable_base(conversation_id: i64, extra: serde_json::Value) -> Write {
    let mut value = json!({
        "thinkingLevel": "off",
        "selectedTools": [],
        "profile": "default",
        "threshold": 0,
        "keepRecent": 20000,
        "plugins": {},
    });
    if let (Some(target), Some(source)) = (value.as_object_mut(), extra.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    Write::Doc {
        r#ref: DocRef::Rewindable { conversation_id },
        ops: vec![crate::agent_core::chord_support::delta::Op::Replace(value)],
    }
}

/// `spec-storage-history.test.ts` "MemoryStorage validates a complete batch
/// before publishing tables, documents, sequence, or id high-water"
/// (`memory.ts:20-24`).
#[tokio::test]
async fn memory_storage_validates_batches_atomically() {
    let storage = MemoryStorage::new();
    let invalid = vec![
        conversation(40),
        Write::Doc {
            r#ref: DocRef::Session,
            ops: vec![crate::agent_core::chord_support::delta::Op::Replace(
                json!({
                    "plugins": { "leaked": { "value": true } }
                }),
            )],
        },
        Write::Entry {
            entry: Entry {
                id: 40,
                conversation_id: 40,
                kind: "duplicate-global-id".to_owned(),
                model: None,
                data: None,
                head: None,
                edits: None,
                by_task_id: None,
            },
        },
    ];
    // The batch claims id 40 twice (conversation + entry): rejected.
    let error = storage.commit(invalid, ctx()).await.unwrap_err();
    assert!(format!("{error}").contains("created twice"), "{error}");
    assert!(storage.conversation(40, ctx()).await.unwrap().is_none());
    assert!(storage.entries(&[40], ctx()).await.unwrap().is_empty());
    let session_doc = storage.doc(&DocRef::Session, ctx()).await.unwrap().unwrap();
    assert_eq!(
        session_doc.get("plugins").cloned(),
        Some(json!({})),
        "the rejected batch leaked no document ops"
    );
    storage.commit(vec![conversation(1)], ctx()).await.unwrap();
    assert_eq!(
        storage.mint_id(),
        2,
        "the rejected id 40 did not move the high-water"
    );
    storage.close(ctx()).await.unwrap();
}

/// `spec-storage-history.test.ts` "storage reads are isolated snapshots for
/// every mutable durable family" (`memory.ts` clone discipline).
#[tokio::test]
async fn storage_reads_are_isolated_snapshots() {
    let storage = MemoryStorage::new();
    storage
        .commit(
            vec![
                Write::Conversation {
                    conversation: Conversation {
                        id: 1,
                        parent: None,
                        owner: None,
                        sections: Some(vec![
                            crate::agent_core::harness::pico3::types::SectionSeed::Set {
                                key: "x".to_owned(),
                                value: json!({ "nested": 1 }),
                            },
                        ]),
                    },
                },
                Write::Entry {
                    entry: Entry {
                        id: 2,
                        conversation_id: 1,
                        kind: "note".to_owned(),
                        model: None,
                        data: Some(
                            json!({ "nested": { "value": 1 } })
                                .as_object()
                                .cloned()
                                .unwrap(),
                        ),
                        head: None,
                        edits: None,
                        by_task_id: None,
                    },
                },
                Write::Task {
                    task: Task {
                        id: 3,
                        conversation_id: 1,
                        kind: "task".to_owned(),
                        input: json!({ "nested": 1 }),
                        status: crate::agent_core::harness::pico3::types::TaskStatus::Pending,
                        checkpoint: None,
                        abort: None,
                        outcome: None,
                        after: vec![],
                        owns: vec![],
                        background: None,
                    },
                },
                Write::Input {
                    input: Input {
                        id: 4,
                        conversation_id: 1,
                        request_id: Some("r".to_owned()),
                        status: "queued".to_owned(),
                        entry: None,
                        answer: None,
                        reason: None,
                        detail: None,
                    },
                },
                Write::Doc {
                    r#ref: DocRef::Rewindable { conversation_id: 1 },
                    ops: vec![crate::agent_core::chord_support::delta::Op::Replace(
                        json!({
                            "plugins": { "p": { "nested": { "value": 1 } } }
                        }),
                    )],
                },
            ],
            ctx(),
        )
        .await
        .unwrap();
    let mut returned_conversation = storage.conversation(1, ctx()).await.unwrap().unwrap();
    let mut returned_entry = storage
        .entries(&[2], ctx())
        .await
        .unwrap()
        .remove(&2)
        .unwrap();
    let mut returned_task = storage.task(3, ctx()).await.unwrap().unwrap();
    let mut returned_input = storage
        .input_by_request(1, "r", ctx())
        .await
        .unwrap()
        .unwrap();
    let mut returned_doc = storage
        .doc(&DocRef::Rewindable { conversation_id: 1 }, ctx())
        .await
        .unwrap()
        .unwrap();

    if let Some(sections) = &mut returned_conversation.sections {
        if let crate::agent_core::harness::pico3::types::SectionSeed::Set { value, .. } =
            &mut sections[0]
        {
            *value = json!("mutated");
        }
    }
    returned_entry.data = Some(json!({}).as_object().cloned().unwrap());
    returned_task.input = json!(9);
    returned_input.status = "done".to_owned();
    returned_doc.insert(
        "plugins".to_owned(),
        json!({ "p": { "nested": { "value": 9 } } }),
    );

    let conversation = storage.conversation(1, ctx()).await.unwrap().unwrap();
    let sections = conversation.sections.unwrap();
    match &sections[0] {
        crate::agent_core::harness::pico3::types::SectionSeed::Set { value, .. } => {
            assert_eq!(
                *value,
                json!({ "nested": 1 }),
                "conversation sections are snapshots"
            );
        }
        crate::agent_core::harness::pico3::types::SectionSeed::Remove { .. } => unreachable!(),
    }
    let entry = storage
        .entries(&[2], ctx())
        .await
        .unwrap()
        .remove(&2)
        .unwrap();
    assert_eq!(
        entry.data.map(|data| json!(data)).unwrap(),
        json!({ "nested": { "value": 1 } }),
    );
    assert_eq!(
        storage.task(3, ctx()).await.unwrap().unwrap().input,
        json!({ "nested": 1 })
    );
    assert_eq!(
        storage
            .input_by_request(1, "r", ctx())
            .await
            .unwrap()
            .unwrap()
            .status,
        "queued"
    );
    assert_eq!(
        json!(storage
            .doc(&DocRef::Rewindable { conversation_id: 1 }, ctx())
            .await
            .unwrap()
            .unwrap()),
        json!({ "plugins": { "p": { "nested": { "value": 1 } } } }),
    );
}

/// `spec-storage-history.test.ts` "fork state is commit-granular" at the
/// storage level (`memory.docAsOf`, `memory.ts:230-243`).
#[tokio::test]
async fn rewindable_history_is_commit_granular() {
    let storage = MemoryStorage::new();
    storage
        .commit(vec![conversation(1), rewindable_base(1, json!({}))], ctx())
        .await
        .unwrap();
    // The anchor entry and the final doc state share one atomic commit
    // (upstream: `tx.write` + `tx.plugins(...).version` in one commit).
    storage
        .commit(
            vec![
                Write::Entry {
                    entry: Entry {
                        id: 2,
                        conversation_id: 1,
                        kind: "anchor".to_owned(),
                        model: None,
                        data: None,
                        head: None,
                        edits: None,
                        by_task_id: None,
                    },
                },
                Write::Doc {
                    r#ref: DocRef::Rewindable { conversation_id: 1 },
                    ops: vec![crate::agent_core::chord_support::delta::Op::Set {
                        path: vec![crate::agent_core::chord_support::delta::Seg::Key(
                            "version".to_owned(),
                        )],
                        value: json!("same-commit-final"),
                    }],
                },
            ],
            ctx(),
        )
        .await
        .unwrap();
    let as_of = storage.doc_as_of(1, 2, ctx()).await.unwrap().unwrap();
    assert_eq!(
        as_of.get("version").cloned(),
        Some(json!("same-commit-final")),
        "an entry observes the final rewindable state of its atomic commit"
    );
}

/// `memory.ts:177-195` fork-aware newest-first scan (the `deriveContext`
/// dependency the oracle suite relies on).
#[tokio::test]
async fn scan_entries_is_fork_aware_and_newest_first() {
    let storage = MemoryStorage::new();
    let entry = |id: i64, conversation_id: i64, kind: &str, head: Option<i64>| Write::Entry {
        entry: Entry {
            id,
            conversation_id,
            kind: kind.to_owned(),
            model: None,
            data: None,
            head,
            edits: None,
            by_task_id: None,
        },
    };
    storage
        .commit(
            vec![
                conversation(1),
                Write::Conversation {
                    conversation: Conversation {
                        id: 2,
                        parent: Some(
                            crate::agent_core::harness::pico3::types::ConversationParent {
                                conversation_id: 1,
                                at: 12,
                            },
                        ),
                        owner: None,
                        sections: None,
                    },
                },
                // Upstream's `created` guard is global across tables per
                // batch (`memory.ts:62-67`), so entry ids avoid the
                // conversation ids.
                entry(11, 1, "user", None),
                entry(12, 1, "assistant", None),
                entry(13, 2, "user", Some(12)),
                entry(14, 2, "assistant", None),
            ],
            ctx(),
        )
        .await
        .unwrap();
    let scan = crate::agent_core::harness::pico3::types::EntryScan {
        conversation_id: 2,
        limit: 100,
        ..Default::default()
    };
    let entries = storage.scan_entries(&scan, ctx()).await.unwrap();
    let ids: Vec<i64> = entries.iter().map(|entry| entry.id).collect();
    assert_eq!(
        ids,
        vec![14, 13, 12, 11],
        "fork-aware: own entries, then parent up to the fork point"
    );
    // `before` is strictly less; parent entries cap at parent.at + 1.
    let capped = storage
        .scan_entries(
            &crate::agent_core::harness::pico3::types::EntryScan {
                conversation_id: 2,
                before: Some(14),
                limit: 100,
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    let capped_ids: Vec<i64> = capped.iter().map(|entry| entry.id).collect();
    assert_eq!(
        capped_ids,
        vec![13, 12, 11],
        "the parent prefix below the fork cap stays visible"
    );
    // withHead filters to head entries only.
    let heads = storage
        .scan_entries(
            &crate::agent_core::harness::pico3::types::EntryScan {
                conversation_id: 2,
                with_head: true,
                limit: 100,
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    assert_eq!(
        heads.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        vec![13],
        "only the fork head is head-visible"
    );
}

/// `atomicity.test.ts` "torn tail: bytes after the last newline are
/// truncated on open; a later append starts a fresh line".
#[tokio::test]
async fn torn_tails_are_truncated_on_open() {
    let env = Env::open_jsonl().await.unwrap();
    let dir = env.dir.clone().unwrap();
    env.crash().await.unwrap();
    std::fs::write(dir.join("main.jsonl.torn"), b"").ok();
    {
        use std::io::Write as IoWrite;
        let mut main = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("main.jsonl"))
            .unwrap();
        main.write_all(b"{\"seq\":99,\"maxId\":9,\"wri").unwrap();
    }
    // Reopen on the torn dir; replay must not trip over the partial line.
    let env2 = Env::reopen_jsonl(dir.clone()).await.unwrap();
    // A later commit appends a complete record.
    env2.commit_kernel(|_tx, _ctx| async { Ok(()) }.boxed())
        .await
        .unwrap();
    drop(env2);
    let content = std::fs::read_to_string(dir.join("main.jsonl")).unwrap();
    for line in content.lines() {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|error| panic!("every line is a complete record: {error} in {line}"));
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Round-1 review fix 3: a file whose entire content is a torn record (no
/// newline at all) truncates to zero on open (`jsonl.ts:337-338`:
/// `lastIndexOf` -1 -> cut to 0), so a later append cannot extend the
/// garbage line.
#[tokio::test]
async fn newline_less_torn_file_truncates_to_zero() {
    let env = Env::open_jsonl().await.unwrap();
    let dir = env.dir.clone().unwrap();
    env.crash().await.unwrap();
    std::fs::write(dir.join("main.jsonl"), b"{\"seq\":99,\"maxId\":9,\"wri").unwrap();
    let reopened = JsonlStorage::open(&dir, false).await.unwrap();
    assert_eq!(
        std::fs::metadata(dir.join("main.jsonl")).unwrap().len(),
        0,
        "the newline-less torn record is cut to zero"
    );
    reopened.close(ctx()).await.unwrap();

    // The emptied directory replays clean and a later append writes a
    // complete line.
    let env2 = Env::reopen_jsonl(dir.clone()).await.unwrap();
    let _ = env2
        .commit_host(|tx, _ctx| {
            async move {
                tx.write(
                    1,
                    crate::agent_core::harness::pico3::types::NewEntry::new("note"),
                )
                .await?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
    drop(env2);
    let content = std::fs::read_to_string(dir.join("main.jsonl")).unwrap();
    assert!(!content.is_empty());
    for line in content.lines() {
        serde_json::from_str::<Value>(line)
            .unwrap_or_else(|error| panic!("every line is a complete record: {error} in {line}"));
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// `atomicity.test.ts` "a malformed complete record fails open" (storage
/// halves: non-monotonic seq; wrong-typed seq).
#[tokio::test]
async fn malformed_records_fail_open() {
    let env = Env::open_jsonl().await.unwrap();
    let dir = env.dir.clone().unwrap();
    env.crash().await.unwrap();
    // Non-monotonic seq, complete line.
    {
        use std::io::Write as IoWrite;
        let mut main = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("main.jsonl"))
            .unwrap();
        let line = serde_json::to_vec(&json!({ "seq": 1, "maxId": 1, "writes": [] })).unwrap();
        main.write_all(&line).unwrap();
        main.write_all(b"\n").unwrap();
    }
    let error = JsonlStorage::open(&dir, false).await.unwrap_err();
    assert!(
        format!("{error}").contains("sequence not increasing"),
        "{error}"
    );
    // Wrong-typed seq: "lacks seq".
    {
        use std::io::Write as IoWrite;
        let mut main = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(dir.join("main.jsonl"))
            .unwrap();
        let line = serde_json::to_vec(&json!({ "seq": "x" })).unwrap();
        main.write_all(&line).unwrap();
        main.write_all(b"\n").unwrap();
    }
    let error = JsonlStorage::open(&dir, false).await.unwrap_err();
    assert!(format!("{error}").contains("lacks seq"), "{error}");
    let _ = std::fs::remove_dir_all(dir);
}

/// `spec-storage-history.test.ts` "unconfirmed JSONL tails are ignored and
/// cannot advance committed id high-water" (`jsonl.ts:152-167`).
#[tokio::test]
async fn unconfirmed_sidecar_tails_are_ignored() {
    let env = Env::open_jsonl().await.unwrap();
    let dir = env.dir.clone().unwrap();
    // A confirmed commit so main has a marker.
    let _ = env
        .commit_host(|tx, _ctx| {
            async move {
                tx.write(
                    1,
                    crate::agent_core::harness::pico3::types::NewEntry::new("confirmed"),
                )
                .await
                .map(|_| ())
            }
            .boxed()
        })
        .await
        .unwrap();
    env.crash().await.unwrap();
    let main = std::fs::read_to_string(dir.join("main.jsonl")).unwrap();
    let last_line = main.lines().last().unwrap();
    let last: serde_json::Value = serde_json::from_str(last_line).unwrap();
    let next_seq = last["seq"].as_i64().unwrap() + 1;
    // An unconfirmed sidecar for an unknown conversation, with a huge id.
    let sidecar = format!(
        "{}\n",
        serde_json::to_string(&json!({
            "seq": next_seq,
            "maxId": 999_999,
            "writes": [{
                "type": "doc",
                "ref": { "doc": "sticky", "conversationId": 999 },
                "ops": [["r", { "inbox": [], "turn": { "tools": [] }, "tasks": {}, "plugins": {} }]],
            }],
        }))
        .unwrap()
    );
    std::fs::write(dir.join("sticky-999.jsonl"), sidecar).unwrap();
    let storage = JsonlStorage::open(&dir, false).await.unwrap();
    assert!(
        storage.mint_id() < 999_999,
        "an unconfirmed tail cannot advance the committed high-water"
    );
    let doc = storage
        .doc(
            &DocRef::Sticky {
                conversation_id: 999,
            },
            ctx(),
        )
        .await
        .unwrap();
    assert!(doc.is_none(), "the unconfirmed sidecar is not applied");
    storage.close(ctx()).await.unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

/// `spec-storage-history.test.ts` "JSONL replay rejects a published marker
/// whose expected sidecar record is missing" (`jsonl.ts:168-180`).
#[tokio::test]
async fn missing_sidecar_record_for_published_marker_fails_open() {
    let env = Env::open_jsonl().await.unwrap();
    let dir = env.dir.clone().unwrap();
    // Produce a commit with a sticky sidecar + main marker.
    let _ = env
        .commit_host(|tx, _ctx| {
            async move {
                tx.config_set(1, "followUpMode", json!("all"))?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
    env.crash().await.unwrap();
    let sidecar = dir.join("sticky-1.jsonl");
    let sidecar_content = std::fs::read_to_string(&sidecar).unwrap();
    let records: Vec<&str> = sidecar_content
        .lines()
        .filter(|line| !line.is_empty())
        .collect();
    assert!(records.len() >= 2, "a seed base and the config write exist");
    let truncated: String = records[..records.len() - 1]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&sidecar, truncated).unwrap();
    let error = JsonlStorage::open(&dir, false).await.unwrap_err();
    assert!(
        format!("{error}").contains("missing record for published sequence"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// `atomicity.test.ts` "an unpublished sidecar tail is truncated before its
/// sequence is reused" (storage-level chop variant).
#[tokio::test]
async fn unpublished_sidecar_tail_is_truncated_before_reuse() {
    let env = Env::open_jsonl().await.unwrap();
    let dir = env.dir.clone().unwrap();
    let _ = env
        .commit_host(|tx, _ctx| {
            async move {
                tx.config_set(1, "followUpMode", json!("all"))?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
    env.crash().await.unwrap();
    // Chop the publication marker: the sticky half remains but is unconfirmed.
    let main_path = dir.join("main.jsonl");
    let main_content = std::fs::read_to_string(&main_path).unwrap();
    let lines: Vec<&str> = main_content
        .lines()
        .filter(|line| !line.is_empty())
        .collect();
    let kept: String = lines[..lines.len() - 1]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&main_path, kept).unwrap();

    // Reopen: the tail is truncated; the config write is gone.
    let env2 = Env::reopen_jsonl(dir.clone()).await.unwrap();
    assert_eq!(
        env2.sticky(1).await.unwrap().get("followUpMode").cloned(),
        Some(json!("one-at-a-time")),
    );
    // The sequence is safely reused.
    let _ = env2
        .commit_host(|tx, _ctx| {
            async move {
                tx.config_set(1, "followUpMode", json!("all"))?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
    drop(env2);
    let sequences: Vec<i64> = std::fs::read_to_string(dir.join("sticky-1.jsonl"))
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["seq"]
                .as_i64()
                .unwrap()
        })
        .collect();
    for pair in sequences.windows(2) {
        assert!(
            pair[1] > pair[0],
            "sequences strictly increase: {sequences:?}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// `atomicity.test.ts` "a stale sidecar for a terminal task cannot resurrect
/// state" (storage half).
#[tokio::test]
async fn stale_task_sidecar_cannot_resurrect_terminal_state() {
    let env = Env::open_jsonl().await.unwrap();
    let dir = env.dir.clone().unwrap();
    let kind = plugin_kind(&env);
    let task_id = env
        .commit_kernel(move |tx, _ctx| {
            let kind = kind.clone();
            async move {
                tx.create_task_kind(
                    &kind,
                    serde_json::Value::Null,
                    crate::agent_core::harness::pico3::session::CreateTaskOptions {
                        conversation_id: Some(1),
                        background: true,
                        after: Vec::new(),
                    },
                )
                .map(|reference| reference.id)
            }
            .boxed()
        })
        .await
        .unwrap()
        .value;
    // Terminalize: the terminal patch goes to main; the sticky slot delete
    // lands in the sticky sidecar (`jsonl.ts:209-215` split).
    env.commit_kernel(move |tx, _ctx| {
        async move {
            let mut task = tx.task(task_id).await?.expect("task exists");
            task.status = crate::agent_core::harness::pico3::types::TaskStatus::Terminal;
            task.outcome = Some(crate::agent_core::harness::pico3::types::Outcome::orphaned());
            tx.set_task(task)?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    env.crash().await.unwrap();
    // Forge a stale running record into the (unlinked) task sidecar.
    std::fs::write(
        dir.join(format!("task-{task_id}.jsonl")),
        serde_json::to_string(&json!({
            "seq": 1,
            "maxId": 1,
            "writes": [{
                "type": "task.patch",
                "patch": { "id": task_id, "status": "running", "checkpoint": { "phase": "started" } }
            }],
        }))
        .unwrap()
        + "\n",
    )
    .unwrap();
    let env2 = Env::reopen_jsonl(dir.clone()).await.unwrap();
    let task = env2.storage().task(task_id, ctx()).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        crate::agent_core::harness::pico3::types::TaskStatus::Terminal,
        "the unconfirmed running record is ignored: main's terminal record wins"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// `hardening.test.ts` "MemoryStorage reads cannot mutate authoritative
/// records" — entry-level half of the snapshot isolation oracle.
#[tokio::test]
async fn memory_storage_entry_reads_cannot_mutate_records() {
    let storage = Arc::new(MemoryStorage::new());
    storage
        .commit(
            vec![
                conversation(1),
                Write::Entry {
                    entry: Entry {
                        id: 2,
                        conversation_id: 1,
                        kind: "original".to_owned(),
                        model: None,
                        data: None,
                        head: None,
                        edits: None,
                        by_task_id: None,
                    },
                },
            ],
            ctx(),
        )
        .await
        .unwrap();
    let mut first = storage
        .entries(&[2], ctx())
        .await
        .unwrap()
        .remove(&2)
        .unwrap();
    first.kind = "mutated".to_owned();
    let entry = storage
        .entries(&[2], ctx())
        .await
        .unwrap()
        .remove(&2)
        .unwrap();
    assert_eq!(entry.kind, "original");
}

/// A document commit failure mid-session leaves later operations working
/// when the storage recovers — and the Session line keeps working across
/// reopen (`spec-storage-history` rejected-cut half at storage level).
#[tokio::test]
async fn rejected_batches_leave_cached_documents_unpublished() {
    struct Rejecting {
        inner: MemoryStorage,
        reject: std::sync::atomic::AtomicBool,
    }
    impl Storage for Rejecting {
        fn commit<'a>(
            &'a self,
            writes: Vec<Write>,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<i64>> {
            Box::pin(async move {
                if self.reject.load(std::sync::atomic::Ordering::SeqCst) {
                    anyhow::bail!("publication cut");
                }
                self.inner.commit(writes, context).await
            })
        }
        fn mint_id(&self) -> i64 {
            self.inner.mint_id()
        }
        fn conversation<'a>(
            &'a self,
            id: i64,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<Option<Conversation>>> {
            Box::pin(async move { self.inner.conversation(id, context).await })
        }
        fn conversations<'a>(
            &'a self,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<Vec<Conversation>>> {
            Box::pin(async move { self.inner.conversations(context).await })
        }
        fn entries<'a>(
            &'a self,
            ids: &[i64],
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<std::collections::HashMap<i64, Entry>>>
        {
            let ids = ids.to_vec();
            Box::pin(async move { self.inner.entries(&ids, context).await })
        }
        fn scan_entries<'a>(
            &'a self,
            scan: &crate::agent_core::harness::pico3::types::EntryScan,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
            let scan = scan.clone();
            Box::pin(async move { self.inner.scan_entries(&scan, context).await })
        }
        fn task<'a>(
            &'a self,
            id: i64,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<Option<Task>>> {
            Box::pin(async move { self.inner.task(id, context).await })
        }
        fn scan_tasks<'a>(
            &'a self,
            scan: &crate::agent_core::harness::pico3::types::TaskScan,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<Vec<Task>>> {
            let scan = scan.clone();
            Box::pin(async move { self.inner.scan_tasks(&scan, context).await })
        }
        fn input<'a>(
            &'a self,
            id: i64,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<Option<Input>>> {
            Box::pin(async move { self.inner.input(id, context).await })
        }
        fn input_by_request<'a>(
            &'a self,
            conversation_id: i64,
            request_id: &str,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<Option<Input>>> {
            let request_id = request_id.to_owned();
            Box::pin(async move {
                self.inner
                    .input_by_request(conversation_id, &request_id, context)
                    .await
            })
        }
        fn doc<'a>(
            &'a self,
            r#ref: &DocRef,
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<Option<serde_json::Map<String, serde_json::Value>>>,
        > {
            let r#ref = *r#ref;
            Box::pin(async move { self.inner.doc(&r#ref, context).await })
        }
        fn doc_as_of<'a>(
            &'a self,
            conversation_id: i64,
            at: i64,
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<Option<serde_json::Map<String, serde_json::Value>>>,
        > {
            Box::pin(async move { self.inner.doc_as_of(conversation_id, at, context).await })
        }
        fn truncate<'a>(
            &'a self,
            r#ref: &DocRef,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<()>> {
            let r#ref = *r#ref;
            Box::pin(async move { self.inner.truncate(&r#ref, context).await })
        }
        fn close<'a>(
            &'a self,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async move { self.inner.close(context).await })
        }
    }

    let inner = MemoryStorage::new();
    let rejecting = Arc::new(Rejecting {
        inner,
        reject: std::sync::atomic::AtomicBool::new(false),
    });
    let env = Env::open_with_storage(rejecting.clone() as Arc<dyn Storage>)
        .await
        .unwrap();
    let state = env.namespace(ns(
        "test.rejected-cut",
        json!({ "cut": { "shouldNotPersist": false } }),
        json!({}),
    ));
    rejecting
        .reject
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = env
        .commit_host(|tx, _ctx| {
            let state = state.clone();
            async move {
                let mut view = tx.plugins(&state)?;
                view.set("cut", json!({ "shouldNotPersist": true }))?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "Faulted");
    // The cached mutated document never published.
    let stored = env.rewindable(1).await.unwrap();
    assert!(
        stored
            .get("plugins")
            .and_then(|plugins| plugins.get("test.rejected-cut"))
            .is_none(),
        "the rejected cut left no namespace slice behind"
    );
}

/// Upstream memory.ts scanTasks iterates a JS Map, not sorted ids. A patch
/// does not change position; JSONL replay must retain the same iteration order.
#[tokio::test]
async fn task_scans_preserve_insertion_order_through_patches_filters_and_replay() {
    use crate::agent_core::harness::pico3::types::{TaskPatch, TaskScan, TaskStatus};

    let dir = tempfile::tempdir().unwrap();
    for jsonl in [false, true] {
        let storage: Arc<dyn Storage> = if jsonl {
            JsonlStorage::open(dir.path(), true).await.unwrap()
        } else {
            Arc::new(MemoryStorage::new())
        };
        let task = |id, conversation_id, kind: &str| Write::Task {
            task: Task {
                id,
                conversation_id,
                kind: kind.to_owned(),
                input: Value::Null,
                status: TaskStatus::Pending,
                checkpoint: None,
                abort: None,
                outcome: None,
                after: vec![],
                owns: vec![],
                background: None,
            },
        };
        storage
            .commit(
                vec![
                    conversation(1),
                    conversation(2),
                    task(90, 1, "a"),
                    task(7, 1, "a"),
                    task(50, 2, "b"),
                ],
                ctx(),
            )
            .await
            .unwrap();
        let mut patch = TaskPatch::new(90);
        patch.status = Some(TaskStatus::Terminal);
        storage
            .commit(vec![Write::TaskPatch { patch }, task(12, 1, "b")], ctx())
            .await
            .unwrap();
        let ids = |tasks: Vec<Task>| tasks.into_iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(
            ids(storage
                .scan_tasks(&TaskScan::default(), ctx())
                .await
                .unwrap()),
            vec![90, 7, 50, 12]
        );
        assert_eq!(
            ids(storage
                .scan_tasks(
                    &TaskScan {
                        conversation_id: Some(1),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap()),
            vec![90, 7, 12]
        );
        assert_eq!(
            ids(storage
                .scan_tasks(
                    &TaskScan {
                        status: Some(vec![TaskStatus::Pending]),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap()),
            vec![7, 50, 12]
        );
        assert_eq!(
            ids(storage
                .scan_tasks(
                    &TaskScan {
                        kind: Some("a".to_owned()),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap()),
            vec![90, 7]
        );
        storage.close(ctx()).await.unwrap();
        if jsonl {
            let reopened = JsonlStorage::open(dir.path(), false).await.unwrap();
            assert_eq!(
                ids(reopened
                    .scan_tasks(&TaskScan::default(), ctx())
                    .await
                    .unwrap()),
                vec![90, 7, 50, 12]
            );
            reopened.close(ctx()).await.unwrap();
        }
    }
}
