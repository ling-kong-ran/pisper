//! Wire-format round-trip tests for the committed-write envelope — the
//! byte-format oracle fixtures from
//! `packages/agent/test/harness/jsonl-storage.test.ts` ("writes one line per
//! transaction...") and `jsonl-v3-migration.test.ts` ("converts to v4 and
//! preserves the first caller transaction...").

use super::*;
use crate::agent_core::harness::session::types::NewEntry;
use crate::agent_core::types::AgentMessage;
use crate::ai::types::message::StringOrBlocks;
use crate::ai::types::message::UserMessage;

const NOW: i64 = 1_700_000_000_000;

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.to_string()),
        timestamp: 1,
    })
}

/// The torn-tail fixture write of `jsonl-storage.test.ts:139-153`
/// (byte-format envelope: `kind` + entry fields + `seq` + `timestamp`).
#[test]
fn entry_write_serializes_upstream_wire() {
    let entry = NewEntry::Message {
        id: "torn".to_string(),
        parent_id: None,
        message: user_message("torn"),
        terminate: None,
    };
    let committed = commit_write(insert_entry(entry), 2, NOW);
    let json = serde_json::to_value(&committed).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "kind": "entry",
            "id": "torn",
            "parentId": null,
            "type": "message",
            "message": { "role": "user", "content": "torn", "timestamp": 1 },
            "seq": 2,
            "timestamp": NOW,
        })
    );
    // Round-trips through the parser unchanged.
    let parsed: CommittedWrite = serde_json::from_value(json).unwrap();
    assert_eq!(parsed, committed);
}

/// The `jsonl-v3-migration.test.ts:640-650` first-upgrade transaction line:
/// a usage adjustment followed by a value set, as an array of two writes.
#[test]
fn upgrade_transaction_serializes_upstream_wire() {
    let writes = vec![
        CommittedWrite::Usage {
            row: super::super::types::UsageRow {
                id: "018f6a14-1a2b-7c30-9a2b-3c4d5e6f7081".to_string(),
                seq: 1,
                usage: crate::ai::types::primitives::Usage::default(),
                entry_id: None,
                adjustment: true,
                details: Some(serde_json::json!({ "source": "v3-import" })),
            },
        },
        CommittedWrite::Value(CommittedValueWrite::Set {
            seq: 2,
            namespace: "pi.session.name".to_string(),
            key: String::new(),
            value: serde_json::json!("Converted session"),
        }),
    ];
    let json = serde_json::to_value(&writes).unwrap();
    assert_eq!(
        json,
        serde_json::json!([
            {
                "kind": "usage",
                "id": "018f6a14-1a2b-7c30-9a2b-3c4d5e6f7081",
                "seq": 1,
                "usage": {
                    "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": 0,
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
                },
                "adjustment": true,
                "details": { "source": "v3-import" },
            },
            {
                "kind": "value",
                "op": "set",
                "seq": 2,
                "namespace": "pi.session.name",
                "key": "",
                "value": "Converted session",
            },
        ])
    );
    let parsed: Vec<CommittedWrite> = serde_json::from_value(json).unwrap();
    assert_eq!(parsed, writes);
}

/// The `jsonl-storage.test.ts:167-186` torn array line: entry + value +
/// list writes, each in one transaction object.
#[test]
fn list_and_value_write_shapes_round_trip() {
    let writes = vec![
        CommittedWrite::Entry {
            entry: Box::new(crate::agent_core::harness::session::types::Entry::Message {
                id: "torn-a".to_string(),
                parent_id: None,
                seq: 2,
                timestamp: NOW,
                message: user_message("torn-a"),
                terminate: None,
            }),
        },
        CommittedWrite::Value(CommittedValueWrite::Set {
            seq: 3,
            namespace: "pi.session.name".to_string(),
            key: String::new(),
            value: serde_json::json!("lost"),
        }),
        CommittedWrite::List(CommittedListWrite::Append {
            seq: 4,
            namespace: "test.events".to_string(),
            key: String::new(),
            value: serde_json::json!("lost"),
        }),
        CommittedWrite::List(CommittedListWrite::Delete {
            seq: 5,
            namespace: "test.events".to_string(),
            key: String::new(),
        }),
    ];
    for write in &writes {
        let json = serde_json::to_value(write).unwrap();
        let parsed: CommittedWrite = serde_json::from_value(json).unwrap();
        assert_eq!(&parsed, write);
    }
    let value_json = serde_json::to_value(&writes[1]).unwrap();
    assert_eq!(
        value_json,
        serde_json::json!({
            "kind": "value", "op": "set", "seq": 3,
            "namespace": "pi.session.name", "key": "", "value": "lost",
        })
    );
    let delete_json = serde_json::to_value(&writes[3]).unwrap();
    assert_eq!(
        delete_json,
        serde_json::json!({
            "kind": "list", "op": "delete", "seq": 5,
            "namespace": "test.events", "key": "",
        })
    );
}

