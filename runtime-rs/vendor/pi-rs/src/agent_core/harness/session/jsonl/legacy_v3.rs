//! Port of `packages/agent/src/harness/session/jsonl/legacy-v3.ts` (681
//! lines): the legacy v3 (SessionManager-era) session normalizer. Scans
//! complete records without modifying the file, remints retained entry ids
//! as UUIDv7s stamped with the legacy timestamps, folds discarded records
//! (config changes, labels, session info) into structure and derived
//! current values, and emits normalized v4 writes — including
//! reconstruction of compaction retained tails.
//!
//! Disclosed substitutions:
//! - `writes()` materializes one pass's writes into a `Vec` before yielding
//!   (upstream streams line by line but caches the required tail messages
//!   for the whole pass anyway); read counts and output order are identical.
//! - A legacy record with a *missing* (vs `null`) `parentId` parses as the
//!   null root; upstream's `undefined` case errors at indexing — real
//!   fixtures always carry the field, so oracle behavior is unchanged.
//! - `Date.parse` of a malformed per-entry timestamp yields 0 (upstream
//!   `NaN` poisons downstream with a `RangeError`); the header timestamp is
//!   validated strictly in the codec, which is the only checked form.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::Deserialize;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::messages::{
    create_branch_summary_message, create_compaction_summary_message,
};
use crate::agent_core::harness::session::commit::CommittedWrite;
use crate::agent_core::harness::session::jsonl::codec::{
    parse_jsonl_session_header, LegacyV3SessionHeader,
};
use crate::agent_core::harness::session::jsonl::io::{file_value, read_jsonl_header};
use crate::agent_core::harness::session::jsonl::iso8601::parse_iso8601_utc;
use crate::agent_core::harness::session::jsonl::types::{
    JsonlStorageHeader, JSONL_STORAGE_VERSION,
};
use crate::agent_core::harness::session::types::{
    Entry, LaneConfiguration, LaneModel, SessionMetadata,
};
use crate::agent_core::harness::session::values::{
    branch_tip, entry_label, lane_config, lane_state, session_name,
};
use crate::agent_core::types::{AgentMessage, CustomAgentMessage, ThinkingLevel};
use crate::ai::types::primitives::Usage;
use crate::ai::uuid;

/// The parsed legacy v3 record union (`legacy-v3.ts:20-115`), retained and
/// discarded kinds alike.
#[derive(Debug, Clone, Deserialize)]
#[allow(clippy::large_enum_variant)] // Message carries the full legacy payload, like Entry
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum LegacyV3Entry {
    Message {
        id: String,
        #[serde(default)]
        parent_id: Option<String>,
        timestamp: String,
        message: AgentMessage,
    },
    Custom {
        id: String,
        #[serde(default)]
        parent_id: Option<String>,
        timestamp: String,
        custom_type: String,
        #[serde(default)]
        data: Option<serde_json::Value>,
    },
    CustomMessage {
        id: String,
        #[serde(default)]
        parent_id: Option<String>,
        timestamp: String,
        custom_type: String,
        content: serde_json::Value,
        #[serde(default)]
        details: Option<serde_json::Value>,
        display: bool,
    },
    BranchSummary {
        id: String,
        #[serde(default)]
        parent_id: Option<String>,
        timestamp: String,
        from_id: String,
        summary: String,
        #[serde(default)]
        details: Option<serde_json::Value>,
        #[serde(default)]
        usage: Option<Usage>,
        #[serde(default)]
        from_hook: Option<bool>,
    },
    Compaction {
        id: String,
        #[serde(default)]
        parent_id: Option<String>,
        timestamp: String,
        summary: String,
        first_kept_entry_id: String,
        tokens_before: i64,
        #[serde(default)]
        details: Option<serde_json::Value>,
        #[serde(default)]
        usage: Option<Usage>,
        #[serde(default)]
        from_hook: Option<bool>,
    },
    ModelChange {
        id: String,
        #[serde(default)]
        parent_id: Option<String>,
        timestamp: String,
        provider: String,
        model_id: String,
    },
    ThinkingLevelChange {
        id: String,
        #[serde(default)]
        parent_id: Option<String>,
        timestamp: String,
        thinking_level: ThinkingLevel,
    },
    ActiveToolsChange {
        id: String,
        #[serde(default)]
        parent_id: Option<String>,
        timestamp: String,
        active_tool_names: Vec<String>,
    },
    SessionInfo {
        id: String,
        #[serde(default)]
        parent_id: Option<String>,
        timestamp: String,
        #[serde(default)]
        name: Option<String>,
    },
    Label {
        id: String,
        #[serde(default)]
        parent_id: Option<String>,
        timestamp: String,
        target_id: String,
        #[serde(default)]
        label: Option<String>,
    },
}

