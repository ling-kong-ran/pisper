//! Flattened port of
//! `packages/agent/src/harness/session/testing/conformance/storage.ts` (920
//! lines), instantiated for `JsonlStorage` exactly as
//! `jsonl-storage-conformance.test.ts` does (NOW = 1_700_000_000_000, header
//! cwd "/workspace"). The upstream suite is a runner-independent case list
//! parametrized across backends; this phase instantiates it only for the
//! JSONL backend, so the case bodies are flat tests (see the testing module
//! docs). Group names are kept in the test names.

use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::env::NodeExecutionEnv;
use crate::agent_core::harness::session::commit::{insert_entry, insert_usage};
use crate::agent_core::harness::session::jsonl::storage::JsonlStorage;
use crate::agent_core::harness::session::jsonl::types::{
    JsonlStorageHeader, JsonlStorageOptions, JSONL_STORAGE_VERSION,
};
use crate::agent_core::harness::session::types::{
    EntryType, NewEntry, NewUsageRow, Storage, StorageBranchScan, Write,
};
use crate::agent_core::harness::session::values::{
    append_list, branch_tip, delete_list, delete_value, entry_label, list, pending_entry,
    session_name, set_value, value, ListReadOptions,
};
use crate::agent_core::types::AgentMessage;
use crate::ai::types::message::{StringOrBlocks, UserMessage};

use crate::ai::types::primitives::Usage;
use std::sync::Arc;

const NOW: i64 = 1_700_000_000_000;
const MESSAGE_TIMESTAMP: i64 = 1_650_000_000_000;

/// The conformance `usage(input, output)` fixture
/// (`conformance/storage.ts:54-71`).
fn usage(input: i64, output: i64) -> Usage {
    Usage {
        input: input as u64,
        output: output as u64,
        cache_read: (input + 1) as u64,
        cache_write: (output + 1) as u64,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: (input + output) as u64,
        cost: crate::ai::types::primitives::UsageCost {
            input: input as f64 / 100.0,
            output: output as f64 / 100.0,
            cache_read: (input + 1) as f64 / 100.0,
            cache_write: (output + 1) as f64 / 100.0,
            total: (input + output + 2) as f64 / 100.0,
        },
    }
}

fn zero_usage() -> Usage {
    Usage::default()
}

fn user_entry(id: &str, parent_id: Option<&str>, text: &str) -> Write {
    insert_entry(NewEntry::Message {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        message: AgentMessage::User(UserMessage {
            content: StringOrBlocks::Blocks(vec![
                crate::ai::types::message::TextOrImageBlock::Text(crate::ai::types::TextContent {
                    text: text.to_string(),
                    text_signature: None,
                }),
            ]),
            timestamp: MESSAGE_TIMESTAMP,
        }),
        terminate: None,
    })
}

fn custom_entry(id: &str, parent_id: Option<&str>, custom_type: &str) -> Write {
    insert_entry(NewEntry::Custom {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        custom_type: custom_type.to_string(),
        data: Some(serde_json::json!({ "id": id })),
    })
}

fn compaction_entry(id: &str, parent_id: Option<&str>) -> Write {
    insert_entry(NewEntry::Compaction {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        summary: format!("summary:{id}"),
        retained_tail: Vec::new(),
        tokens_before: 10,
        details: None,
        usage: None,
        from_hook: false,
    })
}

fn usage_row(id: &str, input: i64, output: i64, adjustment: bool) -> NewUsageRow {
    usage_row_for(id, input, output, adjustment, None)
}

