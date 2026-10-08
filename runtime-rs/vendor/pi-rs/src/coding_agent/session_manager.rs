//! Port of upstream `coding-agent/src/core/session-manager.ts` (v0.99.1,
//! sha256 `450d82c5…64bff`): append-only JSONL session trees with id/parentId
//! entries, leaf-pointer branching, compaction/branch-summary/label/session-
//! info/custom entries, v1→v3 migrations, deferred file persistence, session
//! discovery (`findMostRecentSession` / `findById` / `list` / `listAll`),
//! forking (`forkFrom`, `createBranchedSession`) and the context projection
//! (`buildSessionContext` / `buildContextEntries` /
//! `sessionEntryToContextMessages`).
//!
//! Deterministic outputs (context projections, migration results, JSONL file
//! bytes, discovery order, error texts) were captured from the real upstream
//! TypeScript under node into
//! `tests/fixtures/session_manager_oracle/session_manager.oracle.json` (sha256
//! `d0d016fb…ee84bd`) and compared byte-for-byte in the [`tests`] module. The oracle runs the verbatim
//! upstream source under `node --experimental-strip-types` with two
//! deterministic stubs mirrored by test seams here: `crypto.randomUUID`
//! (entry ids, `"00000001"`-style) and pi-ai `uuidv7` (session ids,
//! `"@u<n>"`), plus the scrub contract (root/stamp/timestamp normalization)
//! documented in the oracle script.
//!
//! Seams and divergences (disclosed):
//!
//! - S1 upstream `getAgentDir`/`getSessionsDir` (`../config.ts`) map to
//!   [`crate::coding_agent::core::get_agent_dir`] (the `PI_CODING_AGENT_DIR`
//!   env override) with `sessions` joined via node-path; `APP_NAME` is
//!   vendored as `"pi"`.
//! - S2 `getCurrentSystemMessage` / `uuidv7` / message types come from the
//!   ported `ai` layer; `AgentMessage` captures extension custom roles as
//!   [`CustomAgentMessage`] data.
//! - S3 `ReadonlySessionManager` (a TS `Pick<>` type) has no Rust
//!   representation and is not ported (type-level only upstream).
//! - S4 async `fs/promises` + `readline` streaming (`buildSessionInfo`,
//!   `list`, `listAll`) ports to synchronous reads; the
//!   `MAX_CONCURRENT_SESSION_INFO_LOADS` bounded-concurrency pool collapses
//!   to sequential loads with identical results, ordering and progress
//!   events. Whole-file lossy reads replace the 1 MiB `StringDecoder` loop in
//!   `loadEntriesFromFile` (identical lines, including U+FFFD replacement of
//!   invalid utf-8).
//! - S5 the free `buildContextEntries`/`buildSessionContext`/`buildSessionPath`
//!   drop the optional `byId` map parameter (a performance-only cache; the
//!   index is rebuilt from `entries`, which yields identical results because
//!   the manager's index is built from the same entries, last-wins).
//! - S6 `generateId` randomness comes from a local v4-shaped random UUID
//!   (rand crate) instead of `crypto.randomUUID`; observable shape (8 hex
//!   chars, collision-checked) is preserved. Tests inject deterministic
//!   [`test_id_seam`] stubs.
//! - S7 `new Date(...)` parsing uses the shared ISO-8601 subset parser
//!   ([`parse_epoch_millis`]); node accepts additional legacy date formats
//!   (never produced by pi itself). `new Date().toISOString()` maps to
//!   [`format_iso8601_utc`] over the wall clock. Invalid dates (JS NaN)
//!   surface as `None` ([`SessionInfo::created`]) or `0` where the field is
//!   non-optional upstream (`compaction.systemMessage.timestamp` — reachable
//!   only with a self-generated timestamp). `SessionInfo` dates are epoch
//!   milliseconds instead of `Date` objects.
//! - S8 Entries that fail strict typed parsing (hand-edited files: unknown
//!   entry kinds, null message content, non-string header fields) are kept as
//!   [`FileEntry::Unparsed`] raw JSON instead of loose JS objects, and
//!   [`SessionEntry::Unparsed`] carries them through the entry APIs. All
//!   reached behaviors match upstream: header validation, null-content
//!   context guards, listing crash-parity on null message content, message
//!   counting, file round-trips (modulo JSON key order, which serde_json
//!   sorts — upstream preserves original order only for re-written parsed
//!   files).
//! - S9 upstream keeps entries lacking an `id` in `byId` under the
//!   `undefined` key and lets `leafId` become `undefined`; no valid lookup
//!   reaches either state, so the port skips id-less entries in the index and
//!   keeps the previous leaf. [`SessionManager::get_leaf_id`] unifies
//!   `null`/`undefined` into `None`.
//! - S10 fs errors (node `CodeError` texts) map to [`SessionManagerError`]
//!   carrying the io error text; every upstream-composed error message is
//!   byte-identical.
//! - S11 rewritten *migrated* files serialize migrated entries in schema
//!   field order; upstream `JSON.stringify` preserves the original file key
//!   order with migration-added keys (`id`/`parentId`/`version`) appended.
//!   The JSON content is identical, the key order differs (invisible to all
//!   consumers; oracle comparisons are key-sorted).
//! - S12 `SessionManager.list` swallows `getDefaultSessionDir` mkdir failures
//!   (upstream propagates them; only reachable on unwritable homes).

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_core::types::{AgentMessage, CustomAgentMessage};
use crate::ai::transcript::get_current_system_message;
use crate::ai::types::{
    AssistantBlock, Message, StringOrBlocks, SystemMessage, TextContent, TextOrImageBlock, Usage,
};
use crate::coding_agent::core::get_agent_dir;
use crate::coding_agent::core::messages::{
    create_branch_summary_message, create_compaction_summary_message, create_custom_message,
    parse_epoch_millis, CustomMessageContent,
};
use crate::coding_agent::utils::node_path;
use crate::coding_agent::utils::paths::{normalize_path, resolve_path_auto_base};

/// Upstream `CURRENT_SESSION_VERSION`.
pub const CURRENT_SESSION_VERSION: u32 = 3;

/// Upstream `APP_NAME` (`pkg.piConfig?.name || "pi"`; seam S1).
const APP_NAME: &str = "pi";

/// Upstream `SESSION_HEADER_READ_BUFFER_SIZE`.
const SESSION_HEADER_READ_BUFFER_SIZE: usize = 4096;
/// Upstream `MAX_SESSION_HEADER_SCAN_BYTES`.
const MAX_SESSION_HEADER_SCAN_BYTES: usize = 1024 * 1024;
/// Upstream `SESSION_READ_BUFFER_SIZE` (the port reads whole files; kept for
/// provenance).
#[allow(dead_code)]
const SESSION_READ_BUFFER_SIZE: usize = 1024 * 1024;

/// Upstream error surfaced from every throwing SessionManager API; `Display`
/// reproduces the upstream error message text byte-for-byte (S10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionManagerError {
    message: String,
}

impl SessionManagerError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SessionManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SessionManagerError {}

impl From<std::io::Error> for SessionManagerError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for SessionManagerError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<crate::coding_agent::utils::paths::PathError> for SessionManagerError {
    fn from(error: crate::coding_agent::utils::paths::PathError) -> Self {
        Self::new(error.to_string())
    }
}

fn err(message: impl Into<String>) -> SessionManagerError {
    SessionManagerError::new(message)
}

/// Upstream `SessionHeader`. Parsed loosely (S8): `id`/`timestamp`/`cwd` are
/// `undefined`-able because session files are parsed without validation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHeader {
    /// v1 sessions don't have this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}

/// Upstream `NewSessionOptions`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewSessionOptions {
    pub id: Option<String>,
    pub parent_session: Option<String>,
}

/// Upstream `SessionMessageEntry`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub message: AgentMessage,
}

/// Upstream `ThinkingLevelChangeEntry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingLevelChangeEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub thinking_level: String,
}

/// Upstream `ModelChangeEntry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChangeEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub provider: String,
    pub model_id: String,
}

/// Upstream `UsageEntry`: model-attributed usage that does not participate in
/// LLM context (e.g. cache warming).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    /// Arbitrary usage category, such as `cache_warm`.
    pub kind: String,
    pub provider: String,
    pub model: String,
    pub usage: Usage,
    /// Optional human-readable qualifier for usage notices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Upstream `ContextEditableContent`: content that an append-only context edit
/// may replace without changing message metadata. `Text` is the upstream
/// `string` shape; `Blocks` is the block-array shape preserved verbatim (the
/// block element union differs per message role upstream).
#[derive(Debug, Clone, PartialEq)]
pub enum ContextEditableContent {
    Text(String),
    Blocks(Value),
}

impl Serialize for ContextEditableContent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ContextEditableContent::Text(text) => text.serialize(serializer),
            ContextEditableContent::Blocks(blocks) => blocks.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ContextEditableContent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(match value {
            Value::String(text) => ContextEditableContent::Text(text),
            other => ContextEditableContent::Blocks(other),
        })
    }
}

/// Upstream `ContextEditEntry["replacement"]`:
/// `{ content: ContextEditableContent } | null`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEditReplacement {
    pub content: ContextEditableContent,
}

/// Upstream `ContextEditEntry`: append-only change to one earlier entry's
/// contribution to model context. `replacement: None` (upstream `null`)
/// omits the target from model context; a value replaces only its content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEditEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub target_id: String,
    #[serde(default)]
    pub replacement: Option<ContextEditReplacement>,
}

/// Upstream `CompactionEntry`. `firstKeptEntryIndex` is a v1-only field
/// captured for the v1→v2 migration and never emitted for current entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub summary: String,
    #[serde(default)]
    pub first_kept_entry_id: Option<String>,
    #[serde(default)]
    pub tokens_before: i64,
    /// Extension-specific data (e.g., ArtifactIndex, version markers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    /// Usage from the LLM call(s) that generated this summary, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// True if generated by an extension, absent/false if pi-generated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
    /// Complete prompt and tool state at this compaction boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_message: Option<SystemMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_kept_entry_index: Option<i64>,
}

/// Upstream `BranchSummaryEntry`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub from_id: String,
    pub summary: String,
    /// Extension-specific data (not sent to LLM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    /// Usage from the LLM call that generated this summary, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// True if generated by an extension, false if pi-generated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
}

/// Upstream `CustomEntry` (extension state; does not participate in context).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomEntry {
    pub custom_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
}

/// Upstream `LabelEntry`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Upstream `SessionInfoEntry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Upstream `CustomMessageEntry` (participates in LLM context).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessageEntry {
    pub custom_type: String,
    pub content: Option<CustomMessageContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    pub display: bool,
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
}

/// Upstream `SessionEntry` union, plus the [`SessionEntry::Unparsed`] capture
/// for loose-parsed entries (S8). Serialization writes the `type` tag first
/// and then the fields in the upstream object-literal order of the
/// constructing `append*` calls, so appended JSONL lines are byte-identical.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SessionEntry {
    #[serde(rename = "message")]
    Message(MessageEntry),
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    #[serde(rename = "model_change")]
    ModelChange(ModelChangeEntry),
    #[serde(rename = "usage")]
    Usage(UsageEntry),
    #[serde(rename = "compaction")]
    Compaction(CompactionEntry),
    #[serde(rename = "branch_summary")]
    BranchSummary(BranchSummaryEntry),
    #[serde(rename = "custom")]
    Custom(CustomEntry),
    #[serde(rename = "custom_message")]
    CustomMessage(CustomMessageEntry),
    #[serde(rename = "context_edit")]
    ContextEdit(ContextEditEntry),
    #[serde(rename = "label")]
    Label(LabelEntry),
    #[serde(rename = "session_info")]
    SessionInfo(SessionInfoEntry),
    /// Raw capture for entries outside the typed schema (S8); never produced
    /// by deserialization (the literal is not a real entry kind).
    #[serde(rename = "__unparsed")]
    Unparsed(Value),
}

impl SessionEntry {
    /// The entry id (`undefined` upstream for unparsed entries without one).
    pub fn id(&self) -> Option<&str> {
        match self {
            SessionEntry::Message(e) => Some(&e.id),
            SessionEntry::ThinkingLevelChange(e) => Some(&e.id),
            SessionEntry::ModelChange(e) => Some(&e.id),
            SessionEntry::Usage(e) => Some(&e.id),
            SessionEntry::Compaction(e) => Some(&e.id),
            SessionEntry::BranchSummary(e) => Some(&e.id),
            SessionEntry::Custom(e) => Some(&e.id),
            SessionEntry::CustomMessage(e) => Some(&e.id),
            SessionEntry::ContextEdit(e) => Some(&e.id),
            SessionEntry::Label(e) => Some(&e.id),
            SessionEntry::SessionInfo(e) => Some(&e.id),
            SessionEntry::Unparsed(_) => None,
        }
    }

    /// The entry's tree parent.
    pub fn parent_id(&self) -> Option<&str> {
        match self {
            SessionEntry::Message(e) => e.parent_id.as_deref(),
            SessionEntry::ThinkingLevelChange(e) => e.parent_id.as_deref(),
            SessionEntry::ModelChange(e) => e.parent_id.as_deref(),
            SessionEntry::Usage(e) => e.parent_id.as_deref(),
            SessionEntry::Compaction(e) => e.parent_id.as_deref(),
            SessionEntry::BranchSummary(e) => e.parent_id.as_deref(),
            SessionEntry::Custom(e) => e.parent_id.as_deref(),
            SessionEntry::CustomMessage(e) => e.parent_id.as_deref(),
            SessionEntry::ContextEdit(e) => e.parent_id.as_deref(),
            SessionEntry::Label(e) => e.parent_id.as_deref(),
            SessionEntry::SessionInfo(e) => e.parent_id.as_deref(),
            SessionEntry::Unparsed(_) => None,
        }
    }