impl LegacyV3Entry {
    /// The record's `type` literal (upstream `entry.type`).
    pub fn type_str(&self) -> &'static str {
        match self {
            LegacyV3Entry::Message { .. } => "message",
            LegacyV3Entry::Custom { .. } => "custom",
            LegacyV3Entry::CustomMessage { .. } => "custom_message",
            LegacyV3Entry::BranchSummary { .. } => "branch_summary",
            LegacyV3Entry::Compaction { .. } => "compaction",
            LegacyV3Entry::ModelChange { .. } => "model_change",
            LegacyV3Entry::ThinkingLevelChange { .. } => "thinking_level_change",
            LegacyV3Entry::ActiveToolsChange { .. } => "active_tools_change",
            LegacyV3Entry::SessionInfo { .. } => "session_info",
            LegacyV3Entry::Label { .. } => "label",
        }
    }

    fn id(&self) -> &str {
        match self {
            LegacyV3Entry::Message { id, .. }
            | LegacyV3Entry::Custom { id, .. }
            | LegacyV3Entry::CustomMessage { id, .. }
            | LegacyV3Entry::BranchSummary { id, .. }
            | LegacyV3Entry::Compaction { id, .. }
            | LegacyV3Entry::ModelChange { id, .. }
            | LegacyV3Entry::ThinkingLevelChange { id, .. }
            | LegacyV3Entry::ActiveToolsChange { id, .. }
            | LegacyV3Entry::SessionInfo { id, .. }
            | LegacyV3Entry::Label { id, .. } => id,
        }
    }

    fn parent_id(&self) -> Option<&str> {
        match self {
            LegacyV3Entry::Message { parent_id, .. }
            | LegacyV3Entry::Custom { parent_id, .. }
            | LegacyV3Entry::CustomMessage { parent_id, .. }
            | LegacyV3Entry::BranchSummary { parent_id, .. }
            | LegacyV3Entry::Compaction { parent_id, .. }
            | LegacyV3Entry::ModelChange { parent_id, .. }
            | LegacyV3Entry::ThinkingLevelChange { parent_id, .. }
            | LegacyV3Entry::ActiveToolsChange { parent_id, .. }
            | LegacyV3Entry::SessionInfo { parent_id, .. }
            | LegacyV3Entry::Label { parent_id, .. } => parent_id.as_deref(),
        }
    }

    fn timestamp(&self) -> &str {
        match self {
            LegacyV3Entry::Message { timestamp, .. }
            | LegacyV3Entry::Custom { timestamp, .. }
            | LegacyV3Entry::CustomMessage { timestamp, .. }
            | LegacyV3Entry::BranchSummary { timestamp, .. }
            | LegacyV3Entry::Compaction { timestamp, .. }
            | LegacyV3Entry::ModelChange { timestamp, .. }
            | LegacyV3Entry::ThinkingLevelChange { timestamp, .. }
            | LegacyV3Entry::ActiveToolsChange { timestamp, .. }
            | LegacyV3Entry::SessionInfo { timestamp, .. }
            | LegacyV3Entry::Label { timestamp, .. } => timestamp,
        }
    }

    /// Upstream `isRetainedEntry` (`legacy-v3.ts:234-244`).
    fn is_retained(&self) -> bool {
        matches!(
            self,
            LegacyV3Entry::Message { .. }
                | LegacyV3Entry::Custom { .. }
                | LegacyV3Entry::CustomMessage { .. }
                | LegacyV3Entry::BranchSummary { .. }
                | LegacyV3Entry::Compaction { .. }
        )
    }
}

/// The per-kind index payload (`legacy-v3.ts:124-142`).
#[derive(Debug, Clone)]
#[allow(dead_code)] // from_id is carried for parity with the upstream index entry
enum LegacyV3IndexKind {
    Message,
    Custom,
    CustomMessage,
    BranchSummary {
        from_id: String,
    },
    Compaction {
        first_kept_entry_id: String,
    },
    Label {
        target_id: String,
        label: Option<String>,
    },
    ModelChange {
        provider: String,
        model_id: String,
    },
    ThinkingLevelChange {
        thinking_level: ThinkingLevel,
    },
    ActiveToolsChange {
        active_tool_names: Vec<String>,
    },
    SessionInfo,
}

impl LegacyV3IndexKind {
    fn type_str(&self) -> &'static str {
        match self {
            LegacyV3IndexKind::Message => "message",
            LegacyV3IndexKind::Custom => "custom",
            LegacyV3IndexKind::CustomMessage => "custom_message",
            LegacyV3IndexKind::BranchSummary { .. } => "branch_summary",
            LegacyV3IndexKind::Compaction { .. } => "compaction",
            LegacyV3IndexKind::Label { .. } => "label",
            LegacyV3IndexKind::ModelChange { .. } => "model_change",
            LegacyV3IndexKind::ThinkingLevelChange { .. } => "thinking_level_change",
            LegacyV3IndexKind::ActiveToolsChange { .. } => "active_tools_change",
            LegacyV3IndexKind::SessionInfo => "session_info",
        }
    }

    fn is_retained(&self) -> bool {
        matches!(
            self,
            LegacyV3IndexKind::Message
                | LegacyV3IndexKind::Custom
                | LegacyV3IndexKind::CustomMessage
                | LegacyV3IndexKind::BranchSummary { .. }
                | LegacyV3IndexKind::Compaction { .. }
        )
    }
}

/// Upstream `LegacyV3IndexEntry` (`legacy-v3.ts:117-141`): one scanned
/// record's structure. `mapped_id` is the record's reminted id, or the
/// nearest retained ancestor's for discarded records (`None` = null root).
#[derive(Debug, Clone)]
struct LegacyV3IndexEntry {
    id: String,
    parent_id: Option<String>,
    mapped_id: Option<String>,
    /// Assigned sequence for retained records only.
    seq: Option<i64>,
    kind: LegacyV3IndexKind,
}