fn usage_row_for(
    id: &str,
    input: i64,
    output: i64,
    adjustment: bool,
    entry_id: Option<&str>,
) -> NewUsageRow {
    NewUsageRow {
        id: id.to_string(),
        usage: usage(input, output),
        entry_id: entry_id.map(str::to_string),
        adjustment,
        details: None,
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    storage: JsonlStorage,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let file_system = Arc::new(NodeExecutionEnv::new(
        dir.path().to_string_lossy().to_string(),
    ));
    let storage = JsonlStorage::create(
        JsonlStorageOptions {
            file_system,
            path: "session.jsonl".to_string(),
            now: Some(Arc::new(|| NOW)),
        },
        JsonlStorageHeader::new("session", JSONL_STORAGE_VERSION, NOW, "/workspace"),
        Vec::new(),
        background_context(),
    )
    .await
    .unwrap();
    Fixture { _dir: dir, storage }
}

fn ids(entries: &[crate::agent_core::harness::session::types::Entry]) -> Vec<String> {
    entries.iter().map(|entry| entry.id().to_string()).collect()
}

fn the_list(key: &str) -> crate::agent_core::harness::session::values::ValueAddress {
    list("test.list", key)
}

/// transactions / "commits mixed writes atomically in write order".
#[tokio::test]
async fn conformance_transactions_commit_mixed_writes_in_order() {
    let fixture = fixture().await;
    let storage = &fixture.storage;
    let name = session_name();
    let result = storage
        .commit(
            vec![
                user_entry("entry", None, "entry"),
                set_value(&name, serde_json::json!("session")),
                insert_usage(usage_row_for("usage", 2, 3, false, Some("entry"))),
            ],
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(result.seqs.len(), 3);
    assert_eq!(result.first_seq, result.seqs[0]);
    assert_eq!(
        result.stats,
        storage.get_stats(background_context()).await.unwrap()
    );
    assert!(result.seqs.windows(2).all(|w| w[0] < w[1]));
    let entries = storage
        .get_entries(&["entry".to_string()], background_context())
        .await
        .unwrap();
    let entry = &entries["entry"];
    assert_eq!(entry.seq(), result.seqs[0]);
    assert_eq!(entry.timestamp(), result.timestamp);
    let stored = storage
        .get_value(&name, background_context())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (stored.value, stored.seq),
        (serde_json::json!("session"), result.seqs[1])
    );
    let rows = storage
        .scan_usage(&Default::default(), background_context())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "usage");
    assert_eq!(rows[0].seq, result.seqs[2]);
    assert_eq!(rows[0].usage.input, 2);
    assert!(!rows[0].adjustment);
    assert_eq!(rows[0].entry_id.as_deref(), Some("entry"));
}

/// transactions / "rolls back every store when a mixed transaction fails".
#[tokio::test]
async fn conformance_transactions_rollback_on_failure() {
    let fixture = fixture().await;
    let storage = &fixture.storage;
    let name = session_name();
    storage
        .commit(
            vec![
                user_entry("root", None, "root"),
                insert_usage(usage_row("taken", 1, 1, false)),
            ],
            background_context(),
        )
        .await
        .unwrap();
    let entries_before = storage
        .scan_entries(&Default::default(), background_context())
        .await
        .unwrap();
    let usage_before = storage
        .scan_usage(&Default::default(), background_context())
        .await
        .unwrap();
    let stats_before = storage.get_stats(background_context()).await.unwrap();

    let error = storage
        .commit(
            vec![
                set_value(&name, serde_json::json!("transient")),
                custom_entry("transient-entry", Some("root"), "note"),
                insert_usage(usage_row("transient-usage", 5, 8, true)),
                custom_entry("taken", Some("root"), "note"),
            ],
            background_context(),
        )
        .await
        .unwrap_err();
    let _ = error;
    assert_eq!(
        storage
            .scan_entries(&Default::default(), background_context())
            .await
            .unwrap(),
        entries_before
    );
    assert_eq!(
        storage
            .scan_usage(&Default::default(), background_context())
            .await
            .unwrap(),
        usage_before
    );
    assert_eq!(
        storage.get_stats(background_context()).await.unwrap(),
        stats_before
    );
    assert!(storage
        .get_value(&name, background_context())
        .await
        .unwrap()
        .is_none());
}

/// transactions / "enforces one shared entry and usage id namespace".
#[tokio::test]
async fn conformance_transactions_shared_id_namespace() {
    let fixture = fixture().await;
    let storage = &fixture.storage;
    storage
        .commit(
            vec![
                user_entry("existing-entry", None, "x"),
                insert_usage(usage_row("existing-usage", 1, 1, false)),
            ],
            background_context(),
        )
        .await
        .unwrap();

    assert!(storage
        .commit(
            vec![insert_usage(usage_row("existing-entry", 2, 2, false))],
            background_context()
        )
        .await
        .is_err());
    assert!(storage
        .commit(
            vec![custom_entry("existing-usage", None, "note")],
            background_context()
        )
        .await
        .is_err());
    assert!(storage
        .commit(
            vec![
                custom_entry("entry-then-usage", None, "note"),
                insert_usage(usage_row("entry-then-usage", 3, 3, false))
            ],
            background_context()
        )
        .await
        .is_err());
    assert!(storage
        .commit(
            vec![
                insert_usage(usage_row("usage-then-entry", 4, 4, false)),
                custom_entry("usage-then-entry", None, "note")
            ],
            background_context()
        )
        .await
        .is_err());

    let entries = storage
        .scan_entries(&Default::default(), background_context())
        .await
        .unwrap();
    assert_eq!(ids(&entries), vec!["existing-entry"]);
    let rows = storage
        .scan_usage(&Default::default(), background_context())
        .await
        .unwrap();
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        vec!["existing-usage"]
    );
}

