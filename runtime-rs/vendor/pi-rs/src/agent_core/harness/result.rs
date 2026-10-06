//! Port of `packages/agent/src/harness/result.ts` (117 lines): the tagged
//! harness error values, the fault/closed errors, and the lane/operation
//! error vocabulary the harness API returns.
//!
//! Upstream models these as `Error` subclasses built by a `TaggedError(tag)`
//! factory, discriminated by a readonly `_tag` property. The port is a closed
//! enum, [`TaggedError`], with one variant per upstream class carrying the
//! same fields plus the message; [`TaggedError::tag`] reproduces `_tag`,
//! [`TaggedError::to_json`] reproduces `toJSON()` (`{"_tag", "message", ...}`
//! with the message moved to the front), and `matchError` maps onto a plain
//! `match`, so the enum variant set replaces the `ErrorMatchers` table.
//!
//! Disclosed deviations:
//! - `Error.prototype.name` (upstream `this.name = tag`) has no Rust
//!   equivalent; `Display` prints `"{tag}: {message}"` so the tag is visible
//!   in logs and panic messages.
//! - `toJSON()` spreads own properties except `_tag`; the port's field set is
//!   exactly the constructor's props, so `to_json` emits `_tag`, `message`,
//!   and the variant's payload fields.

use serde::ser::{SerializeMap as _, Serializer};
use serde::Serialize;
use std::fmt;

use crate::agent_core::chord_support::JsonValue;

/// Upstream `Result<TValue, TError>` (`types.ts:9`) and its `ok`/`err`
/// helpers map onto [`std::result::Result`] with `Ok`/`Err`; the alias keeps
/// ported signatures readable.
pub type Result<TValue, TError> = std::result::Result<TValue, TError>;

/// Upstream tagged error classes (`result.ts:53-88`): one variant per class,
/// carrying the class's props plus its `message`. The `_tag` discriminator is
/// the variant name; see [`TaggedError::tag`] for the upstream string values.
#[derive(Debug, Clone, PartialEq)]
pub enum TaggedError {
    /// `LaneBusy` (`result.ts:53-58`).
    LaneBusy {
        lane: String,
        operation_id: String,
        /// `"run" | "compaction" | "navigation"` (`result.ts:56`).
        operation_kind: OperationKind,
        message: String,
    },
    /// `OperationMismatch` (`result.ts:59-65`).
    OperationMismatch {
        lane: String,
        expected_operation_id: String,
        current_operation_id: Option<String>,
        last_operation_id: Option<String>,
        message: String,
    },
    /// `NoActiveRun` (`result.ts:66`).
    NoActiveRun { lane: String, message: String },
    /// `NoActiveOperation` (`result.ts:67`).
    NoActiveOperation { lane: String, message: String },
    /// `NothingToResume` (`result.ts:68`).
    NothingToResume { lane: String, message: String },
    /// `NothingToCompact` (`result.ts:69`).
    NothingToCompact { lane: String, message: String },
    /// `InvalidMessage` (`result.ts:70-74`).
    InvalidMessage {
        lane: String,
        reason: String,
        message: String,
    },
    /// `InvalidNavigation` (`result.ts:75-79`).
    InvalidNavigation {
        lane: String,
        reason: String,
        message: String,
    },
    /// `UnknownSkill` (`result.ts:80`).
    UnknownSkill { name: String, message: String },
    /// `UnknownTemplate` (`result.ts:81`).
    UnknownTemplate { name: String, message: String },
    /// `UnknownTarget` (`result.ts:82`).
    UnknownTarget { target_id: String, message: String },
    /// `InvalidLane` (`result.ts:83-87`).
    InvalidLane {
        lane: String,
        reason: String,
        message: String,
    },
    /// `Closed` (`result.ts:88`).
    Closed { message: String },
}

/// The `operationKind` union of `LaneBusy` (`result.ts:56`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationKind {
    Run,
    Compaction,
    Navigation,
}

