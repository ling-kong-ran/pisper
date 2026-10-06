//! Port of `packages/agent/src/harness/session/commit.ts` (116 lines): the
//! staged-write constructors, the committed-write shapes, storage-level
//! sequence stamping, and the transaction validation the storage backends
//! run before applying anything.
//!
//! Wire format: committed writes are tagged by `kind` (`entry`, `usage`,
//! `value`, `list`); the entry/usage variants flatten their payload fields
//! (upstream `{ kind: "entry", ...entry, seq, timestamp }`), and the
//! value/list variants carry `op` (`set`/`delete`, `append`/`delete`). A
//! one-write transaction serializes as a bare object, a multi-write one as
//! an array — that spelling lives with the jsonl io module
//! ([`super::jsonl::io::serialize_jsonl_transaction`]).

use serde::{Deserialize, Serialize};

use super::types::{CommitResult, Entry, NewEntry, NewUsageRow, UsageRow, Write};

/// Upstream `CommittedEntryWrite` (`commit.ts:3`): an entry write stamped
/// with its sequence and timestamp, tagged `kind: "entry"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommittedWrite {
    /// Upstream `CommittedEntryWrite`. The entry payload is boxed: it is
    /// intrinsically the largest variant (every entry carries the full
    /// transcript record), and the wire shape is unchanged by boxing.
    Entry {
        #[serde(flatten)]
        entry: Box<Entry>,
    },
    /// Upstream `CommittedUsageWrite`.
    Usage {
        #[serde(flatten)]
        row: UsageRow,
    },
    /// Upstream `CommittedValueSetWrite`/`CommittedValueDeleteWrite`
    /// (`commit.ts:5-19`).
    Value(CommittedValueWrite),
    /// Upstream `CommittedListAppendWrite`/`CommittedListDeleteWrite`
    /// (`commit.ts:20-34`).
    List(CommittedListWrite),
}

impl CommittedWrite {
    /// The committed write's sequence number.
    pub fn seq(&self) -> i64 {
        match self {
            CommittedWrite::Entry { entry } => entry.seq(),
            CommittedWrite::Usage { row } => row.seq,
            CommittedWrite::Value(write) => write.seq(),
            CommittedWrite::List(write) => write.seq(),
        }
    }

    /// The id participating in the shared entry/usage id namespace, or `None`
    /// for value/list writes (`commit.ts:101-114`).
    pub fn entry_or_usage_id(&self) -> Option<&str> {
        match self {
            CommittedWrite::Entry { entry } => Some(entry.id()),
            CommittedWrite::Usage { row } => Some(&row.id),
            CommittedWrite::Value(_) | CommittedWrite::List(_) => None,
        }
    }

    /// The parent id of an entry write (`commit.ts:105-112`).
    pub fn entry_parent_id(&self) -> Option<&str> {
        match self {
            CommittedWrite::Entry { entry } => entry.parent_id(),
            _ => None,
        }
    }

    /// Whether this is an entry write.
    pub fn is_entry(&self) -> bool {
        matches!(self, CommittedWrite::Entry { .. })
    }
}

/// Upstream `CommittedValueSetWrite`/`CommittedValueDeleteWrite`, tagged by
/// `op`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum CommittedValueWrite {
    Set {
        seq: i64,
        namespace: String,
        key: String,
        value: serde_json::Value,
    },
    Delete {
        seq: i64,
        namespace: String,
        key: String,
    },
}

impl CommittedValueWrite {
    /// The write's sequence number.
    pub fn seq(&self) -> i64 {
        match self {
            CommittedValueWrite::Set { seq, .. } | CommittedValueWrite::Delete { seq, .. } => *seq,
        }
    }
}

/// Upstream `CommittedListAppendWrite`/`CommittedListDeleteWrite`, tagged by
/// `op`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum CommittedListWrite {
    Append {
        seq: i64,
        namespace: String,
        key: String,
        value: serde_json::Value,
    },
    Delete {
        seq: i64,
        namespace: String,
        key: String,
    },
}

impl CommittedListWrite {
    /// The write's sequence number.
    pub fn seq(&self) -> i64 {
        match self {
            CommittedListWrite::Append { seq, .. } | CommittedListWrite::Delete { seq, .. } => *seq,
        }
    }
}

/// Upstream `PreparedCommit` (`commit.ts:43-46`).
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedCommit {
    pub writes: Vec<CommittedWrite>,
    /// Upstream `Omit<CommitResult, "stats">`.
    pub result: CommitStatsPending,
}

/// Upstream `Omit<CommitResult, "stats">`.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitStatsPending {
    pub first_seq: i64,
    pub seqs: Vec<i64>,
    pub timestamp: i64,
}