/// The scanned entry collection: file order (the captured-prefix replay and
/// configuration walk depend on it) plus id lookup.
#[derive(Debug, Default)]
struct LegacyV3Entries {
    ordered: Vec<LegacyV3IndexEntry>,
    by_id: HashMap<String, usize>,
}

impl LegacyV3Entries {
    fn get(&self, id: &str) -> Option<&LegacyV3IndexEntry> {
        self.by_id.get(id).map(|&index| &self.ordered[index])
    }

    fn len(&self) -> usize {
        self.ordered.len()
    }

    fn iter(&self) -> impl Iterator<Item = &LegacyV3IndexEntry> {
        self.ordered.iter()
    }
}

/// Upstream `LegacyV3Inventory` (`legacy-v3.ts:144-150`).
struct LegacyV3Inventory {
    entries: LegacyV3Entries,
    imported_usage: Usage,
    name: Option<String>,
    final_id: Option<String>,
    next_seq: i64,
}

/// Upstream `resolveLegacyV3ParentSessionId` (`legacy-v3.ts:154-163`): read
/// failures tolerate to `None`, like upstream `!lines.ok`.
async fn resolve_legacy_v3_parent_session_id(
    file_system: &dyn crate::agent_core::harness::types::FileSystem,
    parent_session_path: &str,
    context: Context,
) -> Option<String> {
    let lines = file_system
        .read_text_lines(
            parent_session_path,
            Some(&crate::agent_core::harness::types::ReadTextLinesOptions { max_lines: Some(1) }),
            context,
        )
        .await
        .ok()?;
    let first = lines.first()?;
    let parsed = parse_jsonl_session_header(first).ok()?;
    Some(parsed.header_id().to_string())
}

/// Upstream `metadataFromLegacyV3Header` (`legacy-v3.ts:165-182`).
pub async fn metadata_from_legacy_v3_header(
    file_system: &Arc<dyn crate::agent_core::harness::types::FileSystem>,
    header: &LegacyV3SessionHeader,
    context: Context,
) -> SessionMetadata {
    let mut metadata = SessionMetadata {
        id: header.id.clone(),
        created_at: parse_iso8601_utc(&header.timestamp).unwrap_or_default(),
        storage_version: JSONL_STORAGE_VERSION,
        cwd: Some(header.cwd.clone()),
        ..SessionMetadata::default()
    };
    if let Some(parent_session_path) = &header.parent_session {
        let parent_session_id =
            resolve_legacy_v3_parent_session_id(file_system.as_ref(), parent_session_path, context)
                .await;
        match parent_session_id {
            Some(parent_session_id) => metadata.parent_session_id = Some(parent_session_id),
            None => metadata.legacy_parent_session_path = Some(parent_session_path.clone()),
        }
    }
    metadata
}

/// Upstream `normalizeLegacyV3Header` (`legacy-v3.ts:184-194`).
pub async fn normalize_legacy_v3_header(
    file_system: &Arc<dyn crate::agent_core::harness::types::FileSystem>,
    header: &LegacyV3SessionHeader,
    context: Context,
) -> JsonlStorageHeader {
    let metadata = metadata_from_legacy_v3_header(file_system, header, context).await;
    JsonlStorageHeader {
        parent_session_id: metadata.parent_session_id,
        legacy_parent_session_path: metadata.legacy_parent_session_path,
        ..JsonlStorageHeader::new(
            metadata.id,
            metadata.storage_version,
            metadata.created_at,
            metadata.cwd.unwrap_or_default(),
        )
    }
}

/// Upstream `parseLegacyV3Entry(line)` (`legacy-v3.ts:196-219`).
fn parse_legacy_v3_entry(line: &str) -> anyhow::Result<LegacyV3Entry> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
        anyhow::anyhow!("Invalid legacy v3 JSONL record: not valid JSON: {error}")
    })?;
    let record_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    match record_type {
        "message"
        | "custom"
        | "custom_message"
        | "branch_summary"
        | "compaction"
        | "model_change"
        | "thinking_level_change"
        | "active_tools_change"
        | "session_info"
        | "label" => {}
        _ => anyhow::bail!("Unsupported legacy v3 record type: {record_type}"),
    }
    Ok(serde_json::from_value(value)?)
}

/// Upstream `importedCustomMessage` (`legacy-v3.ts:221-232`): the v3
/// `custom_message` record as a current custom-role message.
fn imported_custom_message(entry: &LegacyV3Entry) -> AgentMessage {
    let LegacyV3Entry::CustomMessage {
        custom_type,
        content,
        details,
        display,
        timestamp,
        ..
    } = entry
    else {
        unreachable!("imported_custom_message on a non-custom_message record")
    };
    let mut data = serde_json::Map::new();
    data.insert(
        "customType".into(),
        serde_json::Value::String(custom_type.clone()),
    );
    data.insert("content".into(), content.clone());
    if let Some(details) = details {
        data.insert("details".into(), details.clone());
    }
    data.insert("display".into(), serde_json::Value::Bool(*display));
    data.insert(
        "timestamp".into(),
        serde_json::Value::Number(parse_iso8601_utc(timestamp).unwrap_or_default().into()),
    );
    AgentMessage::Custom(CustomAgentMessage {
        role: "custom".into(),
        data,
    })
}