/// `commit.ts` sequence stamping: `prepareStorageCommit` assigns
/// consecutive sequences from `firstSeq`.
#[test]
fn prepare_storage_commit_stamps_sequences() {
    let staged = vec![
        insert_entry(NewEntry::Message {
            id: "root".to_string(),
            parent_id: None,
            message: user_message("hello"),
            terminate: None,
        }),
        super::super::values::set_value(
            &crate::agent_core::harness::session::values::branch_tip("main"),
            serde_json::Value::String("root".to_string()),
        ),
    ];
    let prepared = prepare_storage_commit(staged, 10, 99);
    assert_eq!(prepared.result.first_seq, 10);
    assert_eq!(prepared.result.seqs, vec![10, 11]);
    assert_eq!(prepared.result.timestamp, 99);
    assert_eq!(prepared.writes[0].seq(), 10);
    assert_eq!(prepared.writes[1].seq(), 11);
}

/// `commit.ts:90-116` validation: monotonic sequences, shared entry/usage
/// id namespace, parents from prior entries or earlier writes only.
#[test]
fn validates_duplicates_and_missing_parents() {
    use std::collections::HashMap;
    struct State {
        entries: HashMap<String, ()>,
        usage: HashMap<String, ()>,
    }
    impl CommitValidationState for State {
        fn has_entry_or_usage_id(&self, id: &str) -> bool {
            self.entries.contains_key(id) || self.usage.contains_key(id)
        }
        fn has_entry_id(&self, id: &str) -> bool {
            self.entries.contains_key(id)
        }
    }
    let custom = |seq: i64, id: &str, parent: Option<&str>| CommittedWrite::Entry {
        entry: Box::new(crate::agent_core::harness::session::types::Entry::Custom {
            id: id.to_string(),
            parent_id: parent.map(str::to_string),
            seq,
            timestamp: 0,
            custom_type: "note".to_string(),
            data: None,
        }),
    };
    let usage = |seq: i64, id: &str| CommittedWrite::Usage {
        row: super::super::types::UsageRow {
            id: id.to_string(),
            seq,
            usage: crate::ai::types::primitives::Usage::default(),
            entry_id: None,
            adjustment: false,
            details: None,
        },
    };
    let state = State {
        entries: HashMap::from([("existing-entry".to_string(), ())]),
        usage: HashMap::from([("existing-usage".to_string(), ())]),
    };
    // Usage id colliding with an entry id.
    assert!(validate_committed_writes(&[usage(1, "existing-entry")], 1, &state).is_err());
    // Entry id colliding with a usage id.
    assert!(validate_committed_writes(&[custom(1, "existing-usage", None)], 1, &state).is_err());
    // Same-transaction duplicates, both orders.
    assert!(validate_committed_writes(&[custom(1, "x", None), usage(2, "x")], 1, &state).is_err());
    assert!(validate_committed_writes(&[usage(1, "y"), custom(2, "y", None)], 1, &state).is_err());
    // Missing parent.
    assert!(validate_committed_writes(&[custom(1, "orphan", Some("missing"))], 1, &state).is_err());
    // Parent resolved from an earlier write in the same transaction.
    assert!(validate_committed_writes(
        &[
            custom(1, "parent", None),
            custom(2, "child", Some("parent"))
        ],
        1,
        &state
    )
    .is_ok());
    // Usage rows are not entry parents.
    assert!(
        validate_committed_writes(&[usage(1, "u"), custom(2, "c", Some("u"))], 1, &state).is_err()
    );
    // Non-monotonic sequences reject.
    assert!(validate_committed_writes(
        &[custom(2, "a", None), custom(2, "b", Some("a"))],
        1,
        &state
    )
    .is_err());
}
