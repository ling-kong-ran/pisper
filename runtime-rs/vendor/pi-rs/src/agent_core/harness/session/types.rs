//! Port of `packages/agent/src/harness/session/types.ts` (602 lines): the
//! durable session vocabulary — transcript entries, the write/commit shapes,
//! scans, the value addressing, session/branch/mutation capabilities, and the
//! storage + repo interfaces. (M3b Task 5 landed the compaction used-surface
//! subset; Task 7 grows it to the full session surface.)
//!
//! Wire format: entries are tagged by `type` with the upstream snake_case
//! literals (`message`, `compaction`, `branch_summary`, `custom`) and
//! camelCase fields; `parentId`/`fromId` serialize `null` like the upstream
//! `string | null` always-present fields, and optional fields are omitted
//! (upstream `undefined`). Committed writes are tagged by `kind`
//! (`entry`, `usage`, `value`, `list`) — the JSONL transaction line format.
//!
//! Disclosed substitutions:
//! - Upstream `Entry` is a union of interfaces sharing `EntryBase`; the port
//!   is a closed enum with the shared fields inlined per variant.
//!   `customType` only ever carries a value on `custom` entries upstream, so
//!   it lives on [`Entry::Custom`] alone. [`NewEntry`] mirrors the enum minus
//!   `seq`/`timestamp` (upstream `Omit<TEntry, "seq" | "timestamp">`).
//! - Upstream `JsonlSessionMetadata extends SessionMetadata` flattens into
//!   one [`SessionMetadata`] struct whose `path`/`modifiedAt` are `None` for
//!   memory-backed sessions; the wire header keeps its own dedicated shape
//!   ([`crate::agent_core::harness::session::jsonl::types`]).
//! - Upstream generic `Value<T>`/`ValueList<T>` addresses erase to
//!   [`ValueAddress`]; stored values are opaque `serde_json::Value` (the
//!   upstream runtime type is `unknown` — storage never inspects payloads
//!   beyond the fork-policy namespace switch, which keys on strings too).
//!   The typed operation-state leaves (`OperationState` and friends) stay
//!   with the runtime/lanes consumers (M3b Task 10): the session layer treats
//!   them as opaque values, exactly like upstream.
//! - [`BranchReader`]/[`SessionReader`] remain the Task 5 capability
//!   projections (`Pick<Branch, "findEntries">` / `Pick<Session,
//!   "getEntry">`); the full [`Branch`]/[`Session`] interfaces are
//!   supertraits of them, so the concrete session types satisfy both.
//!   Handler-thrown errors (upstream `throw new Error(...)`) are the `Err`
//!   channel ([`anyhow::Error`]).
//! - `SessionMutationCallback`/`SessionMutator` map to `FnOnce(&dyn
//!   SessionMutator, Context) -> Future`; upstream's callback-scoped
//!   capability invalidation is enforced by the shared `active` flag on the
//!   concrete mutation (upstream throws after `end()`).

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::serde_support::present_json;

use super::values::{ListElement, ListReadOptions, StoredValue, ValueAddress};
use crate::agent_core::chord_support::Context;
use crate::agent_core::types::AgentMessage;
use crate::ai::types::primitives::Usage;
use crate::ai::uuid;

/// Upstream `Number.MAX_SAFE_INTEGER` — the `findEntries` ascending-cursor
/// guard compares against it (`session.ts:323`).
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Upstream `EntryType` (`types.ts:16`): the `type` tag discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    Message,
    Compaction,
    BranchSummary,
    Custom,
}

impl EntryType {
    /// The wire literal (`"message"` etc.).
    pub fn as_str(&self) -> &'static str {
        match self {
            EntryType::Message => "message",
            EntryType::Compaction => "compaction",
            EntryType::BranchSummary => "branch_summary",
            EntryType::Custom => "custom",
        }
    }
}