/// Upstream `createLegacyIdResolver` (`legacy-v3.ts:253-260`): resolve a
/// legacy id to its imported id; discarded records resolve to the minted id
/// of their nearest retained ancestor (folded at scan time). A missing
/// reference errors; a folded null root resolves to `None`.
fn resolve_legacy_id(
    entries: &LegacyV3Entries,
    legacy_id: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let Some(legacy_id) = legacy_id else {
        return Ok(None);
    };
    match entries.get(legacy_id) {
        None => anyhow::bail!("Missing legacy v3 entry reference: {legacy_id}"),
        Some(entry) => Ok(entry.mapped_id.clone()),
    }
}

/// Upstream `resolveBranchSummaryFromId` (`legacy-v3.ts:262-265`): the
/// legacy `"root"` sentinel encodes a null source.
fn resolve_branch_summary_from_id(
    entries: &LegacyV3Entries,
    legacy_from_id: &str,
) -> anyhow::Result<Option<String>> {
    if legacy_from_id == "root" {
        return Ok(None);
    }
    resolve_legacy_id(entries, Some(legacy_from_id))
}

fn entry_timestamp_ms(entry: &LegacyV3Entry) -> i64 {
    parse_iso8601_utc(entry.timestamp()).unwrap_or_default()
}

/// Upstream `projectContextMessage` (`legacy-v3.ts:267-291`).
fn project_context_message(
    entry: &LegacyV3Entry,
    entries: &LegacyV3Entries,
) -> anyhow::Result<Option<AgentMessage>> {
    match entry {
        LegacyV3Entry::Message { message, .. } => Ok(Some(message.clone())),
        LegacyV3Entry::CustomMessage { .. } => Ok(Some(imported_custom_message(entry))),
        LegacyV3Entry::BranchSummary {
            summary, from_id, ..
        } => {
            if summary.is_empty() {
                return Ok(None);
            }
            let from_id = resolve_branch_summary_from_id(entries, from_id)?;
            let message =
                create_branch_summary_message(summary.clone(), from_id, entry_timestamp_ms(entry));
            Ok(Some(AgentMessage::Custom(message.to_custom())))
        }
        LegacyV3Entry::Compaction {
            summary,
            tokens_before,
            ..
        } => {
            let message = create_compaction_summary_message(
                summary.clone(),
                *tokens_before,
                entry_timestamp_ms(entry),
            );
            Ok(Some(AgentMessage::Custom(message.to_custom())))
        }
        LegacyV3Entry::Custom { .. }
        | LegacyV3Entry::ModelChange { .. }
        | LegacyV3Entry::ThinkingLevelChange { .. }
        | LegacyV3Entry::ActiveToolsChange { .. }
        | LegacyV3Entry::SessionInfo { .. }
        | LegacyV3Entry::Label { .. } => Ok(None),
    }
}

/// Upstream `retainedTailStructure` (`legacy-v3.ts:294-309`): walk the
/// physical ancestry from the compaction's parent through
/// `firstKeptEntryId`, inclusive.
fn retained_tail_structure<'a>(
    compaction_id: &str,
    compaction_parent_id: Option<&str>,
    compaction_first_kept_entry_id: &str,
    entries: &'a LegacyV3Entries,
) -> anyhow::Result<Vec<&'a LegacyV3IndexEntry>> {
    let mut structure = Vec::new();
    let mut current_id = compaction_parent_id.map(str::to_string);
    while let Some(current) = current_id {
        // The scan guarantees every parent exists earlier in the file, so
        // cycles are impossible.
        let entry = entries
            .get(&current)
            .expect("parent exists earlier in file");
        structure.push(entry);
        if current == compaction_first_kept_entry_id {
            return Ok(structure);
        }
        current_id = entry.parent_id.clone();
    }
    anyhow::bail!(
        "Legacy v3 compaction {compaction_id} firstKeptEntryId is not on its parent branch: \
         {compaction_first_kept_entry_id}"
    )
}

/// Upstream `normalizeRetainedEntry` (`legacy-v3.ts:311-363`).
fn normalize_retained_entry(
    entry: &LegacyV3Entry,
    indexed: &LegacyV3IndexEntry,
    retained_tail: Vec<AgentMessage>,
    entries: &LegacyV3Entries,
) -> anyhow::Result<CommittedWrite> {
    let parent_id = resolve_legacy_id(entries, indexed.parent_id.as_deref())?;
    let mapped_id = indexed.mapped_id.clone().unwrap_or_default();
    let seq = indexed.seq.unwrap_or_default();
    let timestamp = entry_timestamp_ms(entry);
    let committed = match entry {
        LegacyV3Entry::Message { message, .. } => Entry::Message {
            id: mapped_id,
            parent_id,
            seq,
            timestamp,
            message: message.clone(),
            terminate: None,
        },
        LegacyV3Entry::CustomMessage { .. } => Entry::Message {
            id: mapped_id,
            parent_id,
            seq,
            timestamp,
            message: imported_custom_message(entry),
            terminate: None,
        },
        LegacyV3Entry::BranchSummary {
            from_id,
            summary,
            details,
            usage,
            from_hook,
            ..
        } => Entry::BranchSummary {
            id: mapped_id,
            parent_id,
            seq,
            timestamp,
            from_id: resolve_branch_summary_from_id(entries, from_id)?,
            summary: summary.clone(),
            details: details.clone(),
            usage: *usage,
            from_hook: from_hook.unwrap_or(false),
        },
        LegacyV3Entry::Compaction {
            summary,
            tokens_before,
            details,
            usage,
            from_hook,
            ..
        } => Entry::Compaction {
            id: mapped_id,
            parent_id,
            seq,
            timestamp,
            summary: summary.clone(),
            retained_tail,
            tokens_before: *tokens_before,
            details: details.clone(),
            usage: *usage,
            from_hook: from_hook.unwrap_or(false),
        },
        LegacyV3Entry::Custom {
            custom_type, data, ..
        } => Entry::Custom {
            id: mapped_id,
            parent_id,
            seq,
            timestamp,
            custom_type: custom_type.clone(),
            data: data.clone(),
        },
        LegacyV3Entry::ModelChange { .. }
        | LegacyV3Entry::ThinkingLevelChange { .. }
        | LegacyV3Entry::ActiveToolsChange { .. }
        | LegacyV3Entry::SessionInfo { .. }
        | LegacyV3Entry::Label { .. } => {
            anyhow::bail!("normalizeRetainedEntry on a discarded record")
        }
    };
    Ok(CommittedWrite::Entry {
        entry: Box::new(committed),
    })
}