/// transactions / "resolves parents only from prior entries and earlier
/// writes".
#[tokio::test]
async fn conformance_transactions_parent_resolution() {
    let fixture = fixture().await;
    let storage = &fixture.storage;
    storage
        .commit(vec![user_entry("root", None, "root")], background_context())
        .await
        .unwrap();
    storage
        .commit(
            vec![
                custom_entry("child", Some("root"), "note"),
                custom_entry("grandchild", Some("child"), "note"),
            ],
            background_context(),
        )
        .await
        .unwrap();
    let scan = storage
        .scan_branch(
            &StorageBranchScan {
                start: "grandchild".to_string(),
                order: Some(
                    crate::agent_core::harness::session::types::BranchScanOrder::OldestFirst,
                ),
                ..Default::default()
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&scan), vec!["root", "child", "grandchild"]);

    assert!(storage
        .commit(
            vec![
                custom_entry("before-parent", Some("later-parent"), "note"),
                custom_entry("later-parent", Some("root"), "note"),
                set_value(
                    &entry_label("before-parent"),
                    serde_json::json!("transient")
                ),
            ],
            background_context(),
        )
        .await
        .is_err());
    assert!(storage
        .commit(
            vec![custom_entry("orphan", Some("missing"), "note")],
            background_context()
        )
        .await
        .is_err());
    storage
        .commit(
            vec![insert_usage(usage_row("usage-is-not-parent", 1, 1, false))],
            background_context(),
        )
        .await
        .unwrap();
    assert!(storage
        .commit(
            vec![custom_entry(
                "usage-child",
                Some("usage-is-not-parent"),
                "note"
            )],
            background_context()
        )
        .await
        .is_err());

    assert!(storage
        .get_entries(
            &[
                "before-parent".to_string(),
                "later-parent".to_string(),
                "orphan".to_string(),
                "usage-child".to_string()
            ],
            background_context()
        )
        .await
        .unwrap()
        .is_empty());
    assert!(storage
        .get_value(&entry_label("before-parent"), background_context())
        .await
        .unwrap()
        .is_none());
}