/// Upstream `Entry` (`types.ts:18-64`): one durable transcript record,
/// tagged by `type`.
///
/// The Message variant is intrinsically the largest payload (every entry
/// carries the full `AgentMessage`); boxing it would add indirection at every
/// use site for no functional gain (same precedent as `AgentMessage`).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Entry {
    /// Upstream `MessageEntry` (`types.ts:27-31`).
    Message {
        id: String,
        parent_id: Option<String>,
        seq: i64,
        timestamp: i64,
        message: AgentMessage,
        /// Upstream `terminate?: true` — hints the run should stop after this
        /// entry. Only serialized when set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminate: Option<bool>,
    },
    /// Upstream `CompactionEntry` (`types.ts:33-41`).
    Compaction {
        id: String,
        parent_id: Option<String>,
        seq: i64,
        timestamp: i64,
        summary: String,
        retained_tail: Vec<AgentMessage>,
        tokens_before: i64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_json"
        )]
        details: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        from_hook: bool,
    },
    /// Upstream `BranchSummaryEntry` (`types.ts:43-50`).
    BranchSummary {
        id: String,
        parent_id: Option<String>,
        seq: i64,
        timestamp: i64,
        /// Upstream `fromId: string | null` — always present on the wire.
        from_id: Option<String>,
        summary: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_json"
        )]
        details: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        from_hook: bool,
    },
    /// Upstream `CustomEntry` (`types.ts:52-56`).
    Custom {
        id: String,
        parent_id: Option<String>,
        seq: i64,
        timestamp: i64,
        custom_type: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_json"
        )]
        data: Option<serde_json::Value>,
    },
}

impl Entry {
    /// The entry's upstream `id`.
    pub fn id(&self) -> &str {
        match self {
            Entry::Message { id, .. }
            | Entry::Compaction { id, .. }
            | Entry::BranchSummary { id, .. }
            | Entry::Custom { id, .. } => id,
        }
    }

    /// The entry's upstream `parentId`.
    pub fn parent_id(&self) -> Option<&str> {
        match self {
            Entry::Message { parent_id, .. }
            | Entry::Compaction { parent_id, .. }
            | Entry::BranchSummary { parent_id, .. }
            | Entry::Custom { parent_id, .. } => parent_id.as_deref(),
        }
    }

    /// The entry's upstream `seq`.
    pub fn seq(&self) -> i64 {
        match self {
            Entry::Message { seq, .. }
            | Entry::Compaction { seq, .. }
            | Entry::BranchSummary { seq, .. }
            | Entry::Custom { seq, .. } => *seq,
        }
    }

    /// The entry's upstream `timestamp`.
    pub fn timestamp(&self) -> i64 {
        match self {
            Entry::Message { timestamp, .. }
            | Entry::Compaction { timestamp, .. }
            | Entry::BranchSummary { timestamp, .. }
            | Entry::Custom { timestamp, .. } => *timestamp,
        }
    }

    /// The entry's `type` discriminator ([`EntryType`]).
    pub fn entry_type(&self) -> EntryType {
        match self {
            Entry::Message { .. } => EntryType::Message,
            Entry::Compaction { .. } => EntryType::Compaction,
            Entry::BranchSummary { .. } => EntryType::BranchSummary,
            Entry::Custom { .. } => EntryType::Custom,
        }
    }

    /// The `customType` payload of a `custom` entry.
    pub fn custom_type(&self) -> Option<&str> {
        match self {
            Entry::Custom { custom_type, .. } => Some(custom_type),
            _ => None,
        }
    }

    /// The message payload of a `message` entry.
    pub fn message(&self) -> Option<&AgentMessage> {
        match self {
            Entry::Message { message, .. } => Some(message),
            _ => None,
        }
    }
}