/// Upstream `selectedConfiguration` (`legacy-v3.ts:365-398`): the nearest
/// model/thinking-level/tools changes on the selected physical branch.
/// Invalid configurations still consume the nearest change.
fn selected_configuration(
    entries: &LegacyV3Entries,
    selected_id: Option<&str>,
) -> Option<LaneConfiguration> {
    let mut remaining: HashSet<&'static str> = HashSet::from([
        "model_change",
        "thinking_level_change",
        "active_tools_change",
    ]);
    let mut model: Option<LaneModel> = None;
    let mut thinking_level: Option<ThinkingLevel> = None;
    let mut active_tool_names: Option<Vec<String>> = None;
    let mut current_id = selected_id.map(str::to_string);
    while let Some(current) = current_id {
        if remaining.is_empty() {
            break;
        }
        let entry = entries.get(&current).expect("walk ids are indexed");
        // Consume the nearest change even when invalid: older values must
        // not become fallbacks.
        if remaining.remove(entry.kind.type_str()) {
            match &entry.kind {
                LegacyV3IndexKind::ModelChange { provider, model_id } => {
                    model = Some(LaneModel {
                        provider: provider.clone(),
                        model_id: model_id.clone(),
                    });
                }
                LegacyV3IndexKind::ThinkingLevelChange {
                    thinking_level: level,
                } => {
                    thinking_level = Some(*level);
                }
                LegacyV3IndexKind::ActiveToolsChange {
                    active_tool_names: names,
                } => {
                    active_tool_names = Some(names.clone());
                }
                _ => {}
            }
        }
        current_id = entry.parent_id.clone();
    }
    Some(LaneConfiguration {
        model: model?,
        thinking_level: thinking_level?,
        active_tool_names: active_tool_names.unwrap_or_default(),
    })
}

/// Upstream `indexLegacyV3Entry` (`legacy-v3.ts:400-447`).
fn index_legacy_v3_entry(
    entry: &LegacyV3Entry,
    line_number: usize,
    seq: i64,
    entries: &LegacyV3Entries,
) -> anyhow::Result<LegacyV3IndexEntry> {
    // SessionManager appends after an existing leaf and writes extracted
    // branches in parent order.
    let mapped_parent_id = match entry.parent_id() {
        None => None,
        Some(parent_id) => {
            let Some(parent) = entries.get(parent_id) else {
                anyhow::bail!(
                    "Legacy v3 entry {} has a missing or forward parent at line {line_number}: {}",
                    entry.id(),
                    parent_id
                );
            };
            parent.mapped_id.clone()
        }
    };
    if !entry.is_retained() {
        return Ok(match entry {
            LegacyV3Entry::Label {
                id,
                parent_id,
                target_id,
                label,
                ..
            } => LegacyV3IndexEntry {
                id: id.clone(),
                parent_id: parent_id.clone(),
                mapped_id: mapped_parent_id,
                seq: None,
                kind: LegacyV3IndexKind::Label {
                    target_id: target_id.clone(),
                    label: label.clone(),
                },
            },
            LegacyV3Entry::ModelChange {
                id,
                parent_id,
                provider,
                model_id,
                ..
            } => LegacyV3IndexEntry {
                id: id.clone(),
                parent_id: parent_id.clone(),
                mapped_id: mapped_parent_id,
                seq: None,
                kind: LegacyV3IndexKind::ModelChange {
                    provider: provider.clone(),
                    model_id: model_id.clone(),
                },
            },
            LegacyV3Entry::ThinkingLevelChange {
                id,
                parent_id,
                thinking_level,
                ..
            } => LegacyV3IndexEntry {
                id: id.clone(),
                parent_id: parent_id.clone(),
                mapped_id: mapped_parent_id,
                seq: None,
                kind: LegacyV3IndexKind::ThinkingLevelChange {
                    thinking_level: *thinking_level,
                },
            },
            LegacyV3Entry::ActiveToolsChange {
                id,
                parent_id,
                active_tool_names,
                ..
            } => LegacyV3IndexEntry {
                id: id.clone(),
                parent_id: parent_id.clone(),
                mapped_id: mapped_parent_id,
                seq: None,
                kind: LegacyV3IndexKind::ActiveToolsChange {
                    active_tool_names: active_tool_names.clone(),
                },
            },
            LegacyV3Entry::SessionInfo { id, parent_id, .. } => LegacyV3IndexEntry {
                id: id.clone(),
                parent_id: parent_id.clone(),
                mapped_id: mapped_parent_id,
                seq: None,
                kind: LegacyV3IndexKind::SessionInfo,
            },
            _ => unreachable!("discarded arm on a retained record"),
        });
    }
    let mapped_id = uuid::uuid_v7_at(entry_timestamp_ms(entry)).unwrap_or_default();
    let retained = LegacyV3IndexEntry {
        id: entry.id().to_string(),
        parent_id: entry.parent_id().map(str::to_string),
        mapped_id: Some(mapped_id),
        seq: Some(seq),
        kind: match entry {
            LegacyV3Entry::BranchSummary { from_id, .. } => LegacyV3IndexKind::BranchSummary {
                from_id: from_id.clone(),
            },
            LegacyV3Entry::Compaction {
                first_kept_entry_id,
                ..
            } => LegacyV3IndexKind::Compaction {
                first_kept_entry_id: first_kept_entry_id.clone(),
            },
            LegacyV3Entry::Message { .. } => LegacyV3IndexKind::Message,
            LegacyV3Entry::Custom { .. } => LegacyV3IndexKind::Custom,
            LegacyV3Entry::CustomMessage { .. } => LegacyV3IndexKind::CustomMessage,
            _ => unreachable!("retained arm on a discarded record"),
        },
    };
    Ok(retained)
}

