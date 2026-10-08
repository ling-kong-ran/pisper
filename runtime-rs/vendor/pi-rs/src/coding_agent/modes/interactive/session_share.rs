//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of the deterministic core of upstream
//! `coding-agent/src/modes/interactive/session-share.ts`
//! (206 lines, sha256 `f5add6fccb39e747507a421d9166113a1a864b8063bfb1e948319dfb6ede6852`)
//! and its dependency `coding-agent/src/core/session-export.ts`
//! (`exportSessionToJsonl`).
//!
//! The share export writes the current session branch with presentation
//! metadata for Radius: a trailing `custom` entry of type `pi.share` carrying
//! the system prompt and the presentable tool surface, without changing
//! conversation ids or parent links.
//!
//! # Seams
//!
//! - **Transport and TUI presentation are not ported.** `shareSession`,
//!   `tryShareViaRadius`, `shareViaGist`, the `BorderedLoader` editor-swap
//!   choreography, `getAuthCredential`, and the `DEFAULT_RADIUS_GATEWAY`
//!   upload are interactive presentation over live network/`gh` processes;
//!   this module ports the deterministic export core the transports consume
//!   (`exportSessionForShare`'s document construction).
//! - **`crypto.randomUUID().slice(0, 8)`** (session-share.ts:30): the share
//!   entry id becomes an explicit parameter; callers mint it the same way
//!   ids are minted elsewhere in the port (injected, not global RNG).
//! - **`new Date().toISOString()`** (session-export.ts:25,23): the timestamp
//!   becomes a parameter on [`export_session_to_jsonl_at`]; the
//!   [`export_session_to_jsonl`] wrapper reads the real clock, matching
//!   upstream's single shared timestamp for header and trailing entries.
//! - **`{ ...entry, parentId }` spread** (session-export.ts:31): JS rewrites
//!   `parentId` in place, preserving key position; the port mirrors that
//!   with the same field-position rewrite the `agent_session` export uses.
//! - **Trailing-entry key order.** Upstream's `pi.share` object literal
//!   writes `type, customType, id, parentId, timestamp, data` — `data`
//!   **last**, unlike the session-manager's `appendCustomEntry` literal that
//!   the `CustomEntry` serde model follows (`data` after `customType`). The
//!   share entry is therefore built as an ordered JSON value, not a
//!   `CustomEntry`.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use crate::coding_agent::agent_session::AgentSession;
use crate::coding_agent::extensions::types::ToolInfo;
use crate::coding_agent::session_manager::{SessionEntry, SessionManager, CURRENT_SESSION_VERSION};
use crate::coding_agent::utils::paths::resolve_path;

/// Upstream literal `"pi.share"` (session-share.ts:29).
pub const SHARE_CUSTOM_TYPE: &str = "pi.share";

/// A presentable tool (session-share.ts:35-39 keeps `name`, `description`,
/// and `parameters` off the ported [`ToolInfo`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ShareToolPresentation {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Map the session's tool surface to the share presentation
/// (session-share.ts:35-39 reads `session.state.tools`).
pub fn share_tool_presentations(tools: &[ToolInfo]) -> Vec<ShareToolPresentation> {
    tools
        .iter()
        .map(|tool| ShareToolPresentation {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        })
        .collect()
}

/// Error surfaced by the share export (upstream reports
/// `Failed to export session: ${error.message}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionShareError {
    pub message: String,
}

impl SessionShareError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SessionShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SessionShareError {}

/// Build the trailing `pi.share` custom entry
/// (session-share.ts:27-41, with the id and timestamp injected — see the
/// module seams). Key order is the upstream object literal's.
pub fn share_custom_entry(
    parent_id: Option<&str>,
    timestamp: &str,
    share_id: &str,
    system_prompt: &str,
    tools: &[ShareToolPresentation],
) -> Value {
    let mut entry = Map::new();
    entry.insert("type".to_string(), Value::String("custom".to_string()));
    entry.insert(
        "customType".to_string(),
        Value::String(SHARE_CUSTOM_TYPE.to_string()),
    );
    entry.insert("id".to_string(), Value::String(share_id.to_string()));
    entry.insert(
        "parentId".to_string(),
        match parent_id {
            Some(parent) => Value::String(parent.to_string()),
            None => Value::Null,
        },
    );
    entry.insert(
        "timestamp".to_string(),
        Value::String(timestamp.to_string()),
    );
    let mut data = Map::new();
    data.insert(
        "systemPrompt".to_string(),
        Value::String(system_prompt.to_string()),
    );
    data.insert(
        "tools".to_string(),
        Value::Array(
            tools
                .iter()
                .map(|tool| {
                    let mut presentation = Map::new();
                    presentation.insert("name".to_string(), Value::String(tool.name.clone()));
                    presentation.insert(
                        "description".to_string(),
                        Value::String(tool.description.clone()),
                    );
                    presentation.insert("parameters".to_string(), tool.parameters.clone());
                    Value::Object(presentation)
                })
                .collect(),
        ),
    );
    entry.insert("data".to_string(), Value::Object(data));
    Value::Object(entry)
}