/// Upstream `NewEntry` (`types.ts:67`): an entry supplied to a transaction
/// before storage assigns sequence and timestamp — [`Entry`] minus
/// `seq`/`timestamp` (same tag/field wire spellings).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum NewEntry {
    /// `Omit<MessageEntry, "seq" | "timestamp">`.
    Message {
        id: String,
        parent_id: Option<String>,
        message: AgentMessage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminate: Option<bool>,
    },
    /// `Omit<CompactionEntry, "seq" | "timestamp">`.
    Compaction {
        id: String,
        parent_id: Option<String>,
        summary: String,
        retained_tail: Vec<AgentMessage>,
        tokens_before: i64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_json"
        )]
        details: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        from_hook: bool,
    },
    /// `Omit<BranchSummaryEntry, "seq" | "timestamp">`.
    BranchSummary {
        id: String,
        parent_id: Option<String>,
        from_id: Option<String>,
        summary: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_json"
        )]
        details: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        from_hook: bool,
    },
    /// `Omit<CustomEntry, "seq" | "timestamp">`.
    Custom {
        id: String,
        parent_id: Option<String>,
        custom_type: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_json"
        )]
        data: Option<serde_json::Value>,
    },
}

impl NewEntry {
    /// The staged entry's `id`.
    pub fn id(&self) -> &str {
        match self {
            NewEntry::Message { id, .. }
            | NewEntry::Compaction { id, .. }
            | NewEntry::BranchSummary { id, .. }
            | NewEntry::Custom { id, .. } => id,
        }
    }

    /// Upstream `materializeCommittedEntry` (`commit.ts:78-80`): stamp the
    /// storage-assigned sequence and timestamp.
    pub fn into_entry(self, seq: i64, timestamp: i64) -> Entry {
        match self {
            NewEntry::Message {
                id,
                parent_id,
                message,
                terminate,
            } => Entry::Message {
                id,
                parent_id,
                seq,
                timestamp,
                message,
                terminate,
            },
            NewEntry::Compaction {
                id,
                parent_id,
                summary,
                retained_tail,
                tokens_before,
                details,
                usage,
                from_hook,
            } => Entry::Compaction {
                id,
                parent_id,
                seq,
                timestamp,
                summary,
                retained_tail,
                tokens_before,
                details,
                usage,
                from_hook,
            },
            NewEntry::BranchSummary {
                id,
                parent_id,
                from_id,
                summary,
                details,
                usage,
                from_hook,
            } => Entry::BranchSummary {
                id,
                parent_id,
                seq,
                timestamp,
                from_id,
                summary,
                details,
                usage,
                from_hook,
            },
            NewEntry::Custom {
                id,
                parent_id,
                custom_type,
                data,
            } => Entry::Custom {
                id,
                parent_id,
                seq,
                timestamp,
                custom_type,
                data,
            },
        }
    }
}

/// Upstream `EntryCursor` (`types.ts:417-419`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryCursor {
    pub seq: i64,
}

/// Upstream `BranchScan` order literals (`types.ts:427`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BranchScanOrder {
    NewestFirst,
    OldestFirst,
}

/// Upstream `BranchScan` (`types.ts:421-430`): walk a branch's ancestry
/// starting at `start` (a tip id), stopping at a boundary, filtering, and
/// paging. All fields optional except as noted; serde camelCase like the
/// upstream object keys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchScan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_at_type: Option<EntryType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_at_id: Option<String>,
    /// Upstream `type?` — serde renames the field (`type` is not an
    /// identifier-compatible JSON key name in Rust).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub scan_type: Option<EntryType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<BranchScanOrder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<EntryCursor>,
}

/// Upstream `StorageBranchScan = BranchScan & { start: string }`
/// (`types.ts:432`): the storage-level branch scan with a required start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageBranchScan {
    pub start: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_at_type: Option<EntryType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_at_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub scan_type: Option<EntryType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<BranchScanOrder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<EntryCursor>,
}