/// Upstream `legacyEntryUsage` (`legacy-v3.ts:449-461`).
fn legacy_entry_usage(entry: &LegacyV3Entry) -> Option<Usage> {
    match entry {
        LegacyV3Entry::Message { message, .. } => match message {
            AgentMessage::Assistant(assistant) => Some(assistant.usage),
            AgentMessage::ToolResult(tool_result) => tool_result.usage,
            _ => None,
        },
        LegacyV3Entry::Compaction { usage, .. } | LegacyV3Entry::BranchSummary { usage, .. } => {
            *usage
        }
        _ => None,
    }
}

/// Upstream `readLegacyV3Inventory` (`legacy-v3.ts:463-484`).
async fn read_legacy_v3_inventory(
    reader: &dyn crate::agent_core::harness::types::TextLineReader,
    context: Context,
) -> anyhow::Result<LegacyV3Inventory> {
    let mut entries = LegacyV3Entries::default();
    let mut next_seq: i64 = 1;
    let mut imported_usage = Usage::default();
    let mut name: Option<String> = None;
    let mut final_id: Option<String> = None;
    loop {
        let line = file_value(
            reader.read_line(context.clone()).await,
            "Failed to read legacy v3 source",
        )?;
        let Some(line) = line else { break };
        if !line.terminated {
            break;
        }
        let line_number = entries.len() + 2;
        let entry = parse_legacy_v3_entry(&line.text)?;
        if entries.by_id.contains_key(entry.id()) {
            anyhow::bail!("Duplicate legacy v3 entry id: {}", entry.id());
        }
        let indexed = index_legacy_v3_entry(&entry, line_number, next_seq, &entries)?;
        if indexed.is_retained_kind() {
            next_seq += 1;
        }
        final_id = Some(entry.id().to_string());
        if let LegacyV3Entry::SessionInfo {
            name: session_name, ..
        } = &entry
        {
            name = session_name.clone();
        }
        if let Some(usage) = legacy_entry_usage(&entry) {
            imported_usage =
                crate::agent_core::harness::compaction::utils::add_usage(imported_usage, usage);
        }
        entries
            .by_id
            .insert(entry.id().to_string(), entries.ordered.len());
        entries.ordered.push(indexed);
    }
    Ok(LegacyV3Inventory {
        entries,
        imported_usage,
        name,
        final_id,
        next_seq,
    })
}

impl LegacyV3IndexEntry {
    fn is_retained_kind(&self) -> bool {
        self.kind.is_retained()
    }
}