impl TaggedError {
    /// Upstream `_tag` (`result.ts:19`): the stable discriminator string.
    pub fn tag(&self) -> &'static str {
        match self {
            TaggedError::LaneBusy { .. } => "LaneBusy",
            TaggedError::OperationMismatch { .. } => "OperationMismatch",
            TaggedError::NoActiveRun { .. } => "NoActiveRun",
            TaggedError::NoActiveOperation { .. } => "NoActiveOperation",
            TaggedError::NothingToResume { .. } => "NothingToResume",
            TaggedError::NothingToCompact { .. } => "NothingToCompact",
            TaggedError::InvalidMessage { .. } => "InvalidMessage",
            TaggedError::InvalidNavigation { .. } => "InvalidNavigation",
            TaggedError::UnknownSkill { .. } => "UnknownSkill",
            TaggedError::UnknownTemplate { .. } => "UnknownTemplate",
            TaggedError::UnknownTarget { .. } => "UnknownTarget",
            TaggedError::InvalidLane { .. } => "InvalidLane",
            TaggedError::Closed { .. } => "Closed",
        }
    }

    /// The error `message` prop.
    pub fn message(&self) -> &str {
        match self {
            TaggedError::LaneBusy { message, .. }
            | TaggedError::OperationMismatch { message, .. }
            | TaggedError::NoActiveRun { message, .. }
            | TaggedError::NoActiveOperation { message, .. }
            | TaggedError::NothingToResume { message, .. }
            | TaggedError::NothingToCompact { message, .. }
            | TaggedError::InvalidMessage { message, .. }
            | TaggedError::InvalidNavigation { message, .. }
            | TaggedError::UnknownSkill { message, .. }
            | TaggedError::UnknownTemplate { message, .. }
            | TaggedError::UnknownTarget { message, .. }
            | TaggedError::InvalidLane { message, .. }
            | TaggedError::Closed { message } => message,
        }
    }

    /// Upstream `toJSON()` (`result.ts:38-44`): `{ _tag, message, ...props }`
    /// as a strict-JSON value. Field order follows the constructor props (the
    /// spread order), with `_tag` and `message` leading.
    pub fn to_json(&self) -> JsonValue {
        let mut payload = serde_json::Map::new();
        payload.insert("_tag".into(), self.tag().into());
        payload.insert("message".into(), self.message().into());
        match self {
            TaggedError::LaneBusy {
                lane,
                operation_id,
                operation_kind,
                ..
            } => {
                payload.insert("lane".into(), lane.clone().into());
                payload.insert("operationId".into(), operation_id.clone().into());
                payload.insert(
                    "operationKind".into(),
                    serde_json::to_value(operation_kind).expect("operationKind serializes"),
                );
            }
            TaggedError::OperationMismatch {
                lane,
                expected_operation_id,
                current_operation_id,
                last_operation_id,
                ..
            } => {
                payload.insert("lane".into(), lane.clone().into());
                payload.insert(
                    "expectedOperationId".into(),
                    expected_operation_id.clone().into(),
                );
                payload.insert(
                    "currentOperationId".into(),
                    current_operation_id.clone().into(),
                );
                payload.insert("lastOperationId".into(), last_operation_id.clone().into());
            }
            TaggedError::NoActiveRun { lane, .. }
            | TaggedError::NoActiveOperation { lane, .. }
            | TaggedError::NothingToResume { lane, .. }
            | TaggedError::NothingToCompact { lane, .. } => {
                payload.insert("lane".into(), lane.clone().into());
            }
            TaggedError::InvalidMessage { lane, reason, .. }
            | TaggedError::InvalidNavigation { lane, reason, .. }
            | TaggedError::InvalidLane { lane, reason, .. } => {
                payload.insert("lane".into(), lane.clone().into());
                payload.insert("reason".into(), reason.clone().into());
            }
            TaggedError::UnknownSkill { name, .. } | TaggedError::UnknownTemplate { name, .. } => {
                payload.insert("name".into(), name.clone().into());
            }
            TaggedError::UnknownTarget { target_id, .. } => {
                payload.insert("targetId".into(), target_id.clone().into());
            }
            TaggedError::Closed { .. } => {}
        }
        JsonValue::Object(payload)
    }
}

impl fmt::Display for TaggedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.tag(), self.message())
    }
}

impl std::error::Error for TaggedError {}

impl Serialize for TaggedError {
    /// Wire shape is the `toJSON()` payload (upstream errors serialize through
    /// their `toJSON` when persisted).
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let payload = self.to_json();
        let serde_json::Value::Object(entries) = payload else {
            unreachable!("to_json returns an object");
        };
        let mut map = serializer.serialize_map(Some(entries.len()))?;
        for (key, value) in &entries {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

/// Upstream `HarnessFault` (`result.ts:90-98`): an unexpected harness failure
/// carrying its underlying cause. The port's cause is the anyhow error chain.
#[derive(Debug)]
pub struct HarnessFault {
    pub message: String,
    pub cause: anyhow::Error,
}

impl HarnessFault {
    pub fn new(message: impl Into<String>, cause: anyhow::Error) -> Self {
        HarnessFault {
            message: message.into(),
            cause,
        }
    }
}

impl fmt::Display for HarnessFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.message, self.cause)
    }
}

impl std::error::Error for HarnessFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

/// Upstream `HarnessClosed` (`result.ts:100-105`): raised for operations still
/// active when the harness closed. The message is a fixed literal upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessClosed;

impl HarnessClosed {
    /// Upstream `new HarnessClosed()` (`result.ts:101-103`).
    pub fn message() -> &'static str {
        "AgentHarness was closed while the operation was active"
    }
}

impl fmt::Display for HarnessClosed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(Self::message())
    }
}

impl std::error::Error for HarnessClosed {}

#[cfg(test)]
mod tests;