impl StorageBranchScan {
    /// The `BranchScan & { start }` widening.
    pub fn from_branch_scan(scan: &BranchScan, start: String) -> Self {
        StorageBranchScan {
            start,
            stop_at_type: scan.stop_at_type,
            stop_at_id: scan.stop_at_id.clone(),
            scan_type: scan.scan_type,
            custom_type: scan.custom_type.clone(),
            order: scan.order,
            limit: scan.limit,
            cursor: scan.cursor,
        }
    }
}

/// Upstream `EntryScan` order literals (`types.ts:439`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AscDescOrder {
    Asc,
    Desc,
}

/// Upstream `EntryScan` (`types.ts:434-441`): a global (cross-branch) entry
/// scan by sequence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryScan {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub scan_type: Option<EntryType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<AscDescOrder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// Upstream `UsageScan` (`types.ts:443-448`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageScan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<AscDescOrder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// Upstream `EntryStructure` (`types.ts:408-415`): the payload-free branch
/// structure row (used by storage `scanBranchStructure`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryStructure {
    pub id: String,
    pub parent_id: Option<String>,
    pub seq: i64,
    pub timestamp: i64,
    #[serde(rename = "type")]
    pub entry_type: EntryType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_type: Option<String>,
}

/// Upstream `SessionStats` (`types.ts:450-453`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub message_count: u64,
    pub usage: Usage,
}

/// Upstream `UsageRow` (`types.ts:379-386`): one durable usage-ledger row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    pub id: String,
    pub seq: i64,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    pub adjustment: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_json"
    )]
    pub details: Option<serde_json::Value>,
}

/// Upstream `Omit<UsageRow, "seq">` (`types.ts:395`): the staged usage row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewUsageRow {
    pub id: String,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    pub adjustment: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_json"
    )]
    pub details: Option<serde_json::Value>,
}

/// Upstream `CommitResult` (`types.ts:400-406`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitResult {
    pub first_seq: i64,
    pub seqs: Vec<i64>,
    pub timestamp: i64,
    /// Session totals immediately after this commit was applied.
    pub stats: SessionStats,
}

/// Upstream `SessionMetadata` + `JsonlSessionMetadata` (`types.ts:473-480`,
/// `jsonl/types.ts:26-31`), flattened: `path`/`modifiedAt` are `None` for
/// non-file-backed sessions. See the module docs for the substitution note.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    pub id: String,
    pub created_at: i64,
    pub storage_version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_parent_session_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Filesystem modification time as milliseconds since Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<f64>,
}

/// Upstream `IdGenerator` (`types.ts:482-484`): mints entry ids. The default
/// implementation is the shared UUIDv7 generator
/// (`crate::ai::uuid`, upstream `pi-ai/utils/uuid`).
pub trait IdGenerator: Send + Sync {
    /// `next(timestampMs?)` — a supplied timestamp is preserved for follower
    /// ids. Invalid timestamps are a caller contract violation (upstream
    /// throws `RangeError` synchronously; the trait cannot error, so the
    /// default generator `expect`s — disclosed substitution).
    fn next(&self, timestamp_ms: Option<i64>) -> String;
}

/// The default [`IdGenerator`] over the shared UUIDv7 state.
#[derive(Debug, Clone, Copy, Default)]
pub struct UuidV7Generator;

impl IdGenerator for UuidV7Generator {
    fn next(&self, timestamp_ms: Option<i64>) -> String {
        match timestamp_ms {
            Some(timestamp) => uuid::uuid_v7_at(timestamp).expect("uuidv7 timestamp out of range"),
            None => uuid::uuid_v7(),
        }
    }
}

/// Upstream `EntryQuery` (`types.ts:486-492`): the session-level find query.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryQuery {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub scan_type: Option<EntryType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<AscDescOrder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<EntryCursor>,
}

/// Upstream `LaneConfiguration["model"]` (`types.ts:70`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneModel {
    pub provider: String,
    pub model_id: String,
}