/// Upstream `normalizeLegacyV3Values` (`legacy-v3.ts:486-521`).
fn normalize_legacy_v3_values(inventory: &LegacyV3Inventory) -> Vec<CommittedWrite> {
    let LegacyV3Inventory {
        entries,
        name,
        final_id,
        next_seq,
        ..
    } = inventory;
    let mut next_seq = *next_seq;
    let mut values: Vec<CommittedWrite> = Vec::new();
    let push = |key: crate::agent_core::harness::session::values::ValueAddress,
                value: serde_json::Value,
                next_seq: &mut i64| {
        let seq = *next_seq;
        *next_seq += 1;
        CommittedWrite::Value(super::super::commit::CommittedValueWrite::Set {
            seq,
            namespace: key.namespace,
            key: key.key,
            value,
        })
    };

    // Session name (upstream `if (name)` — empty/undefined clears).
    if let Some(name) = name.as_deref().filter(|name| !name.is_empty()) {
        values.push(push(
            session_name(),
            serde_json::Value::String(name.to_string()),
            &mut next_seq,
        ));
    }

    // Labels, in file order with last-wins.
    let mut labels: Vec<(String, String)> = Vec::new();
    for entry in entries.iter() {
        let LegacyV3IndexKind::Label { target_id, label } = &entry.kind else {
            continue;
        };
        let Ok(target_id) = resolve_legacy_id(entries, Some(target_id)) else {
            continue;
        };
        let Some(target_id) = target_id else { continue };
        match label {
            Some(label) if !label.is_empty() => {
                if let Some(slot) = labels.iter_mut().find(|(id, _)| *id == target_id) {
                    slot.1 = label.clone();
                } else {
                    labels.push((target_id, label.clone()));
                }
            }
            _ => labels.retain(|(id, _)| *id != target_id),
        }
    }
    for (target_id, label) in labels {
        values.push(push(
            entry_label(&target_id),
            serde_json::Value::String(label),
            &mut next_seq,
        ));
    }

    // Branch tip.
    let tip = resolve_legacy_id(entries, final_id.as_deref()).unwrap_or(None);
    values.push(push(
        branch_tip("main"),
        tip.map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
        &mut next_seq,
    ));

    // Configuration.
    if let Some(configuration) = selected_configuration(entries, final_id.as_deref()) {
        let configuration =
            serde_json::to_value(&configuration).expect("LaneConfiguration serializes");
        values.push(push(
            lane_config("main"),
            configuration.clone(),
            &mut next_seq,
        ));
        values.push(push(
            lane_state("main"),
            serde_json::json!({
                "currentOperationId": null,
                "lastOperationId": null,
                "inbox": [],
            }),
            &mut next_seq,
        ));
    }
    values
}

/// Upstream `LegacyV3Source` (`legacy-v3.ts:528-681`): a captured legacy
/// file exposed as repeatable logical v4 writes.
pub struct LegacyV3Source {
    /// Upstream `header`.
    pub header: JsonlStorageHeader,
    /// Upstream `importedUsage`.
    pub imported_usage: Usage,
    /// Upstream `nextSeq`.
    pub next_seq: i64,
    /// Upstream `values`.
    pub values: Vec<CommittedWrite>,
    file_system: Arc<dyn crate::agent_core::harness::types::FileSystem>,
    path: String,
    entries: LegacyV3Entries,
}

impl LegacyV3Source {
    #[allow(clippy::too_many_arguments)]
    fn new(
        file_system: Arc<dyn crate::agent_core::harness::types::FileSystem>,
        path: String,
        header: JsonlStorageHeader,
        entries: LegacyV3Entries,
        imported_usage: Usage,
        values: Vec<CommittedWrite>,
        next_seq: i64,
    ) -> Self {
        LegacyV3Source {
            header,
            imported_usage,
            next_seq,
            values,
            file_system,
            path,
            entries,
        }
    }

    /// Upstream `LegacyV3Source.read` (`legacy-v3.ts:563-587`): scan complete
    /// v3 records without modifying the file, ignoring an unterminated final
    /// line.
    pub async fn read(
        file_system: Arc<dyn crate::agent_core::harness::types::FileSystem>,
        path: &str,
        context: Context,
    ) -> anyhow::Result<LegacyV3Source> {
        let reader = file_value(
            file_system
                .open_text_line_reader(path, context.clone())
                .await,
            &format!("Failed to open legacy v3 source {path}"),
        )?;
        let result = async {
            let parsed = read_jsonl_header(reader.as_ref(), path, context.clone()).await?;
            if !parsed.is_v3() {
                anyhow::bail!("Invalid legacy v3 JSONL storage {path}: expected format 3 header");
            }
            let header = parsed.v3_header().expect("v3 shape").clone();
            let inventory = read_legacy_v3_inventory(reader.as_ref(), context.clone()).await?;
            let values = normalize_legacy_v3_values(&inventory);
            let normalized_header =
                normalize_legacy_v3_header(&file_system, &header, context.clone()).await;
            let next_seq = inventory.next_seq + values.len() as i64;
            Ok(LegacyV3Source::new(
                Arc::clone(&file_system),
                path.to_string(),
                normalized_header,
                inventory.entries,
                inventory.imported_usage,
                values,
                next_seq,
            ))
        }
        .await;
        reader.close(context).await;
        result
    }

    /// Upstream `entryStructures` (`legacy-v3.ts:589-594`).
    pub fn entry_structures(&self) -> Vec<LegacyV3EntryStructure> {
        self.entries
            .iter()
            .filter(|entry| entry.is_retained_kind())
            .map(|entry| LegacyV3EntryStructure {
                id: entry.mapped_id.clone().unwrap_or_default(),
                parent_id: resolve_legacy_id(&self.entries, entry.parent_id.as_deref())
                    .ok()
                    .flatten(),
                seq: entry.seq.unwrap_or_default(),
            })
            .collect()
    }

    /// Upstream `translateForkEntryId` (`legacy-v3.ts:596-601`).
    pub fn translate_fork_entry_id(&self, legacy_id: &str) -> anyhow::Result<String> {
        let entry = self
            .entries
            .get(legacy_id)
            .ok_or_else(|| anyhow::anyhow!("Legacy v3 fork entry does not exist: {legacy_id}"))?;
        if !entry.is_retained_kind() {
            anyhow::bail!("Legacy v3 fork entry is not a retained entry: {legacy_id}");
        }
        Ok(entry.mapped_id.clone().unwrap_or_default())
    }