/// Rewrite an entry's `parentId` in place. Field-position-preserving
/// equivalent of the upstream `{ ...entry, parentId }` spread
/// (session-export.ts:31); duplicated from the `agent_session` export's
/// private helper (same slice conventions, kept local so this module owns
/// its serialization path).
fn set_entry_parent_id(entry: &mut SessionEntry, parent_id: Option<String>) {
    match entry {
        SessionEntry::Message(e) => e.parent_id = parent_id,
        SessionEntry::ThinkingLevelChange(e) => e.parent_id = parent_id,
        SessionEntry::ModelChange(e) => e.parent_id = parent_id,
        SessionEntry::Usage(e) => e.parent_id = parent_id,
        SessionEntry::ContextEdit(e) => e.parent_id = parent_id,
        SessionEntry::Compaction(e) => e.parent_id = parent_id,
        SessionEntry::BranchSummary(e) => e.parent_id = parent_id,
        SessionEntry::Custom(e) => e.parent_id = parent_id,
        SessionEntry::CustomMessage(e) => e.parent_id = parent_id,
        SessionEntry::Label(e) => e.parent_id = parent_id,
        SessionEntry::SessionInfo(e) => e.parent_id = parent_id,
        SessionEntry::Unparsed(value) => match parent_id {
            Some(parent) => value["parentId"] = Value::String(parent),
            None => {
                if let Some(object) = value.as_object_mut() {
                    object.shift_remove("parentId");
                }
            }
        },
    }
}

/// The current wall-clock ISO stamp (upstream `new Date().toISOString()`).
fn now_iso_stamp() -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default();
    crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(now_ms)
}

/// Upstream `exportSessionToJsonl` (session-export.ts:15-41) with the export
/// timestamp injected (see the module seams). Writes the `session` header
/// line, the branch entries re-chained along the export path, and the
/// trailing export-only entries; newline-terminated. Relative output paths
/// resolve against the process cwd.
pub fn export_session_to_jsonl_at(
    session_manager: &Mutex<SessionManager>,
    output_path: Option<&str>,
    timestamp: &str,
    create_trailing_entries: &dyn Fn(Option<&str>, &str) -> Vec<Value>,
) -> Result<String, SessionShareError> {
    let base_dir = std::env::current_dir()
        .map(|dir| dir.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());
    // upstream default name: `session-${now.replace(/[:.]/g, "-")}.jsonl`.
    let default_name = format!("session-{}.jsonl", timestamp.replace([':', '.'], "-"));
    let file_path = resolve_path(output_path.unwrap_or(&default_name), &base_dir)
        .map_err(|error| SessionShareError::new(error.to_string()))?;
    // upstream `if (!existsSync(dir)) mkdirSync(dir, { recursive: true })`.
    if let Some(parent) = std::path::Path::new(&file_path).parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|error| SessionShareError::new(error.to_string()))?;
        }
    }

    let content = {
        let manager = session_manager.lock().expect("session lock");
        let mut lines = vec![format!(
            "{{\"type\":\"session\",\"version\":{},\"id\":{},\"timestamp\":{},\"cwd\":{}}}",
            CURRENT_SESSION_VERSION,
            Value::String(manager.get_session_id().to_string()),
            Value::String(timestamp.to_string()),
            Value::String(manager.get_cwd().to_string()),
        )];

        let mut parent_id: Option<String> = None;
        for mut entry in manager.get_branch(None) {
            set_entry_parent_id(&mut entry, parent_id.clone());
            lines.push(serde_json::to_string(&entry).map_err(|error| {
                SessionShareError::new(format!("failed to serialize session entry: {error}"))
            })?);
            parent_id = entry.id().map(str::to_string);
        }
        for entry in create_trailing_entries(parent_id.as_deref(), timestamp) {
            lines.push(serde_json::to_string(&entry).map_err(|error| {
                SessionShareError::new(format!("failed to serialize share entry: {error}"))
            })?);
        }
        format!("{}\n", lines.join("\n"))
    };
    std::fs::write(&file_path, content)
        .map_err(|error| SessionShareError::new(error.to_string()))?;
    Ok(file_path)
}

/// Upstream `exportSessionToJsonl` at the real clock (the `_at` variant is
/// the injectable core; see the module seams).
pub fn export_session_to_jsonl(
    session_manager: &Mutex<SessionManager>,
    output_path: Option<&str>,
    create_trailing_entries: &dyn Fn(Option<&str>, &str) -> Vec<Value>,
) -> Result<String, SessionShareError> {
    let timestamp = now_iso_stamp();
    export_session_to_jsonl_at(
        session_manager,
        output_path,
        &timestamp,
        create_trailing_entries,
    )
}

/// Upstream `exportSessionForShare` (session-share.ts:25-43): export the
/// current branch with the `pi.share` presentation entry for Radius. The
/// share entry id is injected (upstream mints
/// `crypto.randomUUID().slice(0, 8)`); the transport/TUI half of upstream
/// `shareSession` is not ported (see the module seams).
pub fn export_session_for_share(
    file_path: &str,
    session: &AgentSession,
    share_id: &str,
) -> Result<String, SessionShareError> {
    let system_prompt = session.system_prompt();
    let tools = share_tool_presentations(&session.get_all_tools());
    export_session_to_jsonl(
        &session.session_manager,
        Some(file_path),
        &|parent_id, timestamp| {
            vec![share_custom_entry(
                parent_id,
                timestamp,
                share_id,
                &system_prompt,
                &tools,
            )]
        },
    )
}

/// Convenience for the behavioral tests and callers holding a bare manager:
/// the share export over a `SessionManager` with an explicit presentation
/// payload (upstream reads the payload off `session.state`).
pub fn export_session_for_share_with_presentation(
    file_path: &str,
    session_manager: &Mutex<SessionManager>,
    share_id: &str,
    system_prompt: &str,
    tools: &[ShareToolPresentation],
) -> Result<String, SessionShareError> {
    export_session_to_jsonl(session_manager, Some(file_path), &|parent_id, timestamp| {
        vec![share_custom_entry(
            parent_id,
            timestamp,
            share_id,
            system_prompt,
            tools,
        )]
    })
}
