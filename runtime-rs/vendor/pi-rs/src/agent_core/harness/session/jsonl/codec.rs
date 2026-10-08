//! Port of `packages/agent/src/harness/session/jsonl/codec.ts` (63 lines):
//! the session-header grammar — the v3 legacy shape and the format-4
//! storage header, discriminated by `type`/`kind`.

use serde::{Deserialize, Serialize};

use super::iso8601::parse_iso8601_utc;
use super::types::{HeaderKind, JsonlStorageHeader, JSONL_FORMAT_VERSION};
use crate::agent_core::harness::session::types::SessionMetadata;

/// Upstream `LegacyV3SessionHeader` (`codec.ts:4-12`): the pre-WP01
/// SessionManager-era header line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyV3SessionHeader {
    /// Always `"session"`.
    pub r#type: LegacyHeaderType,
    /// Always `3` (validated in [`parse_legacy_v3_session_header`], like
    /// upstream `value.version === 3`).
    pub version: i64,
    pub id: String,
    /// ISO timestamp string (upstream `Date.parse`d at the consumers).
    pub timestamp: String,
    pub cwd: String,
    /// Upstream `parentSession: string | undefined` — a JSON `null` is NOT
    /// accepted (serde `Option` would collapse null and absent; the custom
    /// deserializer keeps upstream's distinction).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_string_or_absent"
    )]
    pub parent_session: Option<String>,
}

/// Upstream `string | undefined` field handling: absent maps to `None`,
/// `null` is a deserialization error (upstream
/// `typeof value.parentSession === "string"`), a string maps to `Some`.
fn deserialize_string_or_absent<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<String> = Option::deserialize(deserializer)?;
    match value {
        Some(text) => Ok(Some(text)),
        // `Option::deserialize` maps a JSON null to None; reject it.
        None => Err(serde::de::Error::custom(
            "parentSession must be a string or absent",
        )),
    }
}

/// Upstream `type: "session"` literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyHeaderType {
    Session,
}

/// Upstream `JsonlParsedSessionHeader` (`codec.ts:49-51`).
#[derive(Debug, Clone, PartialEq)]
pub enum JsonlParsedSessionHeader {
    V4 { header: JsonlStorageHeader },
    V3Legacy { header: LegacyV3SessionHeader },
}

impl JsonlParsedSessionHeader {
    /// Whether the parsed header is the v3-legacy format.
    pub fn is_v3(&self) -> bool {
        matches!(self, JsonlParsedSessionHeader::V3Legacy { .. })
    }

    /// The legacy header, when this is the v3-legacy format.
    pub fn v3_header(&self) -> Option<&LegacyV3SessionHeader> {
        match self {
            JsonlParsedSessionHeader::V3Legacy { header } => Some(header),
            JsonlParsedSessionHeader::V4 { .. } => None,
        }
    }

    /// The session id shared by both formats.
    pub fn header_id(&self) -> &str {
        match self {
            JsonlParsedSessionHeader::V4 { header } => &header.id,
            JsonlParsedSessionHeader::V3Legacy { header } => &header.id,
        }
    }
}

/// Upstream `isLegacyV3SessionHeader` (`codec.ts:21-32`): `type ===
/// "session"`, `version === 3`, string id/cwd/timestamp with a parseable
/// timestamp, and `parentSession` a string or absent. The port deserializes
/// the JSON value into the typed header (serde enforces the field types and
/// the null-parentSession rejection); this function adds the literal
/// `version === 3` check upstream performs.
pub fn parse_legacy_v3_session_header(value: &serde_json::Value) -> Option<LegacyV3SessionHeader> {
    let header: LegacyV3SessionHeader = serde_json::from_value(value.clone()).ok()?;
    if header.version != 3 {
        return None;
    }
    parse_iso8601_utc(&header.timestamp)?;
    Some(header)
}

/// Upstream `isJsonlStorageHeader` (`codec.ts:34-47`).
pub fn parse_jsonl_storage_header(value: &serde_json::Value) -> Option<JsonlStorageHeader> {
    let header: JsonlStorageHeader = serde_json::from_value(value.clone()).ok()?;
    if header.kind != HeaderKind::Header
        || header.v != JSONL_FORMAT_VERSION
        || header.storage_version < 1
        || header.created_at < 0
        || header.next_seq.is_some_and(|next_seq| next_seq < 1)
    {
        return None;
    }
    Some(header)
}

/// Upstream `parseJsonlSessionHeader(line)` (`codec.ts:53-63`): parse one
/// header line, v4 first. `Err` carries the upstream error messages.
pub fn parse_jsonl_session_header(line: &str) -> anyhow::Result<JsonlParsedSessionHeader> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
        anyhow::anyhow!("Invalid JSONL session header: not valid JSON: {error}")
    })?;
    if let Some(header) = parse_jsonl_storage_header(&value) {
        return Ok(JsonlParsedSessionHeader::V4 { header });
    }
    if let Some(header) = parse_legacy_v3_session_header(&value) {
        return Ok(JsonlParsedSessionHeader::V3Legacy { header });
    }
    anyhow::bail!("Unsupported JSONL session header")
}

/// Upstream `metadataFromHeader` (`repo.ts:21-34`) projection into the
/// flattened session metadata.
pub fn metadata_from_header(
    header: &JsonlStorageHeader,
    path: &str,
    modified_at: f64,
) -> SessionMetadata {
    SessionMetadata {
        id: header.id.clone(),
        created_at: header.created_at,
        storage_version: header.storage_version,
        cwd: Some(header.cwd.clone()),
        parent_session_id: header.parent_session_id.clone(),
        legacy_parent_session_path: header.legacy_parent_session_path.clone(),
        path: Some(path.to_string()),
        modified_at: Some(modified_at),
    }
}

#[cfg(test)]
mod tests;
