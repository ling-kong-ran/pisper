//! Port of `packages/agent/test/harness/jsonl-storage.test.ts` (248 lines):
//! persistence/replay semantics and the torn-tail contract, byte-format
//! pinned.

use super::super::storage::JsonlStorage;
use super::super::types::{JsonlStorageHeader, JSONL_STORAGE_VERSION};
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::env::NodeExecutionEnv;
use crate::agent_core::harness::session::commit::{insert_entry, insert_usage};
use crate::agent_core::harness::session::types::{NewEntry, NewUsageRow, Storage, Write};
use crate::agent_core::harness::session::values::{
    append_list, branch_tip, delete_list, list, session_name, set_value,
};
use crate::agent_core::harness::types::FileSystem;
use crate::agent_core::types::AgentMessage;
use crate::ai::types::message::{StringOrBlocks, UserMessage};
use crate::ai::types::primitives::Usage;
use std::sync::Arc;

const NOW: i64 = 1_700_000_000_000;

fn user_message(text: &str, timestamp: i64) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.to_string()),
        timestamp,
    })
}

fn message_entry(id: &str, parent_id: Option<&str>, text: &str) -> Write {
    insert_entry(NewEntry::Message {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        message: user_message(text, 1),
        terminate: None,
    })
}

fn header(id: &str) -> JsonlStorageHeader {
    JsonlStorageHeader::new(id, JSONL_STORAGE_VERSION, NOW, "/workspace")
}

fn make_env(dir: &tempfile::TempDir) -> Arc<NodeExecutionEnv> {
    Arc::new(NodeExecutionEnv::new(
        dir.path().to_string_lossy().to_string(),
    ))
}

fn usage_row(id: &str, entry_id: Option<&str>) -> NewUsageRow {
    NewUsageRow {
        id: id.to_string(),
        usage: Usage {
            input: 1,
            output: 2,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 3,
            cost: Default::default(),
        },
        entry_id: entry_id.map(str::to_string),
        adjustment: false,
        details: None,
    }
}

/// "replays whole-list deletion without resurrecting earlier appends"
/// (`jsonl-storage.test.ts:24-42`).
#[tokio::test]
async fn replays_whole_list_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let file_system = make_env(&dir);
    let options = || crate::agent_core::harness::session::jsonl::types::JsonlStorageOptions {
        file_system: Arc::clone(&file_system) as _,
        path: "list-delete.jsonl".to_string(),
        now: Some(Arc::new(|| NOW)),
    };
    let events = list("test.events", "");
    let storage = JsonlStorage::create(
        options(),
        header("list-delete"),
        Vec::new(),
        background_context(),
    )
    .await
    .unwrap();
    storage
        .commit(
            vec![
                append_list(&events, serde_json::json!("first")),
                append_list(&events, serde_json::json!("second")),
            ],
            background_context(),
        )
        .await
        .unwrap();
    storage
        .commit(vec![delete_list(&events)], background_context())
        .await
        .unwrap();
    storage.close(background_context()).await.unwrap();

    let reopened = JsonlStorage::open(options(), background_context())
        .await
        .unwrap();
    assert!(reopened
        .read_list(&events, None, background_context())
        .await
        .unwrap()
        .is_empty());
    let recreated = reopened
        .commit(
            vec![append_list(&events, serde_json::json!("after"))],
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(recreated.first_seq, 4);
    assert_eq!(
        reopened
            .read_list(&events, None, background_context())
            .await
            .unwrap(),
        vec![crate::agent_core::harness::session::values::ListElement {
            seq: 4,
            value: serde_json::json!("after"),
        }]
    );
    reopened.close(background_context()).await.unwrap();
}