/// Upstream `LaneConfiguration` (`types.ts:69-73`): one AgentLane's durable
/// model configuration (the `pi.lane.config` value payload).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneConfiguration {
    pub model: LaneModel,
    pub thinking_level: crate::agent_core::types::ThinkingLevel,
    pub active_tool_names: Vec<String>,
}

/// Upstream `InboxItemKind` (`types.ts:129`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InboxItemKind {
    Steer,
    FollowUp,
    NextRun,
    Write,
}

/// Upstream `InboxItem` (`types.ts:131-134`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub entry_id: String,
    pub kind: InboxItemKind,
}

/// Upstream `LaneState` (`types.ts:344-348`): the `pi.lane.state` value
/// payload. The port keeps `inbox` opaque because `InboxItem` only names part
/// of the upstream runtime shape (operation leaves are deferred; see module
/// docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneState {
    pub current_operation_id: Option<String>,
    pub last_operation_id: Option<String>,
    pub inbox: serde_json::Value,
}

/// Upstream `Control` (`types.ts:92-97`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Control {
    Running,
    CancelRequested { requested_at: i64 },
}

/// Upstream `TerminalStatus` (`types.ts:105`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Completed,
    Declined,
    Aborted,
    Failed,
}

/// Upstream `OperationError` (`types.ts:99-103`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationError {
    pub code: String,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_json"
    )]
    pub details: Option<serde_json::Value>,
}

/// Upstream `OperationResultRecord` (`types.ts:108-117`): the immutable
/// lane-lived observation record (`pi.result` value payload).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationResultRecord {
    pub operation_id: String,
    /// Upstream `OperationMeta["intent"]["kind"]` literal.
    pub kind: String,
    pub status: TerminalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<OperationError>,
    pub from_tip_id: Option<String>,
    pub tip_id: Option<String>,
    pub started_at: i64,
    pub ended_at: i64,
}

/// Upstream `PendingEntry` (`types.ts:350-352`): the `pi.pending.entry`
/// value payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)] // Message carries the full pending payload, like Entry
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PendingEntry {
    Message {
        payload: AgentMessage,
    },
    Custom {
        custom_type: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_json"
        )]
        payload: Option<serde_json::Value>,
    },
}

/// Upstream `Write` (`types.ts:398`): one staged write in a transaction.
/// Wire shapes live on the committed form
/// ([`crate::agent_core::harness::session::commit::CommittedWrite`]); this is
/// the pre-commit staging enum.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // Entry carries the staged transcript record
pub enum Write {
    /// `EntryWrite` (`types.ts:388-391`).
    Entry { entry: NewEntry },
    /// `UsageWrite` (`types.ts:393-396`).
    Usage { row: NewUsageRow },
    /// `ValueWrite` (`values.ts:89`).
    Value(ValueWrite),
    /// `ListWrite` (`values.ts:90`).
    List(ListWrite),
}

/// Upstream `ValueWrite` (`values.ts:59-72, 89`).
#[derive(Debug, Clone, PartialEq)]
pub enum ValueWrite {
    Set {
        namespace: String,
        key: String,
        value: serde_json::Value,
    },
    Delete {
        namespace: String,
        key: String,
    },
}

/// Upstream `ListWrite` (`values.ts:74-87, 90`).
#[derive(Debug, Clone, PartialEq)]
pub enum ListWrite {
    Append {
        namespace: String,
        key: String,
        value: serde_json::Value,
    },
    Delete {
        namespace: String,
        key: String,
    },
}

/// Upstream `SessionCreateOptions` (`types.ts:557-560`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionCreateOptions {
    pub id: Option<String>,
    pub parent_session_id: Option<String>,
}