    /// The entry timestamp (string; used for tree ordering).
    pub fn timestamp(&self) -> &str {
        match self {
            SessionEntry::Message(e) => &e.timestamp,
            SessionEntry::ThinkingLevelChange(e) => &e.timestamp,
            SessionEntry::ModelChange(e) => &e.timestamp,
            SessionEntry::Usage(e) => &e.timestamp,
            SessionEntry::Compaction(e) => &e.timestamp,
            SessionEntry::BranchSummary(e) => &e.timestamp,
            SessionEntry::Custom(e) => &e.timestamp,
            SessionEntry::CustomMessage(e) => &e.timestamp,
            SessionEntry::ContextEdit(e) => &e.timestamp,
            SessionEntry::Label(e) => &e.timestamp,
            SessionEntry::SessionInfo(e) => &e.timestamp,
            SessionEntry::Unparsed(value) => value
                .get("timestamp")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        }
    }

    fn set_parent_id(&mut self, parent_id: Option<String>) {
        match self {
            SessionEntry::Message(e) => e.parent_id = parent_id,
            SessionEntry::ThinkingLevelChange(e) => e.parent_id = parent_id,
            SessionEntry::ModelChange(e) => e.parent_id = parent_id,
            SessionEntry::Usage(e) => e.parent_id = parent_id,
            SessionEntry::Compaction(e) => e.parent_id = parent_id,
            SessionEntry::BranchSummary(e) => e.parent_id = parent_id,
            SessionEntry::Custom(e) => e.parent_id = parent_id,
            SessionEntry::CustomMessage(e) => e.parent_id = parent_id,
            SessionEntry::ContextEdit(e) => e.parent_id = parent_id,
            SessionEntry::Label(e) => e.parent_id = parent_id,
            SessionEntry::SessionInfo(e) => e.parent_id = parent_id,
            SessionEntry::Unparsed(value) => {
                match parent_id {
                    Some(parent) => value["parentId"] = Value::String(parent),
                    None => {
                        if let Some(object) = value.as_object_mut() {
                            object.shift_remove("parentId");
                        }
                    }
                };
            }
        }
    }

    fn set_id(&mut self, id: String) {
        match self {
            SessionEntry::Message(e) => e.id = id,
            SessionEntry::ThinkingLevelChange(e) => e.id = id,
            SessionEntry::ModelChange(e) => e.id = id,
            SessionEntry::Usage(e) => e.id = id,
            SessionEntry::Compaction(e) => e.id = id,
            SessionEntry::BranchSummary(e) => e.id = id,
            SessionEntry::Custom(e) => e.id = id,
            SessionEntry::CustomMessage(e) => e.id = id,
            SessionEntry::ContextEdit(e) => e.id = id,
            SessionEntry::Label(e) => e.id = id,
            SessionEntry::SessionInfo(e) => e.id = id,
            SessionEntry::Unparsed(value) => value["id"] = Value::String(id),
        }
    }
}

impl Serialize for SessionEntry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        if let SessionEntry::Unparsed(value) = self {
            return value.serialize(serializer);
        }
        let mut map = serializer.serialize_map(None)?;
        macro_rules! entry {
            ($key:expr, $value:expr) => {
                map.serialize_entry($key, $value)?
            };
        }
        macro_rules! optional {
            ($key:expr, $value:expr) => {
                if let Some(value) = &$value {
                    map.serialize_entry($key, value)?;
                }
            };
        }
        match self {
            SessionEntry::Message(e) => {
                entry!("type", "message");
                entry!("id", &e.id);
                entry!("parentId", &e.parent_id);
                entry!("timestamp", &e.timestamp);
                entry!("message", &e.message);
            }
            SessionEntry::ThinkingLevelChange(e) => {
                entry!("type", "thinking_level_change");
                entry!("id", &e.id);
                entry!("parentId", &e.parent_id);
                entry!("timestamp", &e.timestamp);
                entry!("thinkingLevel", &e.thinking_level);
            }
            SessionEntry::ModelChange(e) => {
                entry!("type", "model_change");
                entry!("id", &e.id);
                entry!("parentId", &e.parent_id);
                entry!("timestamp", &e.timestamp);
                entry!("provider", &e.provider);
                entry!("modelId", &e.model_id);
            }
            SessionEntry::Usage(e) => {
                entry!("type", "usage");
                entry!("id", &e.id);
                entry!("parentId", &e.parent_id);
                entry!("timestamp", &e.timestamp);
                entry!("kind", &e.kind);
                entry!("provider", &e.provider);
                entry!("model", &e.model);
                entry!("usage", &e.usage);
                optional!("note", e.note);
            }
            SessionEntry::ContextEdit(e) => {
                entry!("type", "context_edit");
                entry!("id", &e.id);
                entry!("parentId", &e.parent_id);
                entry!("timestamp", &e.timestamp);
                entry!("targetId", &e.target_id);
                entry!("replacement", &e.replacement);
            }
            SessionEntry::Compaction(e) => {
                entry!("type", "compaction");
                entry!("id", &e.id);
                entry!("parentId", &e.parent_id);
                entry!("timestamp", &e.timestamp);
                entry!("summary", &e.summary);
                optional!("firstKeptEntryId", e.first_kept_entry_id);
                entry!("tokensBefore", &e.tokens_before);
                optional!("details", e.details);
                optional!("usage", e.usage);
                optional!("fromHook", e.from_hook);
                optional!("systemMessage", e.system_message);
                optional!("firstKeptEntryIndex", e.first_kept_entry_index);
            }
            SessionEntry::BranchSummary(e) => {
                entry!("type", "branch_summary");
                entry!("id", &e.id);
                entry!("parentId", &e.parent_id);
                entry!("timestamp", &e.timestamp);
                entry!("fromId", &e.from_id);
                entry!("summary", &e.summary);
                optional!("details", e.details);
                optional!("usage", e.usage);
                optional!("fromHook", e.from_hook);
            }
            SessionEntry::Custom(e) => {
                entry!("type", "custom");
                entry!("customType", &e.custom_type);
                optional!("data", e.data);
                entry!("id", &e.id);
                entry!("parentId", &e.parent_id);
                entry!("timestamp", &e.timestamp);
            }
            SessionEntry::CustomMessage(e) => {
                entry!("type", "custom_message");
                entry!("customType", &e.custom_type);
                optional!("content", e.content);
                entry!("display", &e.display);
                optional!("details", e.details);
                entry!("id", &e.id);
                entry!("parentId", &e.parent_id);
                entry!("timestamp", &e.timestamp);
            }
            SessionEntry::Label(e) => {
                entry!("type", "label");
                entry!("id", &e.id);
                entry!("parentId", &e.parent_id);
                entry!("timestamp", &e.timestamp);
                entry!("targetId", &e.target_id);
                optional!("label", e.label);
            }
            SessionEntry::SessionInfo(e) => {
                entry!("type", "session_info");
                entry!("id", &e.id);
                entry!("parentId", &e.parent_id);
                entry!("timestamp", &e.timestamp);
                optional!("name", e.name);
            }
            SessionEntry::Unparsed(_) => unreachable!(),
        }
        map.end()
    }
}

/// Upstream `FileEntry` (session header | session entry | raw capture, S8).
/// The `Entry` payload dominates (same size-difference allow as
/// [`crate::agent_core::types::AgentMessage`]).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum FileEntry {
    Session(SessionHeader),
    Entry(SessionEntry),
    Unparsed(Value),
}

impl FileEntry {
    /// Typed `SessionHeader` discrimination (upstream `typeof header.id ===
    /// "string"` validation additionally requires the id).
    fn is_typed_session(&self) -> bool {
        matches!(self, FileEntry::Session(_))
    }

    /// The upstream loose `entry.type` value.
    fn type_name(&self) -> Option<&str> {
        match self {
            FileEntry::Session(_) => Some("session"),
            FileEntry::Entry(SessionEntry::Unparsed(value)) | FileEntry::Unparsed(value) => {
                value.get("type").and_then(Value::as_str)
            }
            FileEntry::Entry(typed) => Some(match typed {
                SessionEntry::Message(_) => "message",
                SessionEntry::ThinkingLevelChange(_) => "thinking_level_change",
                SessionEntry::ModelChange(_) => "model_change",
                SessionEntry::Usage(_) => "usage",
                SessionEntry::Compaction(_) => "compaction",
                SessionEntry::BranchSummary(_) => "branch_summary",
                SessionEntry::Custom(_) => "custom",
                SessionEntry::CustomMessage(_) => "custom_message",
                SessionEntry::ContextEdit(_) => "context_edit",
                SessionEntry::Label(_) => "label",
                SessionEntry::SessionInfo(_) => "session_info",
                SessionEntry::Unparsed(_) => unreachable!(),
            }),
        }
    }
}

impl Serialize for FileEntry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            FileEntry::Entry(entry) => entry.serialize(serializer),
            FileEntry::Unparsed(value) => value.serialize(serializer),
            FileEntry::Session(header) => {
                use serde::ser::SerializeMap;
                // Upstream header literal order: type, version, id, timestamp,
                // cwd, parentSession.
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "session")?;
                if let Some(version) = &header.version {
                    map.serialize_entry("version", version)?;
                }
                if let Some(id) = &header.id {
                    map.serialize_entry("id", id)?;
                }
                if let Some(timestamp) = &header.timestamp {
                    map.serialize_entry("timestamp", timestamp)?;
                }
                if let Some(cwd) = &header.cwd {
                    map.serialize_entry("cwd", cwd)?;
                }
                if let Some(parent_session) = &header.parent_session {
                    map.serialize_entry("parentSession", parent_session)?;
                }
                map.end()
            }
        }
    }
}

/// Parse a JSON value into a [`FileEntry`], mirroring the upstream loose
/// `JSON.parse` plus the port's typed-schema fallback (S8).
fn file_entry_from_value(value: Value) -> FileEntry {
    match value.get("type").and_then(Value::as_str) {
        Some("session") => serde_json::from_value::<SessionHeader>(value.clone())
            .map(FileEntry::Session)
            .unwrap_or(FileEntry::Unparsed(value)),
        Some(
            "message"
            | "thinking_level_change"
            | "model_change"
            | "compaction"
            | "branch_summary"
            | "custom"
            | "custom_message"
            | "context_edit"
            | "label"
            | "session_info"
            | "usage",
        ) => serde_json::from_value::<SessionEntry>(value.clone())
            .map(FileEntry::Entry)
            .unwrap_or(FileEntry::Unparsed(value)),
        _ => FileEntry::Unparsed(value),
    }
}

fn parse_session_entry_line(line: &str) -> Option<FileEntry> {
    if line.trim().is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(line)
        .ok()
        .map(file_entry_from_value)
}

/// Upstream `parseSessionEntries`.
pub fn parse_session_entries(content: &str) -> Vec<FileEntry> {
    content
        .trim()
        .split('\n')
        .filter_map(parse_session_entry_line)
        .collect()
}

/// Upstream `migrateSessionEntries` (exported for testing).
pub fn migrate_session_entries(entries: &mut [FileEntry]) {
    migrate_to_current_version(entries);
}

/// Upstream `migrateToCurrentVersion`: returns true when a migration ran.
fn migrate_to_current_version(entries: &mut [FileEntry]) -> bool {
    let version = entries
        .iter()
        .find_map(|entry| match entry {
            FileEntry::Session(header) => Some(header.version.unwrap_or(1)),
            _ => None,
        })
        .unwrap_or(1);
    if version >= CURRENT_SESSION_VERSION {
        return false;
    }
    if version < 2 {
        migrate_v1_to_v2(entries);
    }
    if version < 3 {
        migrate_v2_to_v3(entries);
    }
    true
}

/// v4-shaped random UUID (upstream `crypto.randomUUID`, S6).
fn random_uuid() -> String {
    let bytes: [u8; 16] = rand::random();
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let variant = format!("{:02x}", (bytes[8] & 0x3f) | 0x80);
    format!(
        "{}-{}-4{}-{}{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[7..8].concat(),
        variant,
        hex[9..10].concat(),
        hex[10..16].concat()
    )
}

/// Upstream `generateId`: unique short id (8 hex chars, collision-checked).
fn generate_id_excluding(is_used: &dyn Fn(&str) -> bool) -> String {
    for _ in 0..100 {
        let id = mint_entry_id();
        if !is_used(&id) {
            return id;
        }
    }
    // Fallback to full UUID if somehow we have collisions.
    random_uuid()
}

/// Upstream `createSessionId` (pi-ai `uuidv7`).
fn create_session_id() -> String {
    crate::ai::uuid::uuid_v7()
}

// ---------------------------------------------------------------------------
// Deterministic test seams (see module docs): when enabled, entry ids mint as
// "00000001", "00000002", ... and session ids as "@u1", "@u2", ... matching
// the oracle's node stubs. Thread-local so parallel test functions never
// interfere; each scenario resets before use.
// ---------------------------------------------------------------------------

#[cfg(test)]
thread_local! {
    static ID_SEAM: std::cell::RefCell<Option<(u32, u32)>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) mod test_id_seam {
    use super::ID_SEAM;

    /// Enable and reset both counters (call at the top of each scenario).
    pub fn reset() {
        ID_SEAM.with(|seam| *seam.borrow_mut() = Some((0, 0)));
    }

    /// Disable the seam (default generator behavior).
    pub fn disable() {
        ID_SEAM.with(|seam| *seam.borrow_mut() = None);
    }

    pub fn next_session_id() -> String {
        ID_SEAM.with(|seam| {
            let mut seam = seam.borrow_mut();
            let Some((sessions, _)) = seam.as_mut() else {
                unreachable!("seam queried while disabled");
            };
            *sessions += 1;
            format!("@u{}", *sessions)
        })
    }

    pub fn next_entry_id() -> String {
        ID_SEAM.with(|seam| {
            let mut seam = seam.borrow_mut();
            let Some((_, entries)) = seam.as_mut() else {
                unreachable!("seam queried while disabled");
            };
            *entries += 1;
            format!("{:08}", *entries)
        })
    }
}