/// "writes one line per transaction and replays stamped state"
/// (`jsonl-storage.test.ts:44-116`), including the byte-format header line
/// and the single-write-object vs array framing.
#[tokio::test]
async fn writes_one_line_per_transaction_and_replays() {
    let dir = tempfile::tempdir().unwrap();
    let file_system = make_env(&dir);
    let options = || crate::agent_core::harness::session::jsonl::types::JsonlStorageOptions {
        file_system: Arc::clone(&file_system) as _,
        path: "session.jsonl".to_string(),
        now: Some(Arc::new(|| NOW)),
    };
    let storage = JsonlStorage::create(
        options(),
        header("round-trip"),
        Vec::new(),
        background_context(),
    )
    .await
    .unwrap();
    let committed = storage
        .commit(
            vec![
                message_entry("root", None, "hello"),
                set_value(&branch_tip("main"), serde_json::json!("root")),
                insert_usage(usage_row("usage", Some("root"))),
            ],
            background_context(),
        )
        .await
        .unwrap();
    storage
        .commit(
            vec![set_value(&session_name(), serde_json::json!("name"))],
            background_context(),
        )
        .await
        .unwrap();

    let content = file_system
        .read_text_file("session.jsonl", background_context())
        .await
        .unwrap();
    let lines: Vec<&str> = content.trim_end().split('\n').collect();
    // Byte-format: header parses to the exact header record.
    let parsed_header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(
        parsed_header,
        serde_json::json!({
            "v": 4,
            "kind": "header",
            "id": "round-trip",
            "storageVersion": 1,
            "createdAt": NOW,
            "cwd": "/workspace",
        })
    );
    // Three writes serialize as one array line; one write as a bare object.
    let first_transaction: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(first_transaction.as_array().unwrap().len(), 3);
    let second_transaction: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
    assert!(second_transaction.is_object());
    storage.close(background_context()).await.unwrap();

    let reopened = JsonlStorage::open(options(), background_context())
        .await
        .unwrap();
    let entries = reopened
        .get_entries(&["root".to_string()], background_context())
        .await
        .unwrap();
    let entry = entries.get("root").unwrap();
    assert_eq!(entry.seq(), committed.seqs[0]);
    assert_eq!(entry.timestamp(), committed.timestamp);
    assert_eq!(entry.parent_id(), None);
    assert_eq!(entry.message().map(AgentMessage::role), Some("user"));

    let stored = reopened
        .get_value(&branch_tip("main"), background_context())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.value, serde_json::json!("root"));
    assert_eq!(stored.seq, committed.seqs[1]);

    let usage = reopened
        .scan_usage(&Default::default(), background_context())
        .await
        .unwrap();
    assert_eq!(
        usage
            .iter()
            .map(|row| (row.id.as_str(), row.seq))
            .collect::<Vec<_>>(),
        vec![("usage", committed.seqs[2])]
    );
    let stats = reopened.get_stats(background_context()).await.unwrap();
    assert_eq!(stats.message_count, 1);
    assert_eq!(stats.usage.input, 1);
    assert_eq!(stats.usage.output, 2);
    assert_eq!(stats.usage.total_tokens, 3);

    let next = reopened
        .commit(Vec::new(), background_context())
        .await
        .unwrap();
    assert_eq!(next.first_seq, 5);
    assert_eq!(next.stats, stats);
    reopened.close(background_context()).await.unwrap();
}

/// The torn-tail suite (`jsonl-storage.test.ts:119-248`).
mod torn {
    use super::*;

    async fn seed() -> (tempfile::TempDir, Arc<NodeExecutionEnv>, String) {
        let dir = tempfile::tempdir().unwrap();
        let file_system = make_env(&dir);
        let storage = JsonlStorage::create(
            crate::agent_core::harness::session::jsonl::types::JsonlStorageOptions {
                file_system: Arc::clone(&file_system) as _,
                path: "session.jsonl".to_string(),
                now: Some(Arc::new(|| NOW)),
            },
            header("torn"),
            Vec::new(),
            background_context(),
        )
        .await
        .unwrap();
        storage
            .commit(
                vec![message_entry("kept", None, "kept")],
                background_context(),
            )
            .await
            .unwrap();
        storage.close(background_context()).await.unwrap();
        let prefix = file_system
            .read_text_file("session.jsonl", background_context())
            .await
            .unwrap();
        (dir, file_system, prefix)
    }

    fn options(
        file_system: &Arc<NodeExecutionEnv>,
    ) -> crate::agent_core::harness::session::jsonl::types::JsonlStorageOptions {
        crate::agent_core::harness::session::jsonl::types::JsonlStorageOptions {
            file_system: Arc::clone(file_system) as _,
            path: "session.jsonl".to_string(),
            now: Some(Arc::new(|| NOW)),
        }
    }