/// Upstream `Storage` (`types.ts:455-471`): the durable session storage the
/// capability layer and both backends share.
pub trait Storage: Send + Sync {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CommitResult>>;
    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<HashMap<String, Entry>>>;
    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<StoredValue>>>;
    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<StoredValue>>>;
    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&ListReadOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<ListElement>>>;
    fn scan_branch<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>>;
    fn scan_branch_structure<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<EntryStructure>>>;
    fn scan_entries<'a>(
        &'a self,
        query: &EntryScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>>;
    fn scan_usage<'a>(
        &'a self,
        query: &UsageScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<UsageRow>>>;
    fn get_stats<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>>;
    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>>;
}

/// Upstream `Pick<Branch, "findEntries">` (`types.ts:521-528`): read a
/// branch's ancestry. The Task 5 capability projection; the full [`Branch`]
/// interface is a supertrait.
pub trait BranchReader: Send + Sync {
    /// Upstream `Branch.findEntries(query, context)`: entries along the
    /// branch ancestry described by the scan, in the storage's order.
    fn find_entries<'a>(
        &'a self,
        query: Option<&BranchScan>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>>;
}

/// Upstream `Pick<Session, "getEntry">` (`types.ts:530-533`): fetch one
/// entry by id. The Task 5 capability projection; the full [`Session`]
/// interface is a supertrait.
pub trait SessionReader: Send + Sync {
    /// Upstream `Session.getEntry(id, context)`.
    fn get_entry<'a>(
        &'a self,
        id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Entry>>>;
}

/// Upstream `ForkOptions` position literal (`types.ts:577`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkPosition {
    Before,
    At,
}

/// Upstream `ForkOptions` (`types.ts:562-590`): the `scope`-tagged fork
/// request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkOptions {
    Branch {
        branch: String,
        entry_id: Option<String>,
        position: Option<ForkPosition>,
        id: Option<String>,
    },
    Tree {
        id: Option<String>,
    },
}

/// Upstream `SessionMutation` reader surface (`types.ts:494-506`): the reads
/// every mutation capability forwards to storage.
pub trait SessionMutationReader: Send + Sync {
    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<HashMap<String, Entry>>>;
    fn get_stats<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>>;
    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<StoredValue>>>;
    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<StoredValue>>>;
    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&ListReadOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<ListElement>>>;
    /// Scan a branch from an explicit entry while this capability remains
    /// valid.
    fn scan_branch<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>>;
}

/// Upstream `SessionMutator` (`types.ts:517`): the callback-scoped mutation
/// capability without authority to release the barrier.
pub trait SessionMutator: SessionMutationReader {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CommitResult>>;
}

/// Upstream `SessionMutation` (`types.ts:509-514`): the exclusive keyless
/// mutation capability — a [`SessionMutator`] plus `end`, which waits for
/// the commit, invalidates the capability, and releases the barrier.
pub trait SessionMutation: SessionMutator {
    fn end<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>>;
}

/// Upstream `Branch` (`types.ts:521-528`): one named branch handle.
/// [`BranchReader`] is the Task 5 projection supertrait.
pub trait Branch: BranchReader {
    /// The branch name.
    fn name(&self) -> &str;
    /// `getTipId(context)`.
    fn get_tip_id<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Option<String>>>;
    /// `findEntry(query, context)`.
    fn find_entry<'a>(
        &'a self,
        query: Option<&BranchScan>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Entry>>>;
    /// `appendMessage(message, context)` — appends a message entry at the tip
    /// and returns the new entry id.
    fn append_message<'a>(
        &'a self,
        message: AgentMessage,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<String>>;
    /// `appendCustomEntry(customType, data, context)`.
    fn append_custom_entry<'a>(
        &'a self,
        custom_type: String,
        data: Option<serde_json::Value>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<String>>;
}