    /// Upstream `collectRequiredTailMessageIds` (`legacy-v3.ts:603-617`).
    fn collect_required_tail_message_ids(
        &self,
        is_entry_selected: Option<&(dyn Fn(&str) -> bool + Send + Sync)>,
    ) -> anyhow::Result<HashSet<String>> {
        let mut required_ids = HashSet::new();
        for entry in self.entries.iter() {
            let LegacyV3IndexKind::Compaction {
                first_kept_entry_id,
            } = &entry.kind
            else {
                continue;
            };
            let compaction_is_selected = is_entry_selected.is_none_or(|is_selected| {
                is_selected(entry.mapped_id.as_deref().unwrap_or_default())
            });
            if !compaction_is_selected {
                continue;
            }

            // Walk from the compaction's parent through firstKeptEntryId,
            // inclusive.
            let tail = retained_tail_structure(
                &entry.id,
                entry.parent_id.as_deref(),
                first_kept_entry_id,
                &self.entries,
            )?;
            for tail_entry in tail {
                let can_produce_context_message =
                    tail_entry.is_retained_kind() && tail_entry.kind.type_str() != "custom";
                if can_produce_context_message {
                    required_ids.insert(tail_entry.id.clone());
                }
            }
        }
        Ok(required_ids)
    }

    /// Upstream `writes` (`legacy-v3.ts:623-648`): stream normalized v4
    /// entries, optionally filtered by reminted id, followed by derived
    /// current values. See the module docs for the materialization note.
    pub async fn writes(
        &self,
        context: Context,
        is_entry_selected: Option<&(dyn Fn(&str) -> bool + Send + Sync)>,
    ) -> anyhow::Result<Vec<CommittedWrite>> {
        let required_tail_message_ids =
            self.collect_required_tail_message_ids(is_entry_selected)?;
        // Keep needed context messages for this entire pass; tails may
        // revisit old or shared branches.
        let mut tail_messages_by_legacy_id: HashMap<String, AgentMessage> = HashMap::new();
        let mut writes: Vec<CommittedWrite> = Vec::new();
        for (entry, indexed) in self.read_captured_entries(context.clone()).await? {
            if required_tail_message_ids.contains(entry.id()) {
                if let Some(message) = project_context_message(&entry, &self.entries)? {
                    tail_messages_by_legacy_id.insert(entry.id().to_string(), message);
                }
            }
            if !indexed.is_retained_kind() || !entry.is_retained() {
                continue;
            }
            if let Some(is_entry_selected) = is_entry_selected {
                if !is_entry_selected(indexed.mapped_id.as_deref().unwrap_or_default()) {
                    continue;
                }
            }
            let mut retained_tail: Vec<AgentMessage> = Vec::new();
            if let LegacyV3IndexKind::Compaction {
                first_kept_entry_id,
            } = &indexed.kind
            {
                let structure = retained_tail_structure(
                    &indexed.id,
                    indexed.parent_id.as_deref(),
                    first_kept_entry_id,
                    &self.entries,
                )?;
                for ancestor in structure {
                    if let Some(message) = tail_messages_by_legacy_id.get(&ancestor.id) {
                        retained_tail.push(message.clone());
                    }
                }
                retained_tail.reverse();
            }
            writes.push(normalize_retained_entry(
                &entry,
                &indexed,
                retained_tail,
                &self.entries,
            )?);
        }
        writes.extend(self.values.iter().cloned());
        Ok(writes)
    }

    /// Upstream `readCapturedEntries` (`legacy-v3.ts:651-681`): replay only
    /// the captured prefix and verify its physical identities.
    async fn read_captured_entries(
        &self,
        context: Context,
    ) -> anyhow::Result<Vec<(LegacyV3Entry, LegacyV3IndexEntry)>> {
        let reader = file_value(
            self.file_system
                .open_text_line_reader(&self.path, context.clone())
                .await,
            &format!("Failed to reopen legacy v3 source {}", self.path),
        )?;
        let result = async {
            let parsed = read_jsonl_header(reader.as_ref(), &self.path, context.clone()).await?;
            match &parsed {
                parsed if parsed.is_v3() => {
                    let header = parsed.v3_header().expect("v3 shape");
                    if header.id != self.header.id || header.cwd != self.header.cwd {
                        anyhow::bail!("Legacy v3 source header changed");
                    }
                }
                _ => anyhow::bail!("Legacy v3 source header changed"),
            }
            let mut captured = Vec::with_capacity(self.entries.len());
            for indexed in self.entries.iter() {
                let line = file_value(
                    reader.read_line(context.clone()).await,
                    "Failed to reread legacy v3 source",
                )?;
                let Some(line) = line else {
                    anyhow::bail!("Legacy v3 source ended before captured entries");
                };
                if !line.terminated {
                    anyhow::bail!("Legacy v3 source ended before captured entries");
                }
                let entry = parse_legacy_v3_entry(&line.text)?;
                if entry.id() != indexed.id || entry.type_str() != indexed.kind.type_str() {
                    anyhow::bail!("Legacy v3 source changed");
                }
                captured.push((entry, indexed.clone()));
            }
            Ok(captured)
        }
        .await;
        reader.close(context).await;
        result
    }
}

/// One `entryStructures()` row (upstream
/// `Pick<CommittedEntryWrite, "id" | "parentId" | "seq">`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyV3EntryStructure {
    pub id: String,
    pub parent_id: Option<String>,
    pub seq: i64,
}

#[cfg(test)]
mod tests;