impl CommitStatsPending {
    /// Attach the post-apply stats (the `CommitResult` completion).
    pub fn with_stats(self, stats: super::types::SessionStats) -> CommitResult {
        CommitResult {
            first_seq: self.first_seq,
            seqs: self.seqs,
            timestamp: self.timestamp,
            stats,
        }
    }
}

/// Upstream `insertEntry(entry)` (`commit.ts:53-55`).
pub fn insert_entry(entry: NewEntry) -> Write {
    Write::Entry { entry }
}

/// Upstream `insertUsage(row)` (`commit.ts:57-59`).
pub fn insert_usage(row: NewUsageRow) -> Write {
    Write::Usage { row }
}

/// Upstream `commitWrite(write, seq, timestamp)` (`commit.ts:61-76`).
pub fn commit_write(write: Write, seq: i64, timestamp: i64) -> CommittedWrite {
    match write {
        Write::Entry { entry } => CommittedWrite::Entry {
            entry: Box::new(entry.into_entry(seq, timestamp)),
        },
        Write::Usage { row } => CommittedWrite::Usage {
            row: UsageRow {
                seq,
                id: row.id,
                usage: row.usage,
                entry_id: row.entry_id,
                adjustment: row.adjustment,
                details: row.details,
            },
        },
        Write::Value(value_write) => match value_write {
            super::types::ValueWrite::Set {
                namespace,
                key,
                value,
            } => CommittedWrite::Value(CommittedValueWrite::Set {
                seq,
                namespace,
                key,
                value,
            }),
            super::types::ValueWrite::Delete { namespace, key } => {
                CommittedWrite::Value(CommittedValueWrite::Delete {
                    seq,
                    namespace,
                    key,
                })
            }
        },
        Write::List(list_write) => match list_write {
            super::types::ListWrite::Append {
                namespace,
                key,
                value,
            } => CommittedWrite::List(CommittedListWrite::Append {
                seq,
                namespace,
                key,
                value,
            }),
            super::types::ListWrite::Delete { namespace, key } => {
                CommittedWrite::List(CommittedListWrite::Delete {
                    seq,
                    namespace,
                    key,
                })
            }
        },
    }
}

/// Upstream `prepareStorageCommit(writes, firstSeq, timestamp)`
/// (`commit.ts:82-88`): stamp consecutive sequences and collect the result
/// sequences.
pub fn prepare_storage_commit(
    writes: Vec<Write>,
    first_seq: i64,
    timestamp: i64,
) -> PreparedCommit {
    let committed_writes: Vec<CommittedWrite> = writes
        .into_iter()
        .enumerate()
        .map(|(index, write)| commit_write(write, first_seq + index as i64, timestamp))
        .collect();
    let seqs: Vec<i64> = committed_writes.iter().map(CommittedWrite::seq).collect();
    PreparedCommit {
        result: CommitStatsPending {
            first_seq,
            seqs,
            timestamp,
        },
        writes: committed_writes,
    }
}

/// Upstream `CommitValidationState` (`commit.ts:48-51`): the look-back state
/// validation consults.
pub trait CommitValidationState {
    /// Upstream `hasEntryOrUsageId(id)`.
    fn has_entry_or_usage_id(&self, id: &str) -> bool;
    /// Upstream `hasEntryId(id)`.
    fn has_entry_id(&self, id: &str) -> bool;
}

/// Upstream `validateCommittedWrites(writes, firstSeq, state)`
/// (`commit.ts:90-116`): monotonic sequences, one shared entry/usage id
/// namespace, and parents resolved only from prior entries or earlier writes
/// in the same transaction.
pub fn validate_committed_writes(
    writes: &[CommittedWrite],
    first_seq: i64,
    state: &dyn CommitValidationState,
) -> anyhow::Result<()> {
    let mut previous_seq = first_seq - 1;
    let mut transaction_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut transaction_entry_ids: std::collections::HashSet<&str> =
        std::collections::HashSet::new();
    for write in writes {
        let seq = write.seq();
        if seq <= previous_seq {
            anyhow::bail!("Non-monotonic storage sequence: {seq}");
        }
        previous_seq = seq;
        let Some(id) = write.entry_or_usage_id() else {
            continue;
        };
        if state.has_entry_or_usage_id(id) || transaction_ids.contains(id) {
            anyhow::bail!("Duplicate entry or usage id: {id}");
        }
        if let Some(parent_id) = write.entry_parent_id() {
            if !state.has_entry_id(parent_id) && !transaction_entry_ids.contains(parent_id) {
                anyhow::bail!("Missing parent entry: {parent_id}");
            }
        }
        transaction_ids.insert(id);
        if write.is_entry() {
            transaction_entry_ids.insert(id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