#[cfg(test)]
fn mint_session_id() -> String {
    if ID_SEAM.with(|seam| seam.borrow().is_some()) {
        test_id_seam::next_session_id()
    } else {
        create_session_id()
    }
}

#[cfg(test)]
fn mint_entry_id() -> String {
    if ID_SEAM.with(|seam| seam.borrow().is_some()) {
        test_id_seam::next_entry_id()
    } else {
        random_uuid()[..8].to_string()
    }
}

#[cfg(not(test))]
fn mint_session_id() -> String {
    create_session_id()
}

#[cfg(not(test))]
fn mint_entry_id() -> String {
    random_uuid()[..8].to_string()
}

/// Current wall-clock milliseconds.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Upstream `new Date().toISOString()`.
fn now_iso() -> String {
    crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(now_ms())
}

fn process_cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string())
}

/// node's `path.join` on the host platform.
fn path_join(args: &[&str]) -> String {
    if cfg!(windows) {
        node_path::win32_join(args)
    } else {
        node_path::posix_join(args)
    }
}

/// node's `path.resolve` on the host platform.
fn node_resolve(args: &[&str]) -> String {
    if cfg!(windows) {
        node_path::win32_resolve(args, &process_cwd())
    } else {
        // node posix.resolve falls back to posixCwd(), which strips the
        // drive and converts separators on a Windows host.
        let raw = process_cwd().replace('\\', "/");
        let index = raw.find('/').unwrap_or(0);
        node_path::posix_resolve(args, &raw[index.min(raw.len())..])
    }
}

fn path_exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

fn file_size(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn file_mtime_ms(path: &str) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

/// Upstream `statSync(path).mtimeMs`: sub-millisecond double precision, used
/// by the mtime-descending discovery sorts.
fn file_mtime_ms_f64(path: &str) -> Option<f64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
}

/// Upstream `getDefaultSessionDirPath`.
fn get_default_session_dir_path(cwd: &str, agent_dir: &str) -> String {
    let resolved_cwd = resolve_path_auto_base(cwd).unwrap_or_else(|_| cwd.to_string());
    let resolved_agent_dir =
        resolve_path_auto_base(agent_dir).unwrap_or_else(|_| agent_dir.to_string());
    let stripped = resolved_cwd
        .strip_prefix(['/', '\\'])
        .unwrap_or(&resolved_cwd);
    let safe_path = format!("--{}--", stripped.replace(['/', '\\', ':'], "-"));
    path_join(&[&resolved_agent_dir, "sessions", &safe_path])
}

/// Upstream `getDefaultSessionDir(cwd)` (creates the directory).
pub fn get_default_session_dir(cwd: &str) -> String {
    get_default_session_dir_with(cwd, &get_agent_dir())
}

/// Upstream `getDefaultSessionDir(cwd, agentDir)` overload.
pub fn get_default_session_dir_with(cwd: &str, agent_dir: &str) -> String {
    let session_dir = get_default_session_dir_path(cwd, agent_dir);
    if !path_exists(&session_dir) {
        let _ = std::fs::create_dir_all(&session_dir);
    }
    session_dir
}

/// Upstream `assertValidSessionId`.
pub fn assert_valid_session_id(id: &str) -> Result<(), SessionManagerError> {
    let valid = match id.as_bytes() {
        [] => false,
        [only] => only.is_ascii_alphanumeric(),
        bytes => {
            bytes[0].is_ascii_alphanumeric()
                && bytes[bytes.len() - 1].is_ascii_alphanumeric()
                && bytes
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        }
    };
    if !valid {
        return Err(err(
            "Session id must be non-empty, contain only alphanumeric characters, '-', '_', and '.', and start and end with an alphanumeric character",
        ));
    }
    Ok(())
}

/// Upstream `migrateV1ToV2`: add id/parentId tree structure.
fn migrate_v1_to_v2(entries: &mut [FileEntry]) {
    let mut used: Vec<String> = Vec::new();
    let mut prev_id: Option<String> = None;

    for index in 0..entries.len() {
        if entries[index].is_typed_session() {
            if let FileEntry::Session(header) = &mut entries[index] {
                header.version = Some(2);
            }
            continue;
        }

        let id =
            generate_id_excluding(&|candidate| used.iter().any(|used_id| used_id == candidate));
        used.push(id.clone());
        let parent_id = prev_id.clone();
        match &mut entries[index] {
            FileEntry::Entry(typed) => {
                typed.set_id(id.clone());
                typed.set_parent_id(parent_id.clone());
            }
            FileEntry::Unparsed(value) => {
                value["id"] = Value::String(id.clone());
                value["parentId"] = match parent_id {
                    Some(ref parent) => Value::String(parent.clone()),
                    None => Value::Null,
                };
            }
            FileEntry::Session(_) => unreachable!(),
        }
        prev_id = Some(id);

        // Convert firstKeptEntryIndex to firstKeptEntryId for compaction.
        let first_kept_entry_index = match &entries[index] {
            FileEntry::Entry(SessionEntry::Compaction(compaction)) => {
                compaction.first_kept_entry_index
            }
            FileEntry::Unparsed(value)
                if value.get("type").and_then(Value::as_str) == Some("compaction") =>
            {
                value.get("firstKeptEntryIndex").and_then(Value::as_i64)
            }
            _ => None,
        };
        if let Some(first_kept_entry_index) = first_kept_entry_index {
            let target_id = entries
                .get(first_kept_entry_index as usize)
                .filter(|target| target.type_name() != Some("session"))
                .and_then(|target| match target {
                    FileEntry::Entry(typed) => typed.id().map(str::to_string),
                    FileEntry::Unparsed(value) => {
                        value.get("id").and_then(Value::as_str).map(str::to_string)
                    }
                    FileEntry::Session(_) => None,
                });
            match &mut entries[index] {
                FileEntry::Entry(SessionEntry::Compaction(compaction)) => {
                    if let Some(target_id) = target_id {
                        compaction.first_kept_entry_id = Some(target_id);
                    }
                    compaction.first_kept_entry_index = None;
                }
                FileEntry::Unparsed(value) => {
                    if let Some(target_id) = target_id {
                        value["firstKeptEntryId"] = Value::String(target_id);
                    }
                    if let Some(object) = value.as_object_mut() {
                        object.shift_remove("firstKeptEntryIndex");
                    }
                }
                _ => {}
            }
        }
    }
}

/// Upstream `migrateV2ToV3`: rename hookMessage role to custom.
fn migrate_v2_to_v3(entries: &mut [FileEntry]) {
    for entry in entries.iter_mut() {
        match entry {
            FileEntry::Session(header) => header.version = Some(3),
            FileEntry::Entry(SessionEntry::Message(message)) => {
                if let AgentMessage::Custom(custom) = &mut message.message {
                    if custom.role == "hookMessage" {
                        custom.role = "custom".to_string();
                    }
                }
            }
            FileEntry::Unparsed(value)
                if value.get("type").and_then(Value::as_str) == Some("message") =>
            {
                let role = value
                    .get("message")
                    .and_then(|message| message.get("role"))
                    .and_then(Value::as_str);
                if role == Some("hookMessage") {
                    value["message"]["role"] = Value::String("custom".to_string());
                }
            }
            _ => {}
        }
    }
}

/// Upstream `getLatestCompactionEntry`.
pub fn get_latest_compaction_entry(entries: &[SessionEntry]) -> Option<&SessionEntry> {
    entries
        .iter()
        .rev()
        .find(|entry| matches!(entry, SessionEntry::Compaction(_)))
}

/// Insertion-ordered map standing in for JS `Map` (iteration order feeds the
/// label rewriting and children-order behavior).
#[derive(Debug, Default)]
struct OrderedMap<V> {
    entries: Vec<(String, V)>,
    index: HashMap<String, usize>,
}

impl<V> OrderedMap<V> {
    fn get(&self, key: &str) -> Option<&V> {
        self.index
            .get(key)
            .map(|&position| &self.entries[position].1)
    }

    fn contains_key(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    /// JS `Map.set`: inserting an existing key keeps its position.
    fn insert(&mut self, key: String, value: V) {
        match self.index.get(&key) {
            Some(&position) => self.entries[position].1 = value,
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push((key, value));
            }
        }
    }

    fn remove(&mut self, key: &str) {
        if let Some(position) = self.index.remove(key) {
            self.entries.remove(position);
            for later in self.index.values_mut() {
                if *later > position {
                    *later -= 1;
                }
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
    }

    fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }
}

/// Upstream `SessionTreeNode`. Serialization mirrors the JS object shape
/// (`entry`, `children`, `label`, `labelTimestamp`) for oracle comparison.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTreeNode {
    pub entry: SessionEntry,
    pub children: Vec<SessionTreeNode>,
    /// Resolved label for this entry, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Timestamp of the latest label change for this entry, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_timestamp: Option<String>,
}

/// Upstream `SessionContext`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionContext {
    pub messages: Vec<AgentMessage>,
    pub thinking_level: String,
    pub model: Option<SessionContextModel>,
}

/// Upstream `SessionContext.model` (`{provider, modelId} | null`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextModel {
    pub provider: String,
    pub model_id: String,
}

/// Upstream `SessionInfo`. `created`/`modified` are JS `Date`s ported to
/// epoch milliseconds; `created: None` stands in for JS Invalid Date (S7).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub path: String,
    pub id: String,
    /// Working directory where the session was started. Empty for old sessions.
    pub cwd: String,
    /// User-defined display name from session_info entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Path to the parent session (if this session was forked).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_path: Option<String>,
    pub created: Option<i64>,
    pub modified: i64,
    pub message_count: usize,
    pub first_message: String,
    pub all_messages_text: String,
}

/// The upstream leaf argument's three states (`undefined` / `null` / id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafRef<'a> {
    /// Argument omitted: walk from the last entry.
    Unspecified,
    /// Explicit `null`: empty path.
    Null,
    Id(&'a str),
}

impl<'a> LeafRef<'a> {
    fn from_option(leaf_id: Option<&'a str>) -> Self {
        match leaf_id {
            Some(id) => LeafRef::Id(id),
            None => LeafRef::Null,
        }
    }
}

fn build_entry_index(entries: &[SessionEntry]) -> OrderedMap<usize> {
    let mut index = OrderedMap::default();
    for (position, entry) in entries.iter().enumerate() {
        if let Some(id) = entry.id() {
            index.insert(id.to_string(), position);
        }
    }
    index
}

/// Upstream `buildSessionPath`.
fn build_session_path<'a>(
    entries: &'a [SessionEntry],
    leaf_id: LeafRef<'_>,
) -> Vec<&'a SessionEntry> {
    if leaf_id == LeafRef::Null {
        return Vec::new();
    }
    let index = build_entry_index(entries);
    let leaf = match leaf_id {
        LeafRef::Id(id) => index.get(id).and_then(|&position| entries.get(position)),
        _ => None,
    }
    .or_else(|| entries.last());
    let Some(mut current) = leaf else {
        return Vec::new();
    };

    let mut path = Vec::new();
    loop {
        path.push(current);
        let parent = current
            .parent_id()
            .and_then(|parent_id| index.get(parent_id))
            .and_then(|&position| entries.get(position));
        match parent {
            Some(next) => current = next,
            None => break,
        }
    }
    path.reverse();
    path
}