/// Upstream `Session` (`types.ts:530-555`): the full session capability.
/// [`SessionReader`] is the Task 5 projection supertrait; `mutate` is generic
/// (the trait is used through generics, not trait objects).
pub trait Session: SessionReader {
    /// The session metadata (by reference — upstream exposes the stored
    /// record object itself).
    fn metadata(&self) -> &SessionMetadata;
    /// The session id generator.
    fn id_generator(&self) -> Arc<dyn IdGenerator>;
    /// `getEntries(ids, context)`.
    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<HashMap<String, Entry>>>;
    /// `getStats(context)`.
    fn get_stats<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>>;
    /// `getValue(address, context)`.
    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<StoredValue>>>;
    /// `scanValues(prefix, context)`.
    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<StoredValue>>>;
    /// `readList(address, options, context)`.
    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&ListReadOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<ListElement>>>;
    /// `scanBranch(query, context)` — the storage-level branch scan.
    fn scan_branch<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>>;
    /// `getName(context)`.
    fn get_name<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Option<String>>>;
    /// `getLabel(targetId, context)`.
    fn get_label<'a>(
        &'a self,
        target_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<String>>>;
    /// `findEntries(query, context)` — global scan with cursor paging.
    fn find_entries<'a>(
        &'a self,
        query: Option<&EntryQuery>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>>;
    /// `findEntry(query, context)`.
    fn find_entry<'a>(
        &'a self,
        query: Option<&EntryQuery>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Entry>>>;
    /// `branch(name, context)` — the existing branch, if any.
    fn branch<'a>(
        &'a self,
        name: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Arc<dyn Branch>>>>;
    /// `createBranch(name, at, context)`.
    fn create_branch<'a>(
        &'a self,
        name: &str,
        at: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Arc<dyn Branch>>>;
    /// `beginMutation(context)`: acquire the exclusive mutation capability.
    fn begin_mutation<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Box<dyn SessionMutation>>>;
    /// `mutate(mutation, context)`: the trusted exclusive callback. Awaiting
    /// a public writer from inside the callback queues behind it (and
    /// deadlocks if awaited before returning), like upstream.
    fn mutate<'a, T, F>(&'a self, mutation: F, context: Context) -> BoxFuture<'a, anyhow::Result<T>>
    where
        F: FnOnce(&dyn SessionMutator, Context) -> BoxFuture<'_, anyhow::Result<T>> + Send + 'a,
        T: Send + 'a,
    {
        // Upstream default composition: beginMutation -> callback -> end.
        Box::pin(async move {
            let handle = self.begin_mutation(context.clone()).await?;
            let outcome = mutation(handle.as_ref(), context.clone()).await;
            handle.end(context.clone()).await?;
            outcome
        })
    }
    /// `setValue(address, next, context)`.
    fn set_value<'a>(
        &'a self,
        address: ValueAddress,
        next: serde_json::Value,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    /// `deleteValue(address, context)`.
    fn delete_value<'a>(
        &'a self,
        address: ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    /// `appendList(address, element, context)`.
    fn append_list<'a>(
        &'a self,
        address: ValueAddress,
        element: serde_json::Value,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    /// `deleteList(address, context)`.
    fn delete_list<'a>(
        &'a self,
        address: ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    /// `setName(name, context)` — `undefined` deletes the name.
    fn set_name<'a>(
        &'a self,
        name: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    /// `setLabel(targetId, label, context)`.
    fn set_label<'a>(
        &'a self,
        target_id: &str,
        label: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    /// `close(context)`.
    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>>;
}

/// Upstream `SessionRepo` (`types.ts:592-602`) is generic over its
/// metadata/create-options/list-options types
/// (`SessionRepo<JsonlSessionMetadata, JsonlSessionCreateOptions, ...>`), so
/// the concrete repo signatures cannot be expressed by one non-generic Rust
/// trait without losing the capability types. The repositories therefore
/// expose their upstream methods as inherent methods (see
/// [`crate::agent_core::harness::session::jsonl::repo`] and
/// [`crate::agent_core::harness::session::memory`]); this docs-only alias
/// records the substitution point.
pub type SessionRepoUpstream = ();

#[cfg(test)]
mod tests;
