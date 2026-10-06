//! Port of `packages/agent/test/harness/branch-summarization.test.ts` (the
//! executable spec for `collectEntriesForBranchSummary`): an in-memory
//! branch/session reader walking parent chains exactly like the storage
//! implementations under test upstream.

use std::collections::BTreeMap;

use super::*;
use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::session::types::{BranchReader, BranchScan, Entry, SessionReader};
use crate::agent_core::types::AgentMessage;
use crate::ai::types::message::{Message, StringOrBlocks, UserMessage};
use futures::future::BoxFuture;

/// The oracle's `message` fixture (branch-summarization.test.ts:7-9).
fn message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.to_string()),
        timestamp: 1,
    })
}

/// The oracle's `messageEntry` fixture (branch-summarization.test.ts:11-13).
fn message_entry(id: &str, parent_id: Option<&str>, text: &str, seq: i64) -> Entry {
    Entry::Message {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        seq,
        timestamp: seq,
        message: message(text),
        terminate: None,
    }
}

/// The oracle's `branchReader` fixture (branch-summarization.test.ts:15-40):
/// parent-chain walks over an in-memory id map.
fn branch_reader(entries: Vec<Entry>) -> (impl BranchReader, impl SessionReader) {
    let by_id: BTreeMap<String, Entry> = entries
        .into_iter()
        .map(|entry| (entry.id().to_string(), entry))
        .collect();
    struct TestBranch(BTreeMap<String, Entry>);
    impl BranchReader for TestBranch {
        fn find_entries<'a>(
            &'a self,
            query: Option<&BranchScan>,
            _context: Context,
        ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
            let start = query.and_then(|query| query.start.clone());
            Box::pin(async move {
                let mut path = Vec::new();
                let mut current = start;
                while let Some(id) = current {
                    let entry = self
                        .0
                        .get(&id)
                        .ok_or_else(|| anyhow::anyhow!("Unknown entry {id}"))?;
                    path.push(entry.clone());
                    current = entry.parent_id().map(str::to_string);
                }
                Ok(path)
            })
        }
    }
    struct TestSession(BTreeMap<String, Entry>);
    impl SessionReader for TestSession {
        fn get_entry<'a>(
            &'a self,
            id: &str,
            _context: Context,
        ) -> BoxFuture<'a, anyhow::Result<Option<Entry>>> {
            let id = id.to_string();
            Box::pin(async move { Ok(self.0.get(&id).cloned()) })
        }
    }
    (TestBranch(by_id.clone()), TestSession(by_id))
}

/// Oracle "collects the abandoned side of a branch in chronological order".
#[tokio::test]
async fn collects_the_abandoned_side_of_a_branch_in_chronological_order() {
    let root = message_entry("root", None, "root", 1);
    let common = message_entry("common", Some("root"), "common", 2);
    let abandoned1 = message_entry("abandoned-1", Some("common"), "abandoned 1", 3);
    let abandoned2 = message_entry("abandoned-2", Some("abandoned-1"), "abandoned 2", 4);
    let target = message_entry("target", Some("common"), "target", 5);
    let (branch, session) = branch_reader(vec![
        root.clone(),
        common.clone(),
        abandoned1.clone(),
        abandoned2.clone(),
        target.clone(),
    ]);

    let result = collect_entries_for_branch_summary(
        &branch,
        &session,
        Some("abandoned-2"),
        "target",
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(result.common_ancestor_id.as_deref(), Some("common"));
    assert_eq!(
        result.entries.iter().map(Entry::id).collect::<Vec<_>>(),
        ["abandoned-1", "abandoned-2"]
    );
    assert!(!result.entries.iter().any(|entry| entry.id() == "root"));
    let _: Option<Message> = None;
}

/// Oracle "returns no entries when there was no previous leaf".
#[tokio::test]
async fn returns_no_entries_when_there_was_no_previous_leaf() {
    let target = message_entry("target", None, "target", 1);
    let (branch, session) = branch_reader(vec![target]);
    let result =
        collect_entries_for_branch_summary(&branch, &session, None, "target", background_context())
            .await
            .unwrap();
    assert!(result.entries.is_empty());
    assert_eq!(result.common_ancestor_id, None);
}
