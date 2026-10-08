//! Port of `src/harness/task-graph.ts` (v1.0.0): the Harness's task-graph
//! view (spec §9.5) — every live task of the Session with its owner edge,
//! status, and owned conversations — advanced from the Session's durable
//! commit publications.
//!
//! Divergences (structural, disclosed):
//! - **D-mount (attachment seam).** Upstream attaches `CommittedStateSource`/
//!   `CommittedWatch` observers to a mount built on the Session line; the
//!   port exposes the same graph value and [`advance`] over one
//!   [`CommitPublication`] so the session layer can drive its existing
//!   observer seams. The wire shape of `TaskGraph` and its update ops are
//!   byte-pinned here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::chord::delta::{Op, Seg};
use crate::durable::types::CommitPublication;
use crate::durable::types::{
    CommitChange, ConversationId, JoinPolicy, TableCommitChange, TaskId, TaskRecord, TaskState,
};

/// A live task's durable status without its checkpoint and outcome payloads
/// (upstream `TaskGraphState`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskGraphState {
    #[serde(rename = "pending")]
    Pending { phase: String },
    #[serde(rename = "running")]
    Running { phase: String },
    Waiting {
        phase: String,
        on: Vec<TaskId>,
        policy: JoinPolicy,
    },
    /// Outcome held until its ordinary owned work drains.
    #[serde(rename = "completing")]
    Completing { outcome: String },
}

/// One node of the task graph (upstream `TaskGraphNode`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGraphNode {
    pub id: TaskId,
    pub kind: String,
    pub conversation_id: ConversationId,
    /// Owner task; absent for a conversation-owned task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<TaskId>,
    pub background: bool,
    pub abort_requested: bool,
    pub state: TaskGraphState,
    /// Conversations this task owns, in ID order.
    pub conversations: Vec<ConversationId>,
}

/// Every live task of the Session (upstream `TaskGraph`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGraph {
    /// Every live task, keyed by its decimal ID.
    pub tasks: BTreeMap<String, TaskGraphNode>,
}

/// `nodeOf(record, conversations)`.
pub fn node_of(record: &TaskRecord, conversations: &[ConversationId]) -> TaskGraphNode {
    TaskGraphNode {
        id: record.id,
        kind: record.kind.clone(),
        conversation_id: record.conversation_id,
        owner: record.owner,
        background: record.background,
        abort_requested: record.abort_requested,
        state: state_of(record),
        conversations: conversations.to_vec(),
    }
}

/// `stateOf(record)`. Terminal records never reach the graph.
fn state_of(record: &TaskRecord) -> TaskGraphState {
    match &record.state {
        TaskState::Pending { checkpoint } => TaskGraphState::Pending {
            phase: phase_of(checkpoint),
        },
        TaskState::Running { checkpoint } => TaskGraphState::Running {
            phase: phase_of(checkpoint),
        },
        TaskState::Waiting {
            checkpoint,
            on,
            policy,
        } => TaskGraphState::Waiting {
            phase: phase_of(checkpoint),
            on: on.clone(),
            policy: *policy,
        },
        TaskState::Completing { outcome } => TaskGraphState::Completing {
            outcome: outcome_status(outcome).to_string(),
        },
        // Terminal records never reach here; mirror `completing` for the
        // erased shape if one arrives through a race.
        TaskState::Terminal { outcome } => TaskGraphState::Completing {
            outcome: outcome_status(outcome).to_string(),
        },
    }
}

/// The `status` discriminant of a `TaskOutcome` (upstream reads the union's
/// `status` tag).
fn outcome_status(outcome: &crate::durable::types::TaskOutcome) -> &'static str {
    use crate::durable::types::TaskOutcome;
    match outcome {
        TaskOutcome::Completed { .. } => "completed",
        TaskOutcome::Failed { .. } => "failed",
        TaskOutcome::Aborted { .. } => "aborted",
        TaskOutcome::Orphaned { .. } => "orphaned",
        TaskOutcome::Faulted { .. } => "faulted",
    }
}