/// transactions / "places pending content under its reserved entry id".
#[tokio::test]
async fn conformance_transactions_pending_content_placement() {
    let fixture = fixture().await;
    let storage = &fixture.storage;
    let pending = pending_entry("reserved");
    let tip = branch_tip("main");
    storage
        .commit(
            vec![
                set_value(
                    &pending,
                    serde_json::json!({ "type": "custom", "customType": "queued" }),
                ),
                set_value(&tip, serde_json::Value::Null),
            ],
            background_context(),
        )
        .await
        .unwrap();

    assert!(!storage
        .get_entries(&["reserved".to_string()], background_context())
        .await
        .unwrap()
        .contains_key("reserved"));
    assert_eq!(
        storage
            .get_value(&pending, background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!({ "type": "custom", "customType": "queued" })
    );
    assert_eq!(
        storage
            .get_value(&tip, background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::Value::Null
    );

    let placement = storage
        .commit(
            vec![
                user_entry("reserved", None, "queued"),
                delete_value(&pending),
                set_value(&tip, serde_json::json!("reserved")),
            ],
            background_context(),
        )
        .await
        .unwrap();
    let entries = storage
        .get_entries(&["reserved".to_string()], background_context())
        .await
        .unwrap();
    assert_eq!(entries["reserved"].seq(), placement.seqs[0]);
    assert!(storage
        .get_value(&pending, background_context())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        storage
            .get_value(&tip, background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!("reserved")
    );
    let _ = MESSAGE_TIMESTAMP;
}

/// values / "sets, replaces, deletes, and recreates values without
/// tombstones" + scan ordering by key.
#[tokio::test]
async fn conformance_values_set_delete_recreate() {
    let fixture = fixture().await;
    let storage = &fixture.storage;
    let name = session_name();
    let v = |key: &str| value("test.value", key);
    let first = storage
        .commit(
            vec![
                set_value(&v("prefix/b"), serde_json::json!(1)),
                set_value(&v("prefix/a"), serde_json::json!(2)),
                set_value(&v("other"), serde_json::json!(3)),
                set_value(&v("prefix/a"), serde_json::Value::Null),
            ],
            background_context(),
        )
        .await
        .unwrap();
    let stored = storage
        .get_value(&v("prefix/a"), background_context())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.value, serde_json::Value::Null);
    assert_eq!(stored.seq, first.seqs[3]);

    let second = storage
        .commit(
            vec![
                delete_value(&v("prefix/a")),
                delete_value(&v("absent")),
                set_value(&v("prefix/a"), serde_json::json!("recreated")),
            ],
            background_context(),
        )
        .await
        .unwrap();

    let scanned = storage
        .scan_values(&v("prefix/"), background_context())
        .await
        .unwrap();
    assert_eq!(
        scanned
            .iter()
            .map(|stored| (
                stored.address.key.as_str(),
                stored.value.clone(),
                stored.seq
            ))
            .collect::<Vec<_>>(),
        vec![
            ("prefix/a", serde_json::json!("recreated"), second.seqs[2]),
            ("prefix/b", serde_json::json!(1), first.seqs[0]),
        ]
    );
    assert!(storage
        .get_value(&v("absent"), background_context())
        .await
        .unwrap()
        .is_none());
    let _ = name;
}

/// lists / "pages appends by global sequence and deletes whole lists".
#[tokio::test]
async fn conformance_lists_paging_and_deletion() {
    let fixture = fixture().await;
    let storage = &fixture.storage;
    let address = the_list("events");
    let name = session_name();
    assert!(storage
        .read_list(&address, None, background_context())
        .await
        .unwrap()
        .is_empty());
    let result = storage
        .commit(
            vec![
                append_list(&address, serde_json::json!("a")),
                set_value(&name, serde_json::json!("gap")),
                append_list(&address, serde_json::json!("b")),
                append_list(&address, serde_json::json!("c")),
            ],
            background_context(),
        )
        .await
        .unwrap();
    let read = |options: Option<&ListReadOptions>| {
        storage.read_list(&address, options, background_context())
    };
    assert_eq!(
        read(None)
            .await
            .unwrap()
            .iter()
            .map(|e| e.value.clone())
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!("a"),
            serde_json::json!("b"),
            serde_json::json!("c")
        ]
    );
    let limited = read(Some(&ListReadOptions {
        cursor: None,
        order: None,
        limit: Some(2),
    }))
    .await
    .unwrap();
    assert_eq!(
        limited.iter().map(|e| e.value.clone()).collect::<Vec<_>>(),
        vec![serde_json::json!("a"), serde_json::json!("b")]
    );
    let paged = read(Some(&ListReadOptions {
        cursor: Some(crate::agent_core::harness::session::values::ListCursor {
            seq: result.seqs[0],
        }),
        order: None,
        limit: Some(2),
    }))
    .await
    .unwrap();
    assert_eq!(
        paged.iter().map(|e| e.value.clone()).collect::<Vec<_>>(),
        vec![serde_json::json!("b"), serde_json::json!("c")]
    );
    let desc = read(Some(&ListReadOptions {
        cursor: None,
        order: Some(crate::agent_core::harness::session::types::AscDescOrder::Desc),
        limit: Some(2),
    }))
    .await
    .unwrap();
    assert_eq!(
        desc.iter().map(|e| e.value.clone()).collect::<Vec<_>>(),
        vec![serde_json::json!("c"), serde_json::json!("b")]
    );
    // limit: 0 rejects.
    assert!(read(Some(&ListReadOptions {
        cursor: None,
        order: None,
        limit: Some(0)
    }))
    .await
    .is_err());

    storage
        .commit(
            vec![
                delete_list(&address),
                append_list(&address, serde_json::json!("new")),
            ],
            background_context(),
        )
        .await
        .unwrap();
    let after = read(None).await.unwrap();
    assert_eq!(
        after.iter().map(|e| e.value.clone()).collect::<Vec<_>>(),
        vec![serde_json::json!("new")]
    );
}

/// entry queries / "scans global entries with explicit ranges, filters,
/// orders, and limits".
#[tokio::test]
async fn conformance_entry_queries_scan_entries() {
    let fixture = fixture().await;
    let storage = &fixture.storage;
    let result = storage
        .commit(
            vec![
                user_entry("root", None, "root"),
                custom_entry("note-1", Some("root"), "note"),
                custom_entry("other", Some("note-1"), "other"),
                custom_entry("note-2", Some("other"), "note"),
                user_entry("tail", Some("note-2"), "tail"),
            ],
            background_context(),
        )
        .await
        .unwrap();

    let scanned = storage
        .scan_entries(
            &crate::agent_core::harness::session::types::EntryScan {
                scan_type: Some(EntryType::Custom),
                custom_type: Some("note".to_string()),
                from_seq: Some(result.seqs[1]),
                to_seq: Some(result.seqs[3]),
                order: Some(crate::agent_core::harness::session::types::AscDescOrder::Desc),
                limit: None,
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&scanned), vec!["note-2", "note-1"]);
    let limited = storage
        .scan_entries(
            &crate::agent_core::harness::session::types::EntryScan {
                order: Some(crate::agent_core::harness::session::types::AscDescOrder::Asc),
                limit: Some(2),
                ..Default::default()
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&limited), vec!["root", "note-1"]);
    let desc = storage
        .scan_entries(
            &crate::agent_core::harness::session::types::EntryScan {
                order: Some(crate::agent_core::harness::session::types::AscDescOrder::Desc),
                limit: Some(2),
                ..Default::default()
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&desc), vec!["tail", "note-2"]);
}

/// branch queries / "applies stops before filters and cursors before
/// limits".
#[tokio::test]
async fn conformance_branch_queries_stops_filters_cursors() {
    let fixture = fixture().await;
    let storage = &fixture.storage;
    let result = storage
        .commit(
            vec![
                user_entry("root", None, "root"),
                custom_entry("marker", Some("root"), "marker"),
                user_entry("middle", Some("marker"), "middle"),
                compaction_entry("compact", Some("middle")),
                custom_entry("note", Some("compact"), "note"),
                user_entry("leaf", Some("note"), "leaf"),
            ],
            background_context(),
        )
        .await
        .unwrap();

    let scan = |query: StorageBranchScan| storage.scan_branch(&query, background_context());
    let scanned = scan(StorageBranchScan {
        start: "leaf".to_string(),
        stop_at_type: Some(EntryType::Compaction),
        scan_type: Some(EntryType::Message),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(ids(&scanned), vec!["leaf"]);
    let scanned = scan(StorageBranchScan {
        start: "leaf".to_string(),
        order: Some(crate::agent_core::harness::session::types::BranchScanOrder::OldestFirst),
        stop_at_id: Some("middle".to_string()),
        scan_type: Some(EntryType::Custom),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(ids(&scanned), vec!["marker"]);
    let scanned = scan(StorageBranchScan {
        start: "leaf".to_string(),
        cursor: Some(crate::agent_core::harness::session::types::EntryCursor {
            seq: result.seqs[4],
        }),
        limit: Some(2),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(ids(&scanned), vec!["compact", "middle"]);
    let scanned = scan(StorageBranchScan {
        start: "leaf".to_string(),
        order: Some(crate::agent_core::harness::session::types::BranchScanOrder::OldestFirst),
        cursor: Some(crate::agent_core::harness::session::types::EntryCursor {
            seq: result.seqs[1],
        }),
        limit: Some(2),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(ids(&scanned), vec!["middle", "compact"]);
    let scanned = scan(StorageBranchScan {
        start: "leaf".to_string(),
        stop_at_id: Some("leaf".to_string()),
        scan_type: Some(EntryType::Custom),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(ids(&scanned), Vec::<String>::new());
    let scanned = scan(StorageBranchScan {
        start: "leaf".to_string(),
        custom_type: Some("note".to_string()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(ids(&scanned), vec!["note"]);
    // Unknown start rejects.
    assert!(scan(StorageBranchScan {
        start: "missing".to_string(),
        ..Default::default()
    })
    .await
    .is_err());
}

/// usage and stats / "keeps stats equal to message count and ledger totals"
/// (`conformance/storage.ts:824-872`), including the optional-field sum.
#[tokio::test]
async fn conformance_usage_stats_match_ledger_totals() {
    let fixture = fixture().await;
    let storage = &fixture.storage;
    assert_eq!(
        storage.get_stats(background_context()).await.unwrap(),
        crate::agent_core::harness::session::types::SessionStats {
            message_count: 0,
            usage: zero_usage(),
        }
    );

    let mut first_usage = usage(2, 3);
    first_usage.cache_write_1h = Some(4);
    first_usage.reasoning = Some(1);
    let first = storage
        .commit(
            vec![
                user_entry("message", None, "message"),
                insert_usage(NewUsageRow {
                    id: "usage-1".to_string(),
                    usage: first_usage,
                    entry_id: None,
                    adjustment: false,
                    details: None,
                }),
            ],
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(first.stats.usage, first_usage);
    assert_eq!(first.stats.message_count, 1);

    let mut second_usage = usage(5, 7);
    second_usage.cache_write_1h = Some(6);
    second_usage.reasoning = Some(2);
    let second = storage
        .commit(
            vec![
                custom_entry("custom", Some("message"), "note"),
                compaction_entry("compaction", Some("custom")),
                insert_usage(NewUsageRow {
                    id: "usage-2".to_string(),
                    usage: second_usage,
                    entry_id: None,
                    adjustment: true,
                    details: None,
                }),
            ],
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(second.stats.message_count, 1);
    assert_eq!(second.stats.usage.input, 7);
    assert_eq!(second.stats.usage.output, 10);
    assert_eq!(second.stats.usage.cache_read, 9);
    assert_eq!(second.stats.usage.cache_write, 12);
    assert_eq!(second.stats.usage.cache_write_1h, Some(10));
    assert_eq!(second.stats.usage.reasoning, Some(3));
    assert_eq!(second.stats.usage.total_tokens, 17);
    assert!(
        (second.stats.usage.cost.total - (first_usage.cost.total + second_usage.cost.total)).abs()
            < 1e-9
    );
}

/// serialization / "serializes back-to-back commits in admission order".
#[tokio::test]
async fn conformance_serialization_admission_order() {
    let fixture = fixture().await;
    let storage = Arc::new(&fixture.storage);
    let first = storage.commit(
        vec![user_entry("first", None, "first")],
        background_context(),
    );
    let second = storage.commit(
        vec![user_entry("second", Some("first"), "second")],
        background_context(),
    );
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    assert!(first.seqs[0] < second.seqs[0]);
    assert_eq!(first.stats.message_count, 1);
    assert_eq!(second.stats.message_count, 2);
    let entries = storage
        .scan_entries(&Default::default(), background_context())
        .await
        .unwrap();
    assert_eq!(ids(&entries), vec!["first", "second"]);
}

/// lifecycle / "seals admission, drains admitted commits, and closes
/// idempotently".
#[tokio::test]
async fn conformance_lifecycle_seal_drain_close() {
    let fixture = fixture().await;
    let storage = Arc::new(&fixture.storage);
    let admitted = storage.commit(
        vec![user_entry("admitted", None, "admitted")],
        background_context(),
    );
    let first_close = storage.close(background_context());
    let second_close = storage.close(background_context());

    assert_eq!(admitted.await.unwrap().seqs.len(), 1);
    // Await both closes (Rust futures are lazy, unlike the upstream
    // close() that sets "closing" synchronously).
    first_close.await.unwrap();
    second_close.await.unwrap();
    // Reads reject after close drains admitted commits.
    assert!(storage
        .commit(Vec::new(), background_context())
        .await
        .is_err());
    assert!(storage.get_stats(background_context()).await.is_err());

    assert!(storage
        .get_entries(&[], background_context())
        .await
        .is_err());
    assert!(storage
        .get_value(&session_name(), background_context())
        .await
        .is_err());
    assert!(storage
        .scan_values(&session_name(), background_context())
        .await
        .is_err());
    assert!(storage
        .read_list(&the_list("events"), None, background_context())
        .await
        .is_err());
    assert!(storage
        .scan_branch(
            &StorageBranchScan {
                start: "admitted".to_string(),
                ..Default::default()
            },
            background_context()
        )
        .await
        .is_err());
    assert!(storage
        .scan_entries(&Default::default(), background_context())
        .await
        .is_err());
    assert!(storage
        .scan_usage(&Default::default(), background_context())
        .await
        .is_err());
    assert!(storage.get_stats(background_context()).await.is_err());
}

// Keep helper imports referenced across platform cfgs.
#[allow(unused)]
fn _keepers(_e: Option<AgentMessage>, _u: Option<UserMessage>, _s: Option<StringOrBlocks>) {}
