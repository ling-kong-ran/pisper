//! Port of `packages/agent/test/harness/jsonl-io.test.ts` (108 lines): the
//! atomic publication contract and the transaction serialization framing.

use super::{
    parse_jsonl_transaction, publish_file_atomically, publish_jsonl, serialize_jsonl_transaction,
};
use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::env::NodeExecutionEnv;
use crate::agent_core::harness::session::commit::{
    CommittedListWrite, CommittedValueWrite, CommittedWrite,
};
use crate::agent_core::harness::session::jsonl::types::{
    JsonlStorageHeader, JSONL_STORAGE_VERSION,
};
use crate::agent_core::harness::types::{FileContent, FileSystem};
use std::sync::Arc;

const NOW: i64 = 1_700_000_000_000;

fn header(id: &str) -> JsonlStorageHeader {
    JsonlStorageHeader::new(id, JSONL_STORAGE_VERSION, NOW, "/workspace")
}

fn one_write() -> Vec<CommittedWrite> {
    vec![CommittedWrite::Value(CommittedValueWrite::Set {
        seq: 2,
        namespace: "test".to_string(),
        key: String::new(),
        value: serde_json::json!("first"),
    })]
}

/// "keeps the destination unchanged until all content has been written"
/// (`jsonl-io.test.ts:23-45`).
#[tokio::test]
async fn keeps_destination_unchanged_until_rename() {
    let dir = tempfile::tempdir().unwrap();
    let file_system = Arc::new(NodeExecutionEnv::new(
        dir.path().to_string_lossy().to_string(),
    ));
    file_system
        .write_file(
            "session.jsonl",
            FileContent::Text("original".to_string()),
            background_context(),
        )
        .await
        .unwrap();

    publish_file_atomically(
        file_system.as_ref(),
        "session.jsonl",
        background_context(),
        {
            let file_system = Arc::clone(&file_system);
            move |append| async move {
                append.append("first\n").await?;
                // The staging file holds the partial content; the destination
                // is untouched.
                let staged = file_system
                    .read_text_file("session.jsonl.tmp", background_context())
                    .await
                    .unwrap();
                assert_eq!(staged, "first\n");
                let destination = file_system
                    .read_text_file("session.jsonl", background_context())
                    .await
                    .unwrap();
                assert_eq!(destination, "original");
                append.append("second\n").await
            }
        },
    )
    .await
    .unwrap();

    assert_eq!(
        file_system
            .read_text_file("session.jsonl", background_context())
            .await
            .unwrap(),
        "first\nsecond\n"
    );
    assert!(!file_system
        .exists("session.jsonl.tmp", background_context())
        .await
        .unwrap());
}

/// "discards partial content and preserves the original error when the
/// callback fails" (`jsonl-io.test.ts:47-63`).
#[tokio::test]
async fn discards_partial_content_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let file_system = Arc::new(NodeExecutionEnv::new(
        dir.path().to_string_lossy().to_string(),
    ));
    file_system
        .write_file(
            "session.jsonl",
            FileContent::Text("original".to_string()),
            background_context(),
        )
        .await
        .unwrap();

    let result = publish_file_atomically(
        file_system.as_ref(),
        "session.jsonl",
        background_context(),
        |append| async move {
            append.append("partial").await?;
            anyhow::bail!("content generation failed")
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        file_system
            .read_text_file("session.jsonl", background_context())
            .await
            .unwrap(),
        "original"
    );
    assert!(!file_system
        .exists("session.jsonl.tmp", background_context())
        .await
        .unwrap());
}

/// `publishJsonl` writes the header line then one transaction line per
/// append (`jsonl-io.test.ts` + `jsonl-storage.test.ts` framing).
#[tokio::test]
async fn publish_jsonl_streams_header_and_transactions() {
    let dir = tempfile::tempdir().unwrap();
    let file_system = Arc::new(NodeExecutionEnv::new(
        dir.path().to_string_lossy().to_string(),
    ));
    let single = one_write();
    let multi = vec![
        CommittedWrite::Value(CommittedValueWrite::Set {
            seq: 3,
            namespace: "a".to_string(),
            key: String::new(),
            value: serde_json::json!(1),
        }),
        CommittedWrite::List(CommittedListWrite::Append {
            seq: 4,
            namespace: "b".to_string(),
            key: String::new(),
            value: serde_json::json!(2),
        }),
    ];
    publish_jsonl(
        file_system.as_ref(),
        "session.jsonl",
        &header("io"),
        background_context(),
        |append| async move {
            append.append(&single).await?;
            append.append(&multi).await
        },
    )
    .await
    .unwrap();

    let content = file_system
        .read_text_file("session.jsonl", background_context())
        .await
        .unwrap();
    let lines: Vec<&str> = content.trim_end().split('\n').collect();
    assert_eq!(lines.len(), 3);
    let parsed_header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(
        parsed_header,
        serde_json::json!({
            "v": 4, "kind": "header", "id": "io", "storageVersion": 1,
            "createdAt": NOW, "cwd": "/workspace",
        })
    );
    // One-write transactions are bare objects; multi-write are arrays.
    assert!(serde_json::from_str::<serde_json::Value>(lines[1])
        .unwrap()
        .is_object());
    assert!(serde_json::from_str::<serde_json::Value>(lines[2])
        .unwrap()
        .is_array());
}

/// `parseJsonlTransaction` accepts a bare object or an array and validates
/// framing (`io.ts:66-78`).
#[test]
fn parses_single_and_array_transactions() {
    let single = r#"{"kind":"value","op":"set","seq":2,"namespace":"n","key":"","value":true}"#;
    assert_eq!(parse_jsonl_transaction(single).unwrap().len(), 1);
    let array = r#"[{"kind":"value","op":"delete","seq":2,"namespace":"n","key":""},{"kind":"list","op":"append","seq":3,"namespace":"n","key":"","value":1}]"#;
    assert_eq!(parse_jsonl_transaction(array).unwrap().len(), 2);
    // Invalid JSON.
    assert!(parse_jsonl_transaction("not-json").is_err());
    // Invalid framing.
    let error = parse_jsonl_transaction(r#"{"kind":"nope","seq":2}"#).unwrap_err();
    assert!(
        error.to_string().contains("Invalid JSONL write kind: nope"),
        "{error}"
    );
    let error = parse_jsonl_transaction(
        r#"{"kind":"value","op":"replace","seq":2,"namespace":"n","key":""}"#,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Invalid JSONL value operation: replace"),
        "{error}"
    );
    // seq must be a safe integer >= 1.
    assert!(parse_jsonl_transaction(
        r#"{"kind":"value","op":"set","seq":0,"namespace":"n","key":"","value":1}"#
    )
    .is_err());
    // Entry timestamp must be >= 0.
    assert!(parse_jsonl_transaction(
        r#"{"kind":"entry","id":"e","parentId":null,"type":"custom","customType":"x","seq":2,"timestamp":-1}"#
    )
    .is_err());
    // Round-trip through serialize.
    let writes = parse_jsonl_transaction(array).unwrap();
    let serialized = serialize_jsonl_transaction(&writes).unwrap();
    assert_eq!(parse_jsonl_transaction(&serialized).unwrap(), writes);
    let _ = Context::background;
}