/// `phaseOf(checkpoint)`: the `phase` property of the raw checkpoint.
fn phase_of(checkpoint: &Value) -> String {
    checkpoint
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Wire key of a graph node (`["tasks", String(id)]`).
pub fn node_path(id: TaskId) -> Vec<Value> {
    vec![json!("tasks"), json!(id.to_string())]
}

/// `advance(mount, publication, context)`: derive the mount's operations from
/// one publication, apply them to `graph`, and return the ops (empty when
/// nothing changed). Task changes come first, then conversation-owner edges,
/// so a conversation created with its owner task in one commit finds the
/// owner's node. Ops follow the chord `["s", path, value]` / `["d", path]`
/// wire.
pub fn advance(graph: &mut TaskGraph, publication: &CommitPublication) -> Vec<Op> {
    let mut ops: Vec<Op> = Vec::new();
    // Nodes this publication set or deleted, over the mount's value.
    let mut changed: BTreeMap<String, Option<TaskGraphNode>> = BTreeMap::new();
    fn node(
        changed: &BTreeMap<String, Option<TaskGraphNode>>,
        graph: &TaskGraph,
        key: &str,
    ) -> Option<TaskGraphNode> {
        match changed.get(key) {
            Some(node) => node.clone(),
            None => graph.tasks.get(key).cloned(),
        }
    }
    for change in &publication.changes {
        let CommitChange::Table(TableCommitChange::Task { value: record }) = change else {
            continue;
        };
        let key = record.id.to_string();
        let previous = node(&changed, graph, &key);
        if matches!(record.state, TaskState::Terminal { .. }) {
            if previous.is_none() {
                continue;
            }
            ops.push(Op::Delete {
                path: vec![Seg::Key("tasks".into()), Seg::Key(key.clone())],
            });
            changed.insert(key, None);
            continue;
        }
        let next = node_of(
            record,
            &previous
                .as_ref()
                .map(|node| node.conversations.clone())
                .unwrap_or_default(),
        );
        let next_json = serde_json::to_value(&next).unwrap_or(Value::Null);
        let previous_json = previous
            .as_ref()
            .and_then(|node| serde_json::to_value(node).ok())
            .unwrap_or(Value::Null);
        if previous.is_some() && next_json == previous_json {
            continue;
        }
        ops.push(Op::Set {
            path: vec![Seg::Key("tasks".into()), Seg::Key(key.clone())],
            value: next_json,
        });
        changed.insert(key, Some(next));
    }
    // After the tasks, so a conversation created with its owner task in one
    // commit finds the owner's node. Change order within a publication is
    // unspecified, so each owner's list is sorted again.
    let mut created: BTreeMap<String, Vec<ConversationId>> = BTreeMap::new();
    for change in &publication.changes {
        let CommitChange::Table(TableCommitChange::Conversation {
            value: conversation,
        }) = change
        else {
            continue;
        };
        let Some(owner) = conversation.owner.as_ref() else {
            continue;
        };
        let key = owner.task_id.to_string();
        if node(&changed, graph, &key).is_some() {
            created.entry(key).or_default().push(conversation.id);
        }
    }
    for (key, mut ids) in created {
        ids.sort_unstable();
        let mut conversations = node(&changed, graph, &key)
            .map(|node| node.conversations)
            .unwrap_or_default();
        conversations.extend(ids);
        conversations.sort_unstable();
        // The op applies to the mount's value (upstream
        // `applyImmutable(mount.value, ops)`): when the node came from the
        // mount rather than this publication, update it directly.
        if let Some(Some(node)) = changed.get_mut(&key) {
            node.conversations = conversations.clone();
        } else if let Some(node) = graph.tasks.get_mut(&key) {
            node.conversations = conversations.clone();
        }
        ops.push(Op::Set {
            path: vec![
                Seg::Key("tasks".into()),
                Seg::Key(key),
                Seg::Key("conversations".into()),
            ],
            value: serde_json::to_value(conversations).unwrap_or(Value::Null),
        });
    }
    for (key, node) in changed {
        match node {
            Some(node) => {
                graph.tasks.insert(key, node);
            }
            None => {
                graph.tasks.remove(&key);
            }
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::types::TaskOutcome;

    fn record(id: TaskId, state: TaskState) -> TaskRecord {
        TaskRecord {
            id,
            conversation_id: 1,
            kind: "pi.generation".into(),
            version: 1,
            input: Value::Null,
            owner: None,
            background: false,
            abort_requested: false,
            state,
            memos: None,
        }
    }

    fn publication(changes: Vec<CommitChange>) -> CommitPublication {
        CommitPublication { seq: 1, changes }
    }

    fn task_change(record: TaskRecord) -> CommitChange {
        CommitChange::Table(TableCommitChange::Task { value: record })
    }

    #[test]
    fn nodes_carry_phase_and_wait_state() {
        let node = node_of(
            &record(
                4,
                TaskState::Pending {
                    checkpoint: json!({"phase": "prepare"}),
                },
            ),
            &[],
        );
        assert_eq!(
            node.state,
            TaskGraphState::Pending {
                phase: "prepare".into()
            }
        );
        let node = node_of(
            &record(
                5,
                TaskState::Waiting {
                    checkpoint: json!({"phase": "run"}),
                    on: vec![4],
                    policy: JoinPolicy::AllSettled,
                },
            ),
            &[],
        );
        assert_eq!(
            node.state,
            TaskGraphState::Waiting {
                phase: "run".into(),
                on: vec![4],
                policy: JoinPolicy::AllSettled,
            }
        );
        let node = node_of(
            &record(
                6,
                TaskState::Completing {
                    outcome: TaskOutcome::Aborted {
                        reason: None,
                        result: None,
                    },
                },
            ),
            &[],
        );
        assert_eq!(
            node.state,
            TaskGraphState::Completing {
                outcome: "aborted".into()
            }
        );
    }

    #[test]
    fn advance_sets_updates_and_deletes_nodes() {
        let mut graph = TaskGraph::default();
        let ops = advance(
            &mut graph,
            &publication(vec![task_change(record(
                4,
                TaskState::Running {
                    checkpoint: json!({"phase": "stream"}),
                },
            ))]),
        );
        assert_eq!(ops.len(), 1);
        assert!(graph.tasks.contains_key("4"));
        // An unchanged node emits no op.
        let ops = advance(
            &mut graph,
            &publication(vec![task_change(record(
                4,
                TaskState::Running {
                    checkpoint: json!({"phase": "stream"}),
                },
            ))]),
        );
        assert!(ops.is_empty());
        // Terminal leaves the graph.
        let ops = advance(
            &mut graph,
            &publication(vec![task_change(record(
                4,
                TaskState::Terminal {
                    outcome: TaskOutcome::Completed {
                        result: Value::Null,
                    },
                },
            ))]),
        );
        assert_eq!(ops.len(), 1);
        assert!(graph.tasks.is_empty());
        // Deleting an unknown terminal node emits nothing.
        let ops = advance(
            &mut graph,
            &publication(vec![task_change(record(
                4,
                TaskState::Terminal {
                    outcome: TaskOutcome::Completed {
                        result: Value::Null,
                    },
                },
            ))]),
        );
        assert!(ops.is_empty());
    }

    #[test]
    fn advance_appends_owned_conversations_sorted() {
        let mut graph = TaskGraph::default();
        advance(
            &mut graph,
            &publication(vec![task_change(record(
                4,
                TaskState::Running {
                    checkpoint: json!({"phase": "stream"}),
                },
            ))]),
        );
        let conversation = |id: ConversationId, owner: TaskId| {
            CommitChange::Table(TableCommitChange::Conversation {
                value: crate::durable::types::ConversationRecord {
                    id,
                    parent: None,
                    owner: Some(crate::durable::types::ConversationOwner {
                        conversation_id: 0,
                        task_id: owner,
                    }),
                },
            })
        };
        let ops = advance(
            &mut graph,
            &publication(vec![conversation(9, 4), conversation(7, 4)]),
        );
        // One op per owner, not per conversation.
        assert_eq!(ops.len(), 1);
        assert_eq!(graph.tasks["4"].conversations, vec![7, 9]);
    }
}