/// Upstream `getSessionContextSettings`.
fn get_session_context_settings(path: &[&SessionEntry]) -> (String, Option<SessionContextModel>) {
    let mut thinking_level = "off".to_string();
    let mut model: Option<SessionContextModel> = None;
    for entry in path {
        match entry {
            SessionEntry::ThinkingLevelChange(change) => {
                thinking_level = change.thinking_level.clone()
            }
            SessionEntry::ModelChange(change) => {
                model = Some(SessionContextModel {
                    provider: change.provider.clone(),
                    model_id: change.model_id.clone(),
                });
            }
            SessionEntry::Message(message) => {
                if let AgentMessage::Assistant(assistant) = &message.message {
                    model = Some(SessionContextModel {
                        provider: assistant.provider.clone(),
                        model_id: assistant.model.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    (thinking_level, model)
}

fn to_custom_agent_message(
    role: &str,
    value: &impl serde::Serialize,
) -> Option<CustomAgentMessage> {
    let value = serde_json::to_value(value).ok()?;
    let data = match value {
        Value::Object(map) => map,
        _ => return None,
    };
    Some(CustomAgentMessage {
        role: role.to_string(),
        data,
    })
}

/// The loose-message projection of the upstream null-content guards (S8): a
/// raw message entry whose `content` is null/missing projects like upstream
/// `{...message, content: "" | []}`; hopeless shapes project to nothing.
fn unparsed_message_to_context_messages(value: &Value) -> Vec<AgentMessage> {
    let mut message = match value.get("message") {
        Some(message) => message.clone(),
        None => return Vec::new(),
    };
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let content_is_null = message.get("content").map(Value::is_null).unwrap_or(true);
    if content_is_null {
        let replacement = match role {
            "system" => Value::String(String::new()),
            "user" | "assistant" | "toolResult" => Value::Array(Vec::new()),
            _ => return Vec::new(),
        };
        if let Some(object) = message.as_object_mut() {
            object.insert("content".to_string(), replacement);
        }
    }
    serde_json::from_value::<AgentMessage>(message)
        .map(|patched| vec![patched])
        .unwrap_or_default()
}

/// Upstream `sessionEntryToContextMessages`: project one selected entry into
/// LLM/runtime messages. Plain custom entries are display/state entries and
/// do not participate in context.
pub fn session_entry_to_context_messages(entry: &SessionEntry) -> Vec<AgentMessage> {
    match entry {
        SessionEntry::Message(message) => vec![message.message.clone()],
        SessionEntry::Unparsed(value) => unparsed_message_to_context_messages(value),
        SessionEntry::CustomMessage(custom) => create_custom_message(
            &custom.custom_type,
            custom
                .content
                .clone()
                .unwrap_or(CustomMessageContent::Blocks(Vec::new())),
            custom.display,
            custom.details.clone(),
            &custom.timestamp,
        )
        .and_then(|message| to_custom_agent_message("custom", &message))
        .map(|mut agent| {
            // upstream: createCustomMessage stores `details: undefined` when
            // absent; JSON projection drops the key instead of emitting null.
            if custom.details.is_none() {
                agent.data.shift_remove("details");
            }
            agent
        })
        .into_iter()
        .map(AgentMessage::Custom)
        .collect(),
        SessionEntry::BranchSummary(summary) if !summary.summary.is_empty() => {
            create_branch_summary_message(&summary.summary, &summary.from_id, &summary.timestamp)
                .and_then(|message| to_custom_agent_message("branchSummary", &message))
                .into_iter()
                .map(AgentMessage::Custom)
                .collect()
        }
        SessionEntry::Compaction(compaction) => {
            let summary = create_compaction_summary_message(
                &compaction.summary,
                compaction.tokens_before,
                &compaction.timestamp,
            )
            .and_then(|message| to_custom_agent_message("compactionSummary", &message));
            match (&compaction.system_message, summary) {
                (Some(system), Some(summary)) => {
                    vec![
                        AgentMessage::System(system.clone()),
                        AgentMessage::Custom(summary),
                    ]
                }
                (None, Some(summary)) => vec![AgentMessage::Custom(summary)],
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

/// Upstream `buildContextEntries`: the active, compaction-aware entry list.
pub fn build_context_entries(entries: &[SessionEntry], leaf_id: LeafRef<'_>) -> Vec<SessionEntry> {
    let path = build_session_path(entries, leaf_id);
    // upstream: the loop keeps the LAST compaction on the path
    let Some(compaction) = path
        .iter()
        .copied()
        .rev()
        .find(|entry| matches!(**entry, SessionEntry::Compaction(_)))
    else {
        return path.into_iter().cloned().collect();
    };
    let SessionEntry::Compaction(compaction_entry) = compaction else {
        unreachable!()
    };
    let Some(compaction_index) = path.iter().position(|entry| entry.id() == compaction.id()) else {
        return path.into_iter().cloned().collect();
    };

    let mut context_entries: Vec<SessionEntry> = vec![compaction.clone()];
    let mut found_first_kept = false;
    for entry in &path[..compaction_index] {
        if entry.id() == compaction_entry.first_kept_entry_id.as_deref() {
            found_first_kept = true;
        }
        let is_system_message = matches!(
            entry,
            SessionEntry::Message(message) if matches!(message.message, AgentMessage::System(_))
        );
        if found_first_kept && !is_system_message {
            context_entries.push((*entry).clone());
        }
    }
    context_entries.extend(
        path[compaction_index + 1..]
            .iter()
            .map(|entry| (*entry).clone()),
    );
    context_entries
}

/// Upstream `ProjectedSessionEntry`: the raw append-only entry that owns a
/// projected contribution, plus the model-visible messages after context
/// edits (empty for state-only entries and omissions).
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedSessionEntry {
    pub source_entry: SessionEntry,
    pub messages: Vec<AgentMessage>,
}

/// Upstream `SessionProjection`: provenance-preserving, compaction-aware
/// model context.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionProjection {
    pub entries: Vec<ProjectedSessionEntry>,
    pub messages: Vec<AgentMessage>,
    pub thinking_level: String,
    pub model: Option<SessionContextModel>,
}

/// Upstream `projectContextEntry`: project one selected entry, applying the
/// latest context edit targeting it. A `None` replacement (upstream `null`)
/// omits the entry's messages entirely.
fn project_context_entry(
    entry: &SessionEntry,
    edit: Option<&ContextEditEntry>,
) -> Vec<AgentMessage> {
    let messages = session_entry_to_context_messages(entry);
    let Some(edit) = edit else {
        return messages;
    };
    let Some(replacement) = &edit.replacement else {
        return Vec::new();
    };
    messages
        .into_iter()
        .map(|message| apply_context_replacement(message, replacement))
        .collect()
}

/// The `{ ...message, content }` assignment of upstream `projectContextEntry`:
/// only user/assistant/toolResult/custom messages take the replacement; a
/// string replacement becomes a single text block for assistant and tool
/// result roles, and is assigned verbatim otherwise.
fn apply_context_replacement(
    message: AgentMessage,
    replacement: &ContextEditReplacement,
) -> AgentMessage {
    match &replacement.content {
        ContextEditableContent::Text(text) => match message {
            AgentMessage::User(mut user) => {
                user.content = StringOrBlocks::Text(text.clone());
                AgentMessage::User(user)
            }
            AgentMessage::Assistant(mut assistant) => {
                assistant.content = vec![AssistantBlock::Text(TextContent {
                    text: text.clone(),
                    text_signature: None,
                })];
                AgentMessage::Assistant(assistant)
            }
            AgentMessage::ToolResult(mut tool_result) => {
                tool_result.content = vec![TextOrImageBlock::Text(TextContent {
                    text: text.clone(),
                    text_signature: None,
                })];
                AgentMessage::ToolResult(tool_result)
            }
            AgentMessage::Custom(mut custom) => {
                custom
                    .data
                    .insert("content".to_string(), Value::String(text.clone()));
                AgentMessage::Custom(custom)
            }
            other => other,
        },
        ContextEditableContent::Blocks(blocks) => match message {
            AgentMessage::User(mut user) => {
                if let Ok(content) = serde_json::from_value::<StringOrBlocks>(blocks.clone()) {
                    user.content = content;
                    return AgentMessage::User(user);
                }
                AgentMessage::User(user)
            }
            AgentMessage::Assistant(mut assistant) => {
                if let Ok(content) = serde_json::from_value::<Vec<AssistantBlock>>(blocks.clone()) {
                    assistant.content = content;
                    return AgentMessage::Assistant(assistant);
                }
                AgentMessage::Assistant(assistant)
            }
            AgentMessage::ToolResult(mut tool_result) => {
                if let Ok(content) = serde_json::from_value::<Vec<TextOrImageBlock>>(blocks.clone())
                {
                    tool_result.content = content;
                    return AgentMessage::ToolResult(tool_result);
                }
                AgentMessage::ToolResult(tool_result)
            }
            AgentMessage::Custom(mut custom) => {
                custom.data.insert("content".to_string(), blocks.clone());
                AgentMessage::Custom(custom)
            }
            other => other,
        },
    }
}

/// Upstream `buildSessionProjection`.
pub fn build_session_projection(
    entries: &[SessionEntry],
    leaf_id: LeafRef<'_>,
) -> SessionProjection {
    let path = build_session_path(entries, leaf_id);
    let (thinking_level, model) = get_session_context_settings(&path);
    let context_entries = build_context_entries(entries, leaf_id);
    let mut edits: OrderedMap<ContextEditEntry> = OrderedMap {
        entries: Vec::new(),
        index: HashMap::new(),
    };
    for entry in &context_entries {
        if let SessionEntry::ContextEdit(edit) = entry {
            edits.insert(edit.target_id.clone(), edit.clone());
        }
    }
    let projected_entries: Vec<ProjectedSessionEntry> = context_entries
        .iter()
        .enumerate()
        .map(|(index, source_entry)| {
            let messages = match source_entry {
                // buildContextEntries() may retain an older compaction entry
                // because its raw ID lies inside the newest retained range.
                // Only the newest compaction at index zero contributes a
                // checkpoint and summary.
                SessionEntry::Compaction(_) if index > 0 => Vec::new(),
                _ => {
                    let edit = source_entry.id().and_then(|id| edits.get(id));
                    project_context_entry(source_entry, edit)
                }
            };
            ProjectedSessionEntry {
                source_entry: source_entry.clone(),
                messages,
            }
        })
        .collect();
    let messages = projected_entries
        .iter()
        .flat_map(|entry| entry.messages.iter().cloned())
        .collect();
    SessionProjection {
        entries: projected_entries,
        messages,
        thinking_level,
        model,
    }
}

/// Upstream `buildSessionContext` (the finalized model context from the
/// canonical session projection).
pub fn build_session_context(entries: &[SessionEntry], leaf_id: LeafRef<'_>) -> SessionContext {
    let projection = build_session_projection(entries, leaf_id);
    SessionContext {
        messages: projection.messages,
        thinking_level: projection.thinking_level,
        model: projection.model,
    }
}

// ---------------------------------------------------------------------------
// Header scanning
// ---------------------------------------------------------------------------

/// Upstream `SessionHeaderScanLimitError` plus raw fs failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionHeaderError {
    Limit(String),
    Io(String),
}

impl std::fmt::Display for SessionHeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionHeaderError::Limit(message) | SessionHeaderError::Io(message) => {
                f.write_str(message)
            }
        }
    }
}

enum HeaderCandidate {
    KeepScanning,
    NotHeader,
    Header(SessionHeader),
}

/// Upstream `parseSessionHeaderCandidate`.
fn parse_session_header_candidate(line: &str) -> HeaderCandidate {
    if line.trim().is_empty() {
        return HeaderCandidate::KeepScanning;
    }
    let Some(entry) = parse_session_entry_line(line) else {
        return HeaderCandidate::KeepScanning;
    };
    match entry {
        FileEntry::Session(header) if header.id.is_some() => HeaderCandidate::Header(header),
        _ => HeaderCandidate::NotHeader,
    }
}

/// Upstream `readSessionHeader`: bounded first-line header discovery.
fn read_session_header(file_path: &str) -> Result<Option<SessionHeader>, SessionHeaderError> {
    let mut file =
        std::fs::File::open(file_path).map_err(|e| SessionHeaderError::Io(e.to_string()))?;
    let mut buffer = vec![0u8; SESSION_HEADER_READ_BUFFER_SIZE];
    let mut line_chunks = String::new();
    let mut scanned_bytes = 0usize;

    while scanned_bytes < MAX_SESSION_HEADER_SCAN_BYTES {
        let read_length = buffer
            .len()
            .min(MAX_SESSION_HEADER_SCAN_BYTES - scanned_bytes);
        let bytes_read = file
            .read(&mut buffer[..read_length])
            .map_err(|e| SessionHeaderError::Io(e.to_string()))?;
        if bytes_read == 0 {
            return match parse_session_header_candidate(&line_chunks) {
                HeaderCandidate::Header(header) => Ok(Some(header)),
                _ => Ok(None),
            };
        }
        scanned_bytes += bytes_read;

        let chunk = String::from_utf8_lossy(&buffer[..bytes_read]).into_owned();
        let mut line_start = 0;
        while let Some(newline_index) = chunk[line_start..].find('\n') {
            let newline_index = line_start + newline_index;
            line_chunks.push_str(&chunk[line_start..newline_index]);
            match parse_session_header_candidate(&line_chunks) {
                HeaderCandidate::Header(header) => return Ok(Some(header)),
                HeaderCandidate::NotHeader => return Ok(None),
                HeaderCandidate::KeepScanning => line_chunks.clear(),
            }
            line_start = newline_index + 1;
        }
        line_chunks.push_str(&chunk[line_start..]);
    }

    // Probe for EOF so a final header without a newline is allowed when it
    // ends exactly at the scan limit.
    let mut probe = [0u8; 1];
    let probe_read = file
        .read(&mut probe)
        .map_err(|e| SessionHeaderError::Io(e.to_string()))?;
    if probe_read == 0 {
        return match parse_session_header_candidate(&line_chunks) {
            HeaderCandidate::Header(header) => Ok(Some(header)),
            _ => Ok(None),
        };
    }
    Err(SessionHeaderError::Limit(format!(
        "Session header exceeds {MAX_SESSION_HEADER_SCAN_BYTES}-byte scan limit: {file_path}"
    )))
}

/// Upstream `readSessionHeaderForDiscovery`: best-effort, never fails.
fn read_session_header_for_discovery(file_path: &str) -> Option<SessionHeader> {
    read_session_header(file_path).ok().flatten()
}

/// Upstream `sessionCwdMatches`.
fn session_cwd_matches(cwd: Option<&str>, resolved_cwd: &str) -> bool {
    match cwd {
        Some(cwd) if !cwd.is_empty() => resolve_path_auto_base(cwd).as_deref() == Ok(resolved_cwd),
        _ => false,
    }
}

fn append_file_text(path: &str, text: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(text.as_bytes())
}

/// Upstream `loadEntriesFromFile` (exported for testing): loads entries and
/// repairs a missing final newline (S4).
pub fn load_entries_from_file(file_path: &str) -> Vec<FileEntry> {
    let resolved_file_path = normalize_path(file_path).unwrap_or_else(|_| file_path.to_string());
    if !path_exists(&resolved_file_path) {
        return Vec::new();
    }

    let Ok(bytes) = std::fs::read(&resolved_file_path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();

    let mut entries = Vec::new();
    let mut pending = "";
    for line in text.split('\n') {
        pending = line;
        if let Some(entry) = parse_session_entry_line(pending) {
            entries.push(entry);
        }
    }

    // Validate session header before repairing the file.
    if entries.is_empty() {
        return entries;
    }
    let header_valid = matches!(&entries[0], FileEntry::Session(header) if header.id.is_some());
    if !header_valid {
        return Vec::new();
    }

    if !pending.is_empty() {
        let _ = append_file_text(&resolved_file_path, "\n");
    }
    entries
}

/// Upstream `extractTextContent` for typed messages.
fn extract_text_from_string_or_blocks(content: &StringOrBlocks) -> String {
    match content {
        StringOrBlocks::Text(text) => text.clone(),
        StringOrBlocks::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                TextOrImageBlock::Text(TextContent { text, .. }) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Upstream `getMessageActivityTime` for one parsed (possibly unparsed)
/// message entry.
fn message_activity_time(entry: &FileEntry) -> Option<i64> {
    let role = match entry {
        FileEntry::Entry(SessionEntry::Message(message)) => {
            Some(message.message.role().to_string())
        }
        FileEntry::Unparsed(value)
            if value.get("type").and_then(Value::as_str) == Some("message") =>
        {
            value
                .get("message")
                .and_then(|message| message.get("role"))
                .and_then(Value::as_str)
                .map(str::to_string)
        }
        _ => None,
    }?;
    if role != "user" && role != "assistant" {
        return None;
    }

    let message_timestamp = match entry {
        FileEntry::Entry(SessionEntry::Message(message)) => match &message.message {
            AgentMessage::User(user) => Some(user.timestamp),
            AgentMessage::Assistant(assistant) => Some(assistant.timestamp),
            _ => None,
        },
        FileEntry::Unparsed(value) => value
            .get("message")
            .and_then(|message| message.get("timestamp"))
            .and_then(Value::as_i64),
        _ => None,
    };
    if let Some(timestamp) = message_timestamp {
        return Some(timestamp);
    }

    let entry_timestamp = match entry {
        FileEntry::Entry(SessionEntry::Message(message)) => Some(message.timestamp.as_str()),
        FileEntry::Unparsed(value) => value.get("timestamp").and_then(Value::as_str),
        _ => None,
    };
    entry_timestamp.and_then(parse_epoch_millis)
}

/// The text content of a user/assistant message entry, mirroring upstream
/// `isMessageWithContent` + `extractTextContent` (including crash-parity for
/// null/non-string content, which aborts the whole listing upstream).
enum MessageText {
    Text(String),
    Skip,
    Invalid,
}

fn message_text_content(entry: &FileEntry) -> MessageText {
    if let FileEntry::Entry(SessionEntry::Message(message)) = entry {
        return match &message.message {
            AgentMessage::User(user) => {
                MessageText::Text(extract_text_from_string_or_blocks(&user.content))
            }
            AgentMessage::Assistant(assistant) => MessageText::Text(
                assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::Text(TextContent { text, .. }) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => MessageText::Skip,
        };
    }

    // Unparsed message entry: mirror the loose runtime checks (S8).
    let FileEntry::Unparsed(value) = entry else {
        return MessageText::Skip;
    };
    let Some(message) = value.get("message") else {
        return MessageText::Skip;
    };
    let Some(role) = message.get("role").and_then(Value::as_str) else {
        return MessageText::Skip;
    };
    if role != "user" && role != "assistant" {
        return MessageText::Skip;
    }
    let Some(content) = message.get("content") else {
        return MessageText::Skip;
    };
    match content {
        Value::String(text) => MessageText::Text(text.clone()),
        Value::Array(blocks) => MessageText::Text(
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        // Null or non-string/array content upstream throws inside
        // extractTextContent → buildSessionInfo returns null.
        _ => MessageText::Invalid,
    }
}

/// Upstream `buildSessionInfo`.
fn build_session_info(file_path: &str) -> Option<SessionInfo> {
    std::fs::metadata(file_path).ok()?;
    let bytes = std::fs::read(file_path).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();

    let mut header: Option<SessionHeader> = None;
    let mut message_count = 0usize;
    let mut first_message = String::new();
    let mut all_messages: Vec<String> = Vec::new();
    let mut name: Option<String> = None;
    let mut last_activity_time: Option<i64> = None;

    for line in text.split('\n') {
        let Some(entry) = parse_session_entry_line(line) else {
            continue;
        };

        if header.is_none() {
            header = match &entry {
                FileEntry::Session(parsed) => Some(parsed.clone()),
                // Loose header capture (upstream accepts any entry with
                // `type === "session"` as the header).
                FileEntry::Unparsed(value)
                    if value.get("type").and_then(Value::as_str) == Some("session") =>
                {
                    Some(SessionHeader {
                        version: value
                            .get("version")
                            .and_then(Value::as_u64)
                            .map(|v| v as u32),
                        id: value.get("id").and_then(Value::as_str).map(str::to_string),
                        timestamp: value
                            .get("timestamp")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        cwd: value.get("cwd").and_then(Value::as_str).map(str::to_string),
                        parent_session: value
                            .get("parentSession")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                }
                _ => return None,
            };
            continue;
        }

        // Extract session name (use latest, including explicit clears).
        if let FileEntry::Entry(SessionEntry::SessionInfo(info)) = &entry {
            name = info
                .name
                .as_deref()
                .map(str::trim)
                .filter(|trimmed| !trimmed.is_empty())
                .map(str::to_string);
        }

        if entry.type_name() != Some("message") {
            continue;
        }
        message_count += 1;

        if let Some(activity_time) = message_activity_time(&entry) {
            last_activity_time = Some(
                last_activity_time.map_or(activity_time, |current| current.max(activity_time)),
            );
        }

        let role = match &entry {
            FileEntry::Entry(SessionEntry::Message(message)) => {
                Some(message.message.role().to_string())
            }
            FileEntry::Unparsed(value) => value
                .get("message")
                .and_then(|message| message.get("role"))
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        };
        if !matches!(role.as_deref(), Some("user") | Some("assistant")) {
            continue;
        }

        match message_text_content(&entry) {
            MessageText::Text(text_content) => {
                if !text_content.is_empty() {
                    all_messages.push(text_content.clone());
                    if first_message.is_empty() && role.as_deref() == Some("user") {
                        first_message = text_content;
                    }
                }
            }
            MessageText::Skip => {}
            MessageText::Invalid => return None,
        }
    }

    let header = header?;

    let cwd = header.cwd.clone().unwrap_or_default();
    let parent_session_path = header.parent_session.clone();
    let header_time = header.timestamp.as_deref().and_then(parse_epoch_millis);
    let modified = if last_activity_time.is_some_and(|t| t > 0) {
        last_activity_time.unwrap()
    } else if let Some(header_time) = header_time {
        header_time
    } else {
        file_mtime_ms(file_path).unwrap_or(0)
    };

    Some(SessionInfo {
        path: file_path.to_string(),
        id: header.id.clone().unwrap_or_default(),
        cwd,
        name,
        parent_session_path,
        created: header.timestamp.as_deref().and_then(parse_epoch_millis),
        modified,
        message_count,
        first_message: if first_message.is_empty() {
            "(no messages)".to_string()
        } else {
            first_message
        },
        all_messages_text: all_messages.join(" "),
    })
}

/// Upstream `SessionListProgress`. `partial_sessions` carries the sessions
/// loaded so far, sorted by activity; it is `None` between periodic updates.
/// Boxed filtered-progress callback (type-complexity seam).
type ProgressBox<'a> = Box<dyn FnMut(usize, usize, Option<&[SessionInfo]>) + 'a>;

pub type SessionListProgress<'a> = dyn FnMut(usize, usize, Option<&[SessionInfo]>) + 'a;

/// Upstream `sortSessionInfos` (most recently modified first; stable).
fn sort_session_infos(sessions: &mut [SessionInfo]) {
    sessions.sort_by_key(|a| std::cmp::Reverse(a.modified));
}

/// Upstream `buildSessionInfosWithConcurrency` (sequential, S4): progress
/// fires once per file (including failures); results keep file order.
fn build_session_infos(
    files: &[String],
    on_loaded: &mut dyn FnMut(usize, Option<&SessionInfo>),
) -> Vec<Option<SessionInfo>> {
    files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let info = build_session_info(file);
            on_loaded(index, info.as_ref());
            info
        })
        .collect()
}

/// Upstream `listSessionsFromDir`.
fn list_sessions_from_dir(
    dir: &str,
    mut on_progress: Option<&mut SessionListProgress<'_>>,
) -> Vec<SessionInfo> {
    if !path_exists(dir) {
        return Vec::new();
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<String> = read_dir
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".jsonl"))
        .map(|name| path_join(&[dir, &name]))
        .collect();
    // Upstream sorts the file names descending (`b.localeCompare(a)`); the
    // remaining result-order ties resolve to that order through the final
    // stable mtime sort.
    files.sort_by(|a, b| b.cmp(a));
    let total = files.len();

    const PUBLISH_INTERVAL: usize = 10;
    let mut loaded = 0usize;
    let mut partial_sessions: Vec<SessionInfo> = Vec::new();
    let results = build_session_infos(&files, &mut |_, info| {
        loaded += 1;
        if let Some(info) = info {
            partial_sessions.push(info.clone());
        }
        if let Some(progress) = on_progress.as_deref_mut() {
            let publish_partial =
                loaded == 1 || loaded.is_multiple_of(PUBLISH_INTERVAL) || loaded == files.len();
            progress(
                loaded,
                total,
                publish_partial
                    .then(|| {
                        let mut partial = partial_sessions.clone();
                        sort_session_infos(&mut partial);
                        partial
                    })
                    .as_deref(),
            );
        }
    });
    results.into_iter().flatten().collect()
}

/// Upstream `findMostRecentSession` (exported for testing). All `.jsonl`
/// files are mtime-sorted first; the newest one with a readable header (and,
/// when given, a matching cwd) wins.
pub fn find_most_recent_session(session_dir: &str, cwd: Option<&str>) -> Option<String> {
    let resolved_session_dir =
        normalize_path(session_dir).unwrap_or_else(|_| session_dir.to_string());
    let resolved_cwd = cwd.and_then(|cwd| resolve_path_auto_base(cwd).ok());
    let Ok(read_dir) = std::fs::read_dir(&resolved_session_dir) else {
        return None;
    };

    let mut files: Vec<(String, f64)> = Vec::new();
    for entry in read_dir.filter_map(|entry| entry.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let path = path_join(&[&resolved_session_dir, &name]);
        let Some(mtime) = file_mtime_ms_f64(&path) else {
            // statSync throwing fails the whole upstream discovery.
            return None;
        };
        files.push((path, mtime));
    }

    // Same tie determinism as list_sessions_from_dir: name-sort the base
    // order so ties resolve to the capture machine's enumeration
    // deterministically on every filesystem, then the stable mtime-descending
    // sort on top.
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.sort_by(|a, b| b.1.total_cmp(&a.1));

    for (path, _) in files {
        let Some(header) = read_session_header_for_discovery(&path) else {
            continue;
        };
        if let Some(resolved_cwd) = &resolved_cwd {
            if !session_cwd_matches(header.cwd.as_deref(), resolved_cwd) {
                continue;
            }
        }
        return Some(path);
    }
    None
}

/// Upstream `SessionManager`: manages conversation sessions as append-only
/// trees stored in JSONL files.
#[derive(Debug)]
pub struct SessionManager {
    session_id: String,
    session_file: Option<String>,
    session_dir: String,
    cwd: String,
    persist: bool,
    flushed: bool,
    file_entries: Vec<FileEntry>,
    by_id: OrderedMap<usize>,
    labels_by_id: OrderedMap<String>,
    label_timestamps_by_id: OrderedMap<String>,
    leaf_id: Option<String>,
}

impl SessionManager {
    fn new(
        cwd: &str,
        session_dir: &str,
        session_file: Option<&str>,
        persist: bool,
        new_session_options: Option<&NewSessionOptions>,
        preloaded_file_entries: Option<Vec<FileEntry>>,
    ) -> Result<Self, SessionManagerError> {
        let mut manager = Self {
            session_id: String::new(),
            session_file: None,
            session_dir: normalize_path(session_dir).unwrap_or_else(|_| session_dir.to_string()),
            cwd: resolve_path_auto_base(cwd)?,
            persist,
            flushed: false,
            file_entries: Vec::new(),
            by_id: OrderedMap::default(),
            labels_by_id: OrderedMap::default(),
            label_timestamps_by_id: OrderedMap::default(),
            leaf_id: None,
        };
        if persist && !manager.session_dir.is_empty() && !path_exists(&manager.session_dir) {
            std::fs::create_dir_all(&manager.session_dir)?;
        }

        if let Some(session_file) = session_file {
            manager.set_session_file_internal(session_file, preloaded_file_entries)?;
        } else if preloaded_file_entries
            .as_ref()
            .is_some_and(|entries| !entries.is_empty())
        {
            manager.load_entries(
                preloaded_file_entries.unwrap_or_default(),
                new_session_options,
            )?;
        } else {
            manager.new_session(new_session_options)?;
        }
        Ok(manager)
    }

    /// Switch to a different session file (used for resume and branching).
    pub fn set_session_file(&mut self, session_file: &str) -> Result<(), SessionManagerError> {
        self.set_session_file_internal(session_file, None)
    }

    fn set_session_file_internal(
        &mut self,
        session_file: &str,
        preloaded_file_entries: Option<Vec<FileEntry>>,
    ) -> Result<(), SessionManagerError> {
        let resolved = resolve_path_auto_base(session_file)?;
        self.session_file = Some(resolved.clone());
        if path_exists(&resolved) {
            let entries = match preloaded_file_entries {
                Some(entries) => entries,
                None => load_entries_from_file(&resolved),
            };

            // If file was empty, initialize it with a valid session header. If
            // it was non-empty but did not parse as a pi session, fail without
            // modifying it.
            if entries.is_empty() {
                if file_size(&resolved) > 0 {
                    return Err(err(format!(
                        "Session file is not a valid {APP_NAME} session: {resolved}"
                    )));
                }
                self.new_session(None)?;
                self.session_file = Some(resolved);
                self.rewrite_file()?;
                self.flushed = true;
                return Ok(());
            }

            self.load_entries(entries, None)?;
            self.flushed = true;
        } else {
            self.new_session(None)?;
            self.session_file = Some(resolved); // preserve explicit path from --session flag
        }
        Ok(())
    }

    /// Upstream `newSession`. Returns the session file when persisting.
    pub fn new_session(
        &mut self,
        options: Option<&NewSessionOptions>,
    ) -> Result<Option<String>, SessionManagerError> {
        if let Some(id) = options.and_then(|options| options.id.as_deref()) {
            assert_valid_session_id(id)?;
        }
        self.session_id = options
            .and_then(|options| options.id.clone())
            .unwrap_or_else(mint_session_id);
        let timestamp = now_iso();
        self.file_entries = vec![FileEntry::Session(SessionHeader {
            version: Some(CURRENT_SESSION_VERSION),
            id: Some(self.session_id.clone()),
            timestamp: Some(timestamp.clone()),
            cwd: Some(self.cwd.clone()),
            parent_session: options.and_then(|options| options.parent_session.clone()),
        })];
        self.by_id.clear();
        self.labels_by_id.clear();
        self.label_timestamps_by_id.clear();
        self.leaf_id = None;
        self.flushed = false;

        if self.persist {
            let file_timestamp = timestamp.replace([':', '.'], "-");
            self.session_file = Some(path_join(&[
                self.get_session_dir(),
                &format!("{file_timestamp}_{}.jsonl", self.session_id),
            ]));
        }
        Ok(self.session_file.clone())
    }

    fn load_entries(
        &mut self,
        entries: Vec<FileEntry>,
        options: Option<&NewSessionOptions>,
    ) -> Result<(), SessionManagerError> {
        let header_index = entries.iter().position(FileEntry::is_typed_session);
        if let Some(header_index) = header_index {
            self.session_id = match &entries[header_index] {
                FileEntry::Session(header) => header.id.clone().unwrap_or_default(),
                _ => unreachable!(),
            };
            self.file_entries = entries;
            if migrate_to_current_version(&mut self.file_entries) {
                self.rewrite_file()?;
            }
        } else {
            self.new_session(options)?;
            self.file_entries.extend(entries);
        }
        self.build_index();
        Ok(())
    }

    fn build_index(&mut self) {
        self.by_id.clear();
        self.labels_by_id.clear();
        self.label_timestamps_by_id.clear();
        self.leaf_id = None;
        for (position, entry) in self.file_entries.iter().enumerate() {
            let FileEntry::Entry(typed) = entry else {
                continue;
            };
            let Some(id) = typed.id() else {
                continue;
            };
            self.by_id.insert(id.to_string(), position);
            self.leaf_id = Some(id.to_string());
            if let SessionEntry::Label(label) = typed {
                if let Some(label_text) = label.label.clone() {
                    self.labels_by_id
                        .insert(label.target_id.clone(), label_text);
                    self.label_timestamps_by_id
                        .insert(label.target_id.clone(), label.timestamp.clone());
                } else {
                    self.labels_by_id.remove(&label.target_id);
                    self.label_timestamps_by_id.remove(&label.target_id);
                }
            }
        }
    }

    fn serialize_entry_at(&self, position: usize) -> String {
        let mut line =
            serde_json::to_string(&self.file_entries[position]).expect("entry serializes");
        line.push('\n');
        line
    }

    fn rewrite_file(&self) -> Result<(), SessionManagerError> {
        if !self.persist {
            return Ok(());
        }
        let Some(session_file) = &self.session_file else {
            return Ok(());
        };
        let mut file = std::fs::File::create(session_file)?;
        for position in 0..self.file_entries.len() {
            file.write_all(self.serialize_entry_at(position).as_bytes())?;
        }
        Ok(())
    }

    pub fn is_persisted(&self) -> bool {
        self.persist
    }

    pub fn get_cwd(&self) -> &str {
        &self.cwd
    }

    pub fn get_session_dir(&self) -> &str {
        &self.session_dir
    }

    pub fn uses_default_session_dir(&self) -> bool {
        self.session_dir == get_default_session_dir_path(&self.cwd, &get_agent_dir())
    }

    pub fn get_session_id(&self) -> &str {
        &self.session_id
    }

    pub fn get_session_file(&self) -> Option<&str> {
        self.session_file.as_deref()
    }

    /// Upstream `_hasConversation`: a new session file is created only once
    /// the session contains a user or assistant message. Setup entries alone
    /// (model, thinking level, system prompt) stay in memory so opening and
    /// closing pi without chatting leaves no file behind. Starting at the user
    /// message (not the first assistant reply) keeps the prompt on disk if the
    /// first turn never completes (#10000).
    fn has_conversation(&self) -> bool {
        self.file_entries.iter().any(|entry| match entry {
            FileEntry::Entry(SessionEntry::Message(message)) => {
                matches!(
                    message.message,
                    AgentMessage::User(_) | AgentMessage::Assistant(_)
                )
            }
            FileEntry::Unparsed(value) => {
                value.get("type").and_then(Value::as_str) == Some("message")
                    && matches!(
                        value
                            .get("message")
                            .and_then(|message| message.get("role"))
                            .and_then(Value::as_str),
                        Some("user") | Some("assistant")
                    )
            }
            _ => false,
        })
    }

    /// Upstream `_persist`.
    fn persist_entry(&mut self) -> Result<(), SessionManagerError> {
        if !self.persist {
            return Ok(());
        }
        let Some(session_file) = &self.session_file else {
            return Ok(());
        };
        let last_position = self.file_entries.len() - 1;
        let line = self.serialize_entry_at(last_position);

        if !self.flushed {
            if !self.has_conversation() {
                return Ok(());
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(session_file)?;
            for position in 0..self.file_entries.len() {
                file.write_all(self.serialize_entry_at(position).as_bytes())?;
            }
            self.flushed = true;
        } else {
            append_file_text(session_file, &line)?;
        }
        Ok(())
    }

    fn append_entry(&mut self, entry: SessionEntry) -> Result<String, SessionManagerError> {
        let id = entry.id().map(str::to_string);
        self.file_entries.push(FileEntry::Entry(entry));
        if let Some(id) = &id {
            self.by_id.insert(id.clone(), self.file_entries.len() - 1);
            self.leaf_id = Some(id.clone());
        }
        self.persist_entry()?;
        Ok(id.unwrap_or_default())
    }

    /// Upstream `appendMessage`: append a message as child of the current
    /// leaf, then advance the leaf.
    pub fn append_message(&mut self, message: AgentMessage) -> Result<String, SessionManagerError> {
        let entry = SessionEntry::Message(MessageEntry {
            id: mint_entry_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            message,
        });
        self.append_entry(entry)
    }

    /// Upstream `appendThinkingLevelChange`.
    pub fn append_thinking_level_change(
        &mut self,
        thinking_level: &str,
    ) -> Result<String, SessionManagerError> {
        let entry = SessionEntry::ThinkingLevelChange(ThinkingLevelChangeEntry {
            id: mint_entry_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            thinking_level: thinking_level.to_string(),
        });
        self.append_entry(entry)
    }

    /// Upstream `appendModelChange`.
    pub fn append_model_change(
        &mut self,
        provider: &str,
        model_id: &str,
    ) -> Result<String, SessionManagerError> {
        let entry = SessionEntry::ModelChange(ModelChangeEntry {
            id: mint_entry_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            provider: provider.to_string(),
            model_id: model_id.to_string(),
        });
        self.append_entry(entry)
    }

    /// Upstream `appendCompaction`: append a compaction summary as child of
    /// the current leaf, then advance the leaf. `first_kept_entry_id: None`
    /// (upstream `null`) records the compaction entry itself. Returns the
    /// entry id.
    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: Option<&str>,
        tokens_before: i64,
        details: Option<Value>,
        from_hook: Option<bool>,
        usage: Option<Usage>,
    ) -> Result<String, SessionManagerError> {
        let timestamp = now_iso();
        let projection = self.build_session_projection();
        let context_messages: Vec<Message> = projection
            .messages
            .iter()
            .filter_map(AgentMessage::to_message)
            .collect();
        let system_message = get_current_system_message(&context_messages).map(|mut system| {
            system.timestamp = parse_epoch_millis(&timestamp).unwrap_or(0);
            system
        });
        let id = mint_entry_id();
        let entry = SessionEntry::Compaction(CompactionEntry {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp,
            summary: summary.to_string(),
            first_kept_entry_id: Some(
                first_kept_entry_id
                    .map(str::to_string)
                    .unwrap_or_else(|| id.clone()),
            ),
            tokens_before,
            details,
            usage,
            from_hook,
            system_message,
            first_kept_entry_index: None,
        });
        self.append_entry(entry)
    }

    /// Upstream `appendUsage`: append model-attributed usage that does not
    /// participate in LLM context. Returns the appended entry id.
    pub fn append_usage(
        &mut self,
        kind: &str,
        provider: &str,
        model: &str,
        usage: Usage,
        note: Option<&str>,
    ) -> Result<String, SessionManagerError> {
        // Upstream spreads `...(note ? { note } : {})`: an empty note is absent.
        let note = note.filter(|note| !note.is_empty()).map(str::to_string);
        let entry = SessionEntry::Usage(UsageEntry {
            id: mint_entry_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            kind: kind.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            usage,
            note,
        });
        self.append_entry(entry)
    }

    /// Upstream `appendContextEdit`: append a branch-local edit to an earlier
    /// model-visible entry. Returns the entry id.
    pub fn append_context_edit(
        &mut self,
        target_id: &str,
        replacement: Option<ContextEditReplacement>,
    ) -> Result<String, SessionManagerError> {
        if let Some(replacement) = &replacement {
            let valid = match &replacement.content {
                ContextEditableContent::Text(_) => true,
                ContextEditableContent::Blocks(blocks) => blocks.is_array(),
            };
            if !valid {
                return Err(err(
                    "Context edit replacement must be null or contain string/array content",
                ));
            }
        }
        if !self.by_id.contains_key(target_id) {
            return Err(err(format!("Entry {target_id} not found")));
        }
        if !self
            .get_branch(None)
            .iter()
            .any(|entry| entry.id() == Some(target_id))
        {
            return Err(err(format!(
                "Entry {target_id} is not on the active branch"
            )));
        }
        let target_role = match self.get_entry(target_id) {
            Some(SessionEntry::CustomMessage(_)) => Some("custom"),
            Some(SessionEntry::Message(message)) => Some(message.message.role()),
            _ => None,
        };
        let editable = matches!(
            target_role,
            Some("user") | Some("assistant") | Some("toolResult") | Some("custom")
        );
        if !editable {
            return Err(err(format!(
                "Entry {target_id} does not contribute editable model content"
            )));
        }
        // A string replacement becomes a single text block for assistant and
        // tool result roles.
        let normalized_replacement = replacement.map(|replacement| {
            let is_text_role = matches!(target_role, Some("assistant") | Some("toolResult"));
            match (&replacement.content, is_text_role) {
                (ContextEditableContent::Text(text), true) => ContextEditReplacement {
                    content: ContextEditableContent::Blocks(serde_json::json!([
                        { "type": "text", "text": text }
                    ])),
                },
                _ => replacement,
            }
        });
        let entry = SessionEntry::ContextEdit(ContextEditEntry {
            id: mint_entry_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            target_id: target_id.to_string(),
            replacement: normalized_replacement,
        });
        self.append_entry(entry)
    }

    /// Upstream `appendCustomEntry`.
    pub fn append_custom_entry(
        &mut self,
        custom_type: &str,
        data: Option<Value>,
    ) -> Result<String, SessionManagerError> {
        let entry = SessionEntry::Custom(CustomEntry {
            custom_type: custom_type.to_string(),
            data,
            id: mint_entry_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
        });
        self.append_entry(entry)
    }

    /// Upstream `appendSessionInfo`.
    pub fn append_session_info(&mut self, name: &str) -> Result<String, SessionManagerError> {
        // name.replace(/[\r\n]+/g, " ").trim()
        let mut sanitized = String::with_capacity(name.len());
        let mut previous_was_newline = false;
        for character in name.chars() {
            if character == '\r' || character == '\n' {
                if !previous_was_newline {
                    sanitized.push(' ');
                    previous_was_newline = true;
                }
            } else {
                sanitized.push(character);
                previous_was_newline = false;
            }
        }
        let sanitized_name =
            crate::coding_agent::utils::text::trim_js_whitespace(&sanitized).to_string();
        let entry = SessionEntry::SessionInfo(SessionInfoEntry {
            id: mint_entry_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            name: Some(sanitized_name),
        });
        self.append_entry(entry)
    }

    /// Upstream `getSessionName`: the latest session_info entry (empty names
    /// explicitly clear the session title). Reads `fileEntries` directly: the
    /// footer calls this on every frame, and `getEntries()` copies the whole
    /// session.
    pub fn get_session_name(&self) -> Option<String> {
        self.file_entries
            .iter()
            .rev()
            .filter_map(|entry| match entry {
                FileEntry::Entry(SessionEntry::SessionInfo(info)) => Some(info),
                _ => None,
            })
            .next()
            .and_then(|info| {
                info.name
                    .as_deref()
                    .map(crate::coding_agent::utils::text::trim_js_whitespace)
                    .filter(|trimmed| !trimmed.is_empty())
                    .map(str::to_string)
            })
    }

    /// Upstream `appendCustomMessageEntry`.
    pub fn append_custom_message_entry(
        &mut self,
        custom_type: &str,
        content: CustomMessageContent,
        display: bool,
        details: Option<Value>,
    ) -> Result<String, SessionManagerError> {
        let entry = SessionEntry::CustomMessage(CustomMessageEntry {
            custom_type: custom_type.to_string(),
            content: Some(content),
            display,
            details,
            id: mint_entry_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
        });
        self.append_entry(entry)
    }

    // =========================================================================
    // Tree Traversal
    // =========================================================================

    pub fn get_leaf_id(&self) -> Option<&str> {
        self.leaf_id.as_deref()
    }

    pub fn get_leaf_entry(&self) -> Option<&SessionEntry> {
        let id = self.leaf_id.as_deref()?;
        self.get_entry(id)
    }

    pub fn get_entry(&self, id: &str) -> Option<&SessionEntry> {
        let &position = self.by_id.get(id)?;
        match &self.file_entries[position] {
            FileEntry::Entry(entry) => Some(entry),
            _ => None,
        }
    }

    /// Upstream `getChildren`: direct children in index (insertion) order.
    pub fn get_children(&self, parent_id: &str) -> Vec<SessionEntry> {
        self.by_id
            .iter()
            .filter_map(|(id, _)| self.get_entry(id))
            .filter(|entry| entry.parent_id() == Some(parent_id))
            .cloned()
            .collect()
    }

    /// Upstream `getLabel`.
    pub fn get_label(&self, id: &str) -> Option<&str> {
        self.labels_by_id.get(id).map(String::as_str)
    }

    /// Upstream `appendLabelChange`: set or clear a label on an entry.
    pub fn append_label_change(
        &mut self,
        target_id: &str,
        label: Option<&str>,
    ) -> Result<String, SessionManagerError> {
        if !self.by_id.contains_key(target_id) {
            return Err(err(format!("Entry {target_id} not found")));
        }
        let timestamp = now_iso();
        let entry = SessionEntry::Label(LabelEntry {
            id: mint_entry_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: timestamp.clone(),
            target_id: target_id.to_string(),
            label: label.map(str::to_string),
        });
        let entry_id = self.append_entry(entry)?;
        match label {
            Some(label) if !label.is_empty() => {
                self.labels_by_id
                    .insert(target_id.to_string(), label.to_string());
                self.label_timestamps_by_id
                    .insert(target_id.to_string(), timestamp);
            }
            _ => {
                self.labels_by_id.remove(target_id);
                self.label_timestamps_by_id.remove(target_id);
            }
        }
        Ok(entry_id)
    }

    /// Upstream `getBranch`: walk from entry to root, in path order.
    pub fn get_branch(&self, from_id: Option<&str>) -> Vec<SessionEntry> {
        let start_id = from_id.or(self.leaf_id.as_deref());
        let mut path = Vec::new();
        let mut current = start_id.and_then(|id| self.get_entry(id));
        while let Some(entry) = current {
            path.push(entry.clone());
            current = entry
                .parent_id()
                .and_then(|parent_id| self.get_entry(parent_id));
        }
        path.reverse();
        path
    }

    /// Upstream `buildContextEntries` method form.
    pub fn build_context_entries(&self) -> Vec<SessionEntry> {
        build_context_entries(
            &self.get_entries(),
            LeafRef::from_option(self.leaf_id.as_deref()),
        )
    }

    /// Upstream `buildSessionProjection` method form.
    pub fn build_session_projection(&self) -> SessionProjection {
        build_session_projection(
            &self.get_entries(),
            LeafRef::from_option(self.leaf_id.as_deref()),
        )
    }

    /// Upstream `buildSessionContext` method form.
    pub fn build_session_context(&self) -> SessionContext {
        build_session_context(
            &self.get_entries(),
            LeafRef::from_option(self.leaf_id.as_deref()),
        )
    }

    /// Upstream `getEntryCount`: number of session entries (excludes the
    /// header), without copying them like `get_entries()`.
    pub fn get_entry_count(&self) -> usize {
        self.by_id.entries.len()
    }

    /// Upstream `getHeader`.
    pub fn get_header(&self) -> Option<SessionHeader> {
        self.file_entries.iter().find_map(|entry| match entry {
            FileEntry::Session(header) => Some(header.clone()),
            FileEntry::Unparsed(value)
                if value.get("type").and_then(Value::as_str) == Some("session") =>
            {
                Some(SessionHeader {
                    version: value
                        .get("version")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32),
                    id: value.get("id").and_then(Value::as_str).map(str::to_string),
                    timestamp: value
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    cwd: value.get("cwd").and_then(Value::as_str).map(str::to_string),
                    parent_session: value
                        .get("parentSession")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            }
            _ => None,
        })
    }

    /// Upstream `getEntries`: all session entries (excluding the header).
    pub fn get_entries(&self) -> Vec<SessionEntry> {
        self.file_entries
            .iter()
            .filter_map(|entry| match entry {
                FileEntry::Entry(typed) => Some(typed.clone()),
                FileEntry::Unparsed(value)
                    if value.get("type").and_then(Value::as_str) != Some("session") =>
                {
                    Some(SessionEntry::Unparsed(value.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// Upstream `getTree`: the session as a tree structure (orphans become
    /// roots; children sorted oldest-first, iteratively).
    pub fn get_tree(&self) -> Vec<SessionTreeNode> {
        let entries = self.get_entries();
        let mut node_map: HashMap<String, usize> = HashMap::new();
        let mut entry_of: Vec<SessionEntry> = Vec::with_capacity(entries.len());
        let mut children_of: Vec<Vec<usize>> = Vec::with_capacity(entries.len());
        let mut roots: Vec<usize> = Vec::new();

        for entry in &entries {
            if let Some(id) = entry.id() {
                node_map.insert(id.to_string(), entry_of.len());
            }
            entry_of.push(entry.clone());
            children_of.push(Vec::new());
        }

        for (position, entry) in entries.iter().enumerate() {
            let is_root = match entry.parent_id() {
                None => true,
                Some(parent_id) => {
                    if Some(parent_id) == entry.id() {
                        true
                    } else {
                        match node_map.get(parent_id) {
                            Some(&parent_position) => {
                                children_of[parent_position].push(position);
                                false
                            }
                            None => true,
                        }
                    }
                }
            };
            if is_root {
                roots.push(position);
            }
        }

        // Sort children by timestamp (oldest first) at every node, bottom-up.
        let ordered_timestamp =
            |position: usize| parse_epoch_millis(entry_of[position].timestamp()).unwrap_or(0);
        let mut sort_stack: Vec<usize> = roots.clone();
        while let Some(node) = sort_stack.pop() {
            children_of[node].sort_by_key(|&child| ordered_timestamp(child));
            sort_stack.extend(children_of[node].iter().copied());
        }

        // Iterative post-order assembly into owned nodes.
        let mut slots: Vec<Option<SessionTreeNode>> = (0..entries.len()).map(|_| None).collect();
        for root in roots {
            let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
            while let Some(&mut (node, ref mut child_index)) = stack.last_mut() {
                if *child_index < children_of[node].len() {
                    let child = children_of[node][*child_index];
                    *child_index += 1;
                    stack.push((child, 0));
                } else {
                    let (node, _) = stack.pop().expect("stack non-empty");
                    let children = children_of[node]
                        .iter()
                        .map(|&child| slots[child].take().expect("child assembled before parent"))
                        .collect();
                    slots[node] = Some(SessionTreeNode {
                        entry: entry_of[node].clone(),
                        children,
                        label: entry_of[node]
                            .id()
                            .and_then(|id| self.labels_by_id.get(id))
                            .cloned(),
                        label_timestamp: entry_of[node]
                            .id()
                            .and_then(|id| self.label_timestamps_by_id.get(id))
                            .cloned(),
                    });
                }
            }
        }
        slots.into_iter().flatten().collect()
    }

    // =========================================================================
    // Branching
    // =========================================================================

    /// Upstream `branch`: start a new branch from an earlier entry.
    pub fn branch(&mut self, branch_from_id: &str) -> Result<(), SessionManagerError> {
        if !self.by_id.contains_key(branch_from_id) {
            return Err(err(format!("Entry {branch_from_id} not found")));
        }
        self.leaf_id = Some(branch_from_id.to_string());
        Ok(())
    }

    /// Upstream `resetLeaf`: reset the leaf pointer (before any entries).
    pub fn reset_leaf(&mut self) {
        self.leaf_id = None;
    }

    /// Upstream `branchWithSummary`.
    pub fn branch_with_summary(
        &mut self,
        branch_from_id: Option<&str>,
        summary: &str,
        details: Option<Value>,
        from_hook: Option<bool>,
        usage: Option<Usage>,
    ) -> Result<String, SessionManagerError> {
        if let Some(branch_from_id) = branch_from_id {
            if !self.by_id.contains_key(branch_from_id) {
                return Err(err(format!("Entry {branch_from_id} not found")));
            }
        }
        let from_id = self.leaf_id.clone().unwrap_or_else(|| "root".to_string());
        self.leaf_id = branch_from_id.map(str::to_string);
        let entry = SessionEntry::BranchSummary(BranchSummaryEntry {
            id: mint_entry_id(),
            parent_id: branch_from_id.map(str::to_string),
            timestamp: now_iso(),
            from_id,
            summary: summary.to_string(),
            details,
            usage,
            from_hook,
        });
        self.append_entry(entry)
    }

    /// Upstream `createBranchedSession`: create a new session containing only
    /// the path from root to the specified leaf. Returns the new session file
    /// path, or `None` when not persisting.
    pub fn create_branched_session(
        &mut self,
        leaf_id: &str,
    ) -> Result<Option<String>, SessionManagerError> {
        let previous_session_file = self.session_file.clone();
        let path = self.get_branch(Some(leaf_id));
        if path.is_empty() {
            return Err(err(format!("Entry {leaf_id} not found")));
        }

        // Filter out LabelEntry from path — recreate them from the resolved
        // map. Removing labels requires re-chaining the retained path to
        // avoid orphaned subtrees.
        let mut path_without_labels: Vec<SessionEntry> = Vec::new();
        let mut replacement_by_label_id: OrderedMap<String> = OrderedMap::default();
        let mut pending_label_ids: Vec<String> = Vec::new();
        let mut path_parent_id: Option<String> = None;
        for mut entry in path {
            if let SessionEntry::Label(label) = &entry {
                pending_label_ids.push(label.id.clone());
                continue;
            }
            for label_id in pending_label_ids.drain(..) {
                replacement_by_label_id
                    .insert(label_id, entry.id().map(str::to_string).unwrap_or_default());
            }
            let first_kept_replacement = match &entry {
                SessionEntry::Compaction(compaction) => compaction
                    .first_kept_entry_id
                    .as_deref()
                    .and_then(|kept| replacement_by_label_id.get(kept).cloned()),
                _ => None,
            };
            entry.set_parent_id(path_parent_id.clone());
            if let (SessionEntry::Compaction(compaction), Some(replacement)) =
                (&mut entry, first_kept_replacement)
            {
                compaction.first_kept_entry_id = Some(replacement);
            }
            let entry_id = entry.id().map(str::to_string);
            path_without_labels.push(entry);
            path_parent_id = entry_id;
        }

        let new_session_id = mint_session_id();
        let timestamp = now_iso();
        let file_timestamp = timestamp.replace([':', '.'], "-");
        let new_session_file = path_join(&[
            self.get_session_dir(),
            &format!("{file_timestamp}_{new_session_id}.jsonl"),
        ]);

        let header = SessionHeader {
            version: Some(CURRENT_SESSION_VERSION),
            id: Some(new_session_id.clone()),
            timestamp: Some(timestamp),
            cwd: Some(self.cwd.clone()),
            parent_session: if self.persist {
                previous_session_file
            } else {
                None
            },
        };

        // Collect labels for entries in the path.
        let mut path_entry_ids: std::collections::HashSet<String> = path_without_labels
            .iter()
            .filter_map(|entry| entry.id().map(str::to_string))
            .collect();
        let labels_to_write: Vec<(String, String, String)> = self
            .labels_by_id
            .iter()
            .filter(|(target_id, _)| path_entry_ids.contains(*target_id))
            .map(|(target_id, label)| {
                (
                    target_id.to_string(),
                    label.clone(),
                    self.label_timestamps_by_id
                        .get(target_id)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect();

        if self.persist {
            // Build label entries.
            let mut parent_id = path_without_labels
                .last()
                .and_then(|entry| entry.id())
                .map(str::to_string);
            let mut label_entries: Vec<SessionEntry> = Vec::new();
            for (target_id, label, label_timestamp) in labels_to_write {
                let label_entry = SessionEntry::Label(LabelEntry {
                    id: generate_id_excluding(&|candidate| path_entry_ids.contains(candidate)),
                    parent_id: parent_id.clone(),
                    timestamp: label_timestamp,
                    target_id: target_id.clone(),
                    label: Some(label),
                });
                if let SessionEntry::Label(label) = &label_entry {
                    path_entry_ids.insert(label.id.clone());
                    parent_id = Some(label.id.clone());
                }
                label_entries.push(label_entry);
            }

            self.file_entries =
                Vec::with_capacity(1 + path_without_labels.len() + label_entries.len());
            self.file_entries.push(FileEntry::Session(header));
            self.file_entries
                .extend(path_without_labels.into_iter().map(FileEntry::Entry));
            self.file_entries
                .extend(label_entries.into_iter().map(FileEntry::Entry));
            self.session_id = new_session_id;
            self.session_file = Some(new_session_file.clone());
            self.build_index();

            // Use the same rule as persist_entry(): write now if the branched
            // path already has a conversation, otherwise let persist_entry()
            // create the file later.
            if self.has_conversation() {
                self.rewrite_file()?;
                self.flushed = true;
            } else {
                self.flushed = false;
            }

            return Ok(Some(new_session_file));
        }

        // In-memory mode: replace current session with the path + labels.
        let mut parent_id = path_without_labels
            .last()
            .and_then(|entry| entry.id())
            .map(str::to_string);
        let mut label_entries: Vec<SessionEntry> = Vec::new();
        for (target_id, label, label_timestamp) in labels_to_write {
            let label_entry = SessionEntry::Label(LabelEntry {
                id: generate_id_excluding(&|candidate| {
                    path_entry_ids.contains(candidate)
                        || label_entries
                            .iter()
                            .any(|entry| entry.id().is_some_and(|entry_id| entry_id == candidate))
                }),
                parent_id: parent_id.clone(),
                timestamp: label_timestamp,
                target_id: target_id.clone(),
                label: Some(label),
            });
            if let SessionEntry::Label(label) = &label_entry {
                parent_id = Some(label.id.clone());
            }
            label_entries.push(label_entry);
        }
        self.file_entries = Vec::with_capacity(1 + path_without_labels.len() + label_entries.len());
        self.file_entries.push(FileEntry::Session(header));
        self.file_entries
            .extend(path_without_labels.into_iter().map(FileEntry::Entry));
        self.file_entries
            .extend(label_entries.into_iter().map(FileEntry::Entry));
        self.session_id = new_session_id;
        self.build_index();
        Ok(None)
    }

    // =========================================================================
    // Constructors & discovery
    // =========================================================================

    /// Upstream `SessionManager.create`.
    pub fn create(
        cwd: &str,
        session_dir: Option<&str>,
        options: Option<&NewSessionOptions>,
    ) -> Result<Self, SessionManagerError> {
        let dir = match session_dir {
            Some(session_dir) => {
                normalize_path(session_dir).unwrap_or_else(|_| session_dir.to_string())
            }
            None => get_default_session_dir(cwd),
        };
        Self::new(cwd, &dir, None, true, options, None)
    }

    /// Upstream `SessionManager.open`.
    pub fn open(
        path: &str,
        session_dir: Option<&str>,
        cwd_override: Option<&str>,
    ) -> Result<Self, SessionManagerError> {
        let resolved_path = resolve_path_auto_base(path)?;
        let mut header: Option<SessionHeader> = None;
        let mut preloaded_file_entries: Option<Vec<FileEntry>> = None;
        if cwd_override.is_none() && path_exists(&resolved_path) {
            match read_session_header(&resolved_path) {
                Ok(found) => header = found,
                Err(SessionHeaderError::Limit(_)) => {
                    // The bounded scan is only a discovery optimization. A
                    // full load remains authoritative for legacy files with
                    // very large headers or prefixes.
                    preloaded_file_entries = Some(load_entries_from_file(&resolved_path));
                    header = preloaded_file_entries.as_ref().and_then(|entries| {
                        entries.first().and_then(|first| match first {
                            FileEntry::Session(header) => Some(header.clone()),
                            _ => None,
                        })
                    });
                }
                Err(other) => return Err(err(other.to_string())),
            }
        }
        let cwd = cwd_override
            .map(str::to_string)
            .or_else(|| header.as_ref().and_then(|header| header.cwd.clone()))
            .unwrap_or_else(process_cwd);
        // If no sessionDir provided, derive from file's parent directory.
        let dir = match session_dir {
            Some(session_dir) => {
                normalize_path(session_dir).unwrap_or_else(|_| session_dir.to_string())
            }
            None => node_resolve(&[&resolved_path, ".."]),
        };
        Self::new(
            &cwd,
            &dir,
            Some(&resolved_path),
            true,
            None,
            preloaded_file_entries,
        )
    }

    /// Upstream `SessionManager.continueRecent`.
    pub fn continue_recent(
        cwd: &str,
        session_dir: Option<&str>,
    ) -> Result<Self, SessionManagerError> {
        let dir = match session_dir {
            Some(session_dir) => {
                normalize_path(session_dir).unwrap_or_else(|_| session_dir.to_string())
            }
            None => get_default_session_dir(cwd),
        };
        let filter_cwd =
            session_dir.is_some() && dir != get_default_session_dir_path(cwd, &get_agent_dir());
        let most_recent = find_most_recent_session(&dir, filter_cwd.then_some(cwd));
        if let Some(most_recent) = most_recent {
            return Self::new(cwd, &dir, Some(&most_recent), true, None, None);
        }
        Self::new(cwd, &dir, None, true, None, None)
    }

    /// Upstream `SessionManager.inMemory`.
    pub fn in_memory(
        cwd: &str,
        options: Option<&NewSessionOptions>,
        entries: Option<Vec<FileEntry>>,
    ) -> Result<Self, SessionManagerError> {
        Self::new(cwd, "", None, false, options, entries)
    }

    /// Upstream `SessionManager.forkFrom`.
    pub fn fork_from(
        source_path: &str,
        target_cwd: &str,
        session_dir: Option<&str>,
        options: Option<&NewSessionOptions>,
    ) -> Result<Self, SessionManagerError> {
        let resolved_source_path = resolve_path_auto_base(source_path)?;
        let resolved_target_cwd = resolve_path_auto_base(target_cwd)?;
        let source_entries = load_entries_from_file(&resolved_source_path);
        if source_entries.is_empty() {
            return Err(err(format!(
                "Cannot fork: source session file is empty or invalid: {resolved_source_path}"
            )));
        }

        let source_header = source_entries.iter().find_map(|entry| match entry {
            FileEntry::Session(header) => Some(header.clone()),
            _ => None,
        });
        if source_header.is_none() {
            return Err(err(format!(
                "Cannot fork: source session has no header: {resolved_source_path}"
            )));
        }

        let dir = match session_dir {
            Some(session_dir) => {
                normalize_path(session_dir).unwrap_or_else(|_| session_dir.to_string())
            }
            None => get_default_session_dir(&resolved_target_cwd),
        };
        if !path_exists(&dir) {
            std::fs::create_dir_all(&dir)?;
        }

        // Create new session file with new ID but forked content.
        if let Some(id) = options.and_then(|options| options.id.as_deref()) {
            assert_valid_session_id(id)?;
        }
        let new_session_id = options
            .and_then(|options| options.id.clone())
            .unwrap_or_else(mint_session_id);
        let timestamp = now_iso();
        let file_timestamp = timestamp.replace([':', '.'], "-");
        let new_session_file =
            path_join(&[&dir, &format!("{file_timestamp}_{new_session_id}.jsonl")]);

        // Write new header pointing to source as parent, with updated cwd.
        let new_header = FileEntry::Session(SessionHeader {
            version: Some(CURRENT_SESSION_VERSION),
            id: Some(new_session_id),
            timestamp: Some(timestamp),
            cwd: Some(resolved_target_cwd.clone()),
            parent_session: Some(resolved_source_path),
        });
        {
            let mut line = serde_json::to_string(&new_header)?;
            line.push('\n');
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&new_session_file)?;
            file.write_all(line.as_bytes())?;
        }

        // Copy all non-header entries from source.
        for entry in &source_entries {
            if entry.type_name() != Some("session") {
                let mut line = serde_json::to_string(entry)?;
                line.push('\n');
                append_file_text(&new_session_file, &line)?;
            }
        }

        Self::new(
            &resolved_target_cwd,
            &dir,
            Some(&new_session_file),
            true,
            None,
            None,
        )
    }

    /// Upstream `SessionManager.findById`: exact session id without loading
    /// transcript bodies.
    pub fn find_by_id(cwd: &str, id: &str, session_dir: Option<&str>) -> Option<String> {
        let dir = match session_dir {
            Some(session_dir) => {
                normalize_path(session_dir).unwrap_or_else(|_| session_dir.to_string())
            }
            None => get_default_session_dir(cwd),
        };
        let filter_cwd =
            session_dir.is_some() && dir != get_default_session_dir_path(cwd, &get_agent_dir());
        let Ok(resolved_cwd) = resolve_path_auto_base(cwd) else {
            return None;
        };

        let Ok(read_dir) = std::fs::read_dir(&dir) else {
            return None;
        };
        for entry in read_dir.filter_map(|entry| entry.ok()) {
            let file = entry.file_name().to_string_lossy().into_owned();
            if !file.ends_with(".jsonl") {
                continue;
            }
            let path = path_join(&[&dir, &file]);
            let Some(header) = read_session_header_for_discovery(&path) else {
                continue;
            };
            if header.id.as_deref() != Some(id) {
                continue;
            }
            if filter_cwd && !session_cwd_matches(header.cwd.as_deref(), &resolved_cwd) {
                continue;
            }
            return Some(path);
        }
        None
    }

    /// Upstream `SessionManager.list`.
    pub fn list(
        cwd: &str,
        session_dir: Option<&str>,
        on_progress: Option<&mut SessionListProgress<'_>>,
    ) -> Vec<SessionInfo> {
        let dir = match session_dir {
            Some(session_dir) => {
                normalize_path(session_dir).unwrap_or_else(|_| session_dir.to_string())
            }
            None => get_default_session_dir(cwd),
        };
        let filter_cwd =
            session_dir.is_some() && dir != get_default_session_dir_path(cwd, &get_agent_dir());
        let Ok(resolved_cwd) = resolve_path_auto_base(cwd) else {
            return Vec::new();
        };
        let include_session = |session: &SessionInfo| {
            !filter_cwd || session_cwd_matches(Some(&session.cwd), &resolved_cwd)
        };
        let mut progress: Option<ProgressBox<'_>> =
            on_progress.map(|on_progress| -> ProgressBox<'_> {
                Box::new(
                    move |loaded: usize, total: usize, partial: Option<&[SessionInfo]>| {
                        let filtered: Option<Vec<SessionInfo>> = partial.map(|partial| {
                            partial
                                .iter()
                                .filter(|session| include_session(session))
                                .cloned()
                                .collect()
                        });
                        on_progress(loaded, total, filtered.as_deref());
                    },
                )
            });
        let mut sessions: Vec<SessionInfo> = list_sessions_from_dir(&dir, progress.as_deref_mut())
            .into_iter()
            .filter(|session| include_session(session))
            .collect();
        sort_session_infos(&mut sessions);
        sessions
    }

    /// Upstream `SessionManager.listAll` (both overloads: custom session dir
    /// or the shared sessions root).
    pub fn list_all(
        session_dir: Option<&str>,
        mut on_progress: Option<&mut SessionListProgress<'_>>,
    ) -> Vec<SessionInfo> {
        if let Some(custom_session_dir) = session_dir {
            let custom_session_dir = normalize_path(custom_session_dir)
                .unwrap_or_else(|_| custom_session_dir.to_string());
            let mut sessions =
                list_sessions_from_dir(&custom_session_dir, on_progress.as_deref_mut());
            sort_session_infos(&mut sessions);
            return sessions;
        }

        let sessions_dir = path_join(&[&get_agent_dir(), "sessions"]);
        if !path_exists(&sessions_dir) {
            return Vec::new();
        }
        let Ok(read_dir) = std::fs::read_dir(&sessions_dir) else {
            return Vec::new();
        };
        let dirs: Vec<String> = read_dir
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_type()
                    .map(|file_type| file_type.is_dir() || file_type.is_symlink())
                    .unwrap_or(false)
            })
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .map(|name| path_join(&[&sessions_dir, &name]))
            .collect();

        let dir_files: Vec<Vec<String>> = dirs
            .iter()
            .map(|dir| match std::fs::read_dir(dir) {
                Ok(entries) => entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.ends_with(".jsonl"))
                    .map(|name| path_join(&[dir, &name]))
                    .collect(),
                Err(_) => Vec::new(),
            })
            .collect();

        // Stat every candidate up front so the discovery order is
        // mtime-descending (stat failures sort last, then by basename
        // descending).
        let all_files: Vec<String> = dir_files.into_iter().flatten().collect();
        let mut candidates: Vec<(String, Option<f64>)> = all_files
            .into_iter()
            .map(|path| {
                let mtime = file_mtime_ms_f64(&path);
                (path, mtime)
            })
            .collect();
        candidates.sort_by(|a, b| {
            let a_mtime = a.1.unwrap_or(f64::NEG_INFINITY);
            let b_mtime = b.1.unwrap_or(f64::NEG_INFINITY);
            b_mtime
                .total_cmp(&a_mtime)
                .then_with(|| basename(&b.0).cmp(basename(&a.0)))
        });

        const PUBLISH_INTERVAL: usize = 100;
        let total_files = candidates.len();
        let mut loaded = 0usize;
        let mut first_candidate_loaded = false;
        let mut partial_sessions: Vec<SessionInfo> = Vec::new();
        let files: Vec<String> = candidates.into_iter().map(|(path, _)| path).collect();
        let results = build_session_infos(&files, &mut |index, info| {
            loaded += 1;
            if index == 0 {
                first_candidate_loaded = true;
            }
            if let Some(info) = info {
                partial_sessions.push(info.clone());
            }
            if let Some(progress) = on_progress.as_deref_mut() {
                let publish_partial = first_candidate_loaded
                    && (index == 0
                        || loaded.is_multiple_of(PUBLISH_INTERVAL)
                        || loaded == total_files);
                progress(
                    loaded,
                    total_files,
                    publish_partial
                        .then(|| {
                            let mut partial = partial_sessions.clone();
                            sort_session_infos(&mut partial);
                            partial
                        })
                        .as_deref(),
                );
            }
        });
        let mut sessions: Vec<SessionInfo> = results.into_iter().flatten().collect();
        sort_session_infos(&mut sessions);
        sessions
    }
}

/// The basename of a joined path (the discovery sorts tie-break on it).
fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(test)]
#[path = "session_manager_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_session_delta_oracle_tests.rs"]
mod delta_oracle_tests;