    /// "discards an unterminated final object line and truncates before
    /// admitting writes" (`jsonl-storage.test.ts:139-165`).
    #[tokio::test]
    async fn discards_unterminated_final_object_line() {
        let (_dir, file_system, prefix) = seed().await;
        file_system
            .append_file(
                "session.jsonl",
                crate::agent_core::harness::types::FileContent::Text(
                    r#"{"kind":"entry","id":"torn","parentId":null,"type":"message","message":{"role":"user","content":"torn","timestamp":1},"seq":2,"timestamp":1700000000000}"#.to_string(),
                ),
                background_context(),
            )
            .await
            .unwrap();

        let reopened = JsonlStorage::open(options(&file_system), background_context())
            .await
            .unwrap();
        assert!(!reopened
            .get_entries(&["torn".to_string()], background_context())
            .await
            .unwrap()
            .contains_key("torn"));
        assert!(reopened
            .get_entries(&["kept".to_string()], background_context())
            .await
            .unwrap()
            .contains_key("kept"));
        assert_eq!(
            file_system
                .read_text_file("session.jsonl", background_context())
                .await
                .unwrap(),
            prefix
        );
        assert!(!file_system
            .exists("session.jsonl.tmp", background_context())
            .await
            .unwrap());

        let next = reopened
            .commit(
                vec![message_entry("after", None, "after")],
                background_context(),
            )
            .await
            .unwrap();
        assert_eq!(next.first_seq, 2);
        assert_eq!(
            reopened
                .get_entries(&["after".to_string()], background_context())
                .await
                .unwrap()
                .get("after")
                .unwrap()
                .seq(),
            2
        );
        reopened.close(background_context()).await.unwrap();
    }

    /// "discards a torn array line wholly, including list elements"
    /// (`jsonl-storage.test.ts:167-194`).
    #[tokio::test]
    async fn discards_torn_array_line_wholly() {
        let (_dir, file_system, prefix) = seed().await;
        let events = list("test.events", "");
        file_system
            .append_file(
                "session.jsonl",
                crate::agent_core::harness::types::FileContent::Text(
                    r#"[{"kind":"entry","id":"torn-a","parentId":null,"type":"message","message":{"role":"user","content":"torn-a","timestamp":1},"seq":2,"timestamp":1700000000000},{"kind":"value","op":"set","seq":3,"namespace":"pi.session.name","key":"","value":"lost"},{"kind":"list","op":"append","seq":4,"namespace":"test.events","key":"","value":"lost"}]"#.to_string(),
                ),
                background_context(),
            )
            .await
            .unwrap();

        let reopened = JsonlStorage::open(options(&file_system), background_context())
            .await
            .unwrap();
        assert!(!reopened
            .get_entries(&["torn-a".to_string()], background_context())
            .await
            .unwrap()
            .contains_key("torn-a"));
        assert!(reopened
            .get_value(&session_name(), background_context())
            .await
            .unwrap()
            .is_none());
        assert!(reopened
            .read_list(&events, None, background_context())
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            file_system
                .read_text_file("session.jsonl", background_context())
                .await
                .unwrap(),
            prefix
        );
        reopened.close(background_context()).await.unwrap();
    }

    /// "rejects a malformed interior line without rewriting"
    /// (`jsonl-storage.test.ts:196-221`), including the pre-WP01 "register"
    /// record spelling.
    #[tokio::test]
    async fn rejects_malformed_interior_line_without_rewriting() {
        let (_dir, file_system, prefix) = seed().await;
        let corrupted = format!("{prefix}not-json\n");
        file_system
            .write_file(
                "session.jsonl",
                crate::agent_core::harness::types::FileContent::Text(corrupted.clone()),
                background_context(),
            )
            .await
            .unwrap();

        let error = JsonlStorage::open(options(&file_system), background_context())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("line 3"), "{error}");
        assert_eq!(
            file_system
                .read_text_file("session.jsonl", background_context())
                .await
                .unwrap(),
            corrupted
        );
        assert!(!file_system
            .exists("session.jsonl.tmp", background_context())
            .await
            .unwrap());
    }

    /// "rejects the unsupported pre-WP01 scalar record spelling"
    /// (`jsonl-storage.test.ts:206-221`).
    #[tokio::test]
    async fn rejects_legacy_register_record_spelling() {
        let (_dir, file_system, prefix) = seed().await;
        let legacy_kind = format!("{}{}", "reg", "ister");
        let record = serde_json::json!({
            "kind": legacy_kind,
            "op": "set",
            "seq": 2,
            "namespace": "legacy.value",
            "key": "state",
            "value": true,
        });
        let corrupted = format!("{prefix}{record}\n");
        file_system
            .write_file(
                "session.jsonl",
                crate::agent_core::harness::types::FileContent::Text(corrupted.clone()),
                background_context(),
            )
            .await
            .unwrap();

        let error = JsonlStorage::open(options(&file_system), background_context())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("line 3"), "{error}");
        assert_eq!(
            file_system
                .read_text_file("session.jsonl", background_context())
                .await
                .unwrap(),
            corrupted
        );
    }

    /// "rejects a complete malformed final line without rewriting"
    /// (`jsonl-storage.test.ts:223-230`).
    #[tokio::test]
    async fn rejects_complete_malformed_final_line() {
        let (_dir, file_system, prefix) = seed().await;
        let corrupted = format!("{prefix}not-json\n");
        file_system
            .write_file(
                "session.jsonl",
                crate::agent_core::harness::types::FileContent::Text(corrupted.clone()),
                background_context(),
            )
            .await
            .unwrap();

        let error = JsonlStorage::open(options(&file_system), background_context())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("line 3"), "{error}");
        assert_eq!(
            file_system
                .read_text_file("session.jsonl", background_context())
                .await
                .unwrap(),
            corrupted
        );
    }

    /// "rejects a complete final line with invalid transaction framing"
    /// (`jsonl-storage.test.ts:232-239`).
    #[tokio::test]
    async fn rejects_invalid_transaction_framing() {
        let (_dir, file_system, prefix) = seed().await;
        let corrupted = format!("{prefix}{{\"kind\":\"nope\",\"seq\":2}}\n");
        file_system
            .write_file(
                "session.jsonl",
                crate::agent_core::harness::types::FileContent::Text(corrupted.clone()),
                background_context(),
            )
            .await
            .unwrap();

        let error = JsonlStorage::open(options(&file_system), background_context())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("line 3"), "{error}");
        assert_eq!(
            file_system
                .read_text_file("session.jsonl", background_context())
                .await
                .unwrap(),
            corrupted
        );
    }

    /// "rejects an unterminated header" (`jsonl-storage.test.ts:241-248`).
    #[tokio::test]
    async fn rejects_unterminated_header() {
        let dir = tempfile::tempdir().unwrap();
        let file_system = make_env(&dir);
        let full = serde_json::to_string(&header("torn")).unwrap();
        let truncated = &full[..full.len() - 4];
        file_system
            .write_file(
                "session.jsonl",
                crate::agent_core::harness::types::FileContent::Text(truncated.to_string()),
                background_context(),
            )
            .await
            .unwrap();

        let error = JsonlStorage::open(options(&file_system), background_context())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing header"), "{error}");
    }
}

#[tokio::test]
async fn explicit_json_null_survives_entry_and_usage_jsonl_close_and_reopen() {
    use crate::agent_core::harness::session::types::Entry;
    let dir = tempfile::tempdir().unwrap();
    let file_system = make_env(&dir);
    let options = || crate::agent_core::harness::session::jsonl::types::JsonlStorageOptions {
        file_system: file_system.clone(),
        path: "nullable.jsonl".into(),
        now: Some(Arc::new(|| NOW)),
    };
    let storage = JsonlStorage::create(options(), header("nullable"), vec![], background_context())
        .await
        .unwrap();
    let mut usage = usage_row("u", Some("null"));
    usage.details = Some(serde_json::Value::Null);
    storage
        .commit(
            vec![
                insert_entry(NewEntry::Custom {
                    id: "null".into(),
                    parent_id: None,
                    custom_type: "null".into(),
                    data: Some(serde_json::Value::Null),
                }),
                insert_entry(NewEntry::Custom {
                    id: "absent".into(),
                    parent_id: Some("null".into()),
                    custom_type: "absent".into(),
                    data: None,
                }),
                insert_usage(usage),
            ],
            background_context(),
        )
        .await
        .unwrap();
    storage.close(background_context()).await.unwrap();
    let text = file_system
        .read_text_file("nullable.jsonl", background_context())
        .await
        .unwrap();
    assert!(text.contains("\"data\":null"));
    assert!(text.contains("\"details\":null"));
    let reopened = JsonlStorage::open(options(), background_context())
        .await
        .unwrap();
    let entries = reopened
        .get_entries(&["null".into(), "absent".into()], background_context())
        .await
        .unwrap();
    let null = entries.get("null").unwrap();
    assert!(matches!(
        null,
        Entry::Custom {
            data: Some(serde_json::Value::Null),
            ..
        }
    ));
    let absent = entries.get("absent").unwrap();
    assert!(matches!(absent, Entry::Custom { data: None, .. }));
    let rows = reopened
        .scan_usage(&Default::default(), background_context())
        .await
        .unwrap();
    assert_eq!(rows[0].details, Some(serde_json::Value::Null));
    reopened.close(background_context()).await.unwrap();
}
