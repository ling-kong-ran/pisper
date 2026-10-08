//! Port of `packages/agent/src/harness/session/values.ts` (195 lines): the
//! stored-value addressing vocabulary — `Value<T>`/`ValueList<T>` addresses,
//! read options, the four write constructors, and the reserved `pi.*`
//! address helpers every session consumer binds to.
//!
//! Disclosed substitution: upstream `Value<T>`/`ValueList<T>` are generically
//! typed opaque handles (a phantom `storedValueType` marker); the port
//! erases the marker to a plain [`ValueAddress`] (see the session module
//! docs). `value<T>(namespace, key)`/`list<T>(...)` map to
//! [`value`]/[`list`]; every upstream address helper keeps its exact
//! namespace and key-composition literals.

use crate::agent_core::types::ThinkingLevel;
use serde::{Deserialize, Serialize};

use super::types::LaneConfiguration;

/// Upstream `StoredAddressBase` (`values.ts:16-20`) erased to the shared
/// address shape; `kind` distinguishes `value` from `list`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueAddress {
    pub namespace: String,
    pub key: String,
    /// Upstream `kind: "value" | "list"`.
    pub kind: AddressKind,
}

/// Upstream `kind` literal (`values.ts:19`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressKind {
    Value,
    List,
}

/// Upstream `StoredValue<T>` (`values.ts:32-36`) with the payload erased to
/// JSON (upstream `unknown` at runtime).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredValue {
    pub address: ValueAddress,
    pub value: serde_json::Value,
    pub seq: i64,
}

/// Upstream `ListElement<T>` (`values.ts:38-41`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListElement {
    pub seq: i64,
    pub value: serde_json::Value,
}

/// Upstream `ListCursor` (`values.ts:43-45`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListCursor {
    pub seq: i64,
}

/// Upstream `ListReadOptions` (`values.ts:47-51`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListReadOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ListCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<super::types::AscDescOrder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// Upstream `ResolvedListReadOptions` (`values.ts:53-57`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedListReadOptions {
    pub cursor: Option<ListCursor>,
    pub order: super::types::AscDescOrder,
    pub limit: u64,
}

/// Upstream `value<T>(namespace, key = "")` (`values.ts:98-101`) with the
/// address validation (`values.ts:92-96`).
///
/// # Panics
/// Upstream throws `TypeError` for empty namespaces or NUL bytes; the port
/// panics with the same message (constructor contract violations, not
/// runtime failures).
pub fn value(namespace: &str, key: &str) -> ValueAddress {
    validate_address(namespace, key);
    ValueAddress {
        namespace: namespace.to_string(),
        key: key.to_string(),
        kind: AddressKind::Value,
    }
}

/// Upstream `list<T>(namespace, key = "")` (`values.ts:103-106`).
///
/// # Panics
/// Same contract as [`value`].
pub fn list(namespace: &str, key: &str) -> ValueAddress {
    validate_address(namespace, key);
    ValueAddress {
        namespace: namespace.to_string(),
        key: key.to_string(),
        kind: AddressKind::List,
    }
}

/// Upstream `validateAddress` (`values.ts:92-96`).
fn validate_address(namespace: &str, key: &str) {
    if namespace.is_empty() {
        panic!("Value namespace must not be empty");
    }
    if namespace.contains('\0') {
        panic!("Value namespace must not contain \\u0000");
    }
    if key.contains('\0') {
        panic!("Value key must not contain \\u0000");
    }
}

/// Upstream `setValue(address, next)` (`values.ts:108-116`) — the staged
/// write constructor.
pub fn set_value(address: &ValueAddress, next: serde_json::Value) -> super::types::Write {
    super::types::Write::Value(super::types::ValueWrite::Set {
        namespace: address.namespace.clone(),
        key: address.key.clone(),
        value: next,
    })
}

/// Upstream `deleteValue(address)` (`values.ts:118-125`).
pub fn delete_value(address: &ValueAddress) -> super::types::Write {
    super::types::Write::Value(super::types::ValueWrite::Delete {
        namespace: address.namespace.clone(),
        key: address.key.clone(),
    })
}

/// Upstream `appendList(address, element)` (`values.ts:127-135`).
pub fn append_list(address: &ValueAddress, element: serde_json::Value) -> super::types::Write {
    super::types::Write::List(super::types::ListWrite::Append {
        namespace: address.namespace.clone(),
        key: address.key.clone(),
        value: element,
    })
}

/// Upstream `deleteList(address)` (`values.ts:137-144`).
pub fn delete_list(address: &ValueAddress) -> super::types::Write {
    super::types::Write::List(super::types::ListWrite::Delete {
        namespace: address.namespace.clone(),
        key: address.key.clone(),
    })
}

/// Upstream `resolveListReadOptions(options = {})` (`values.ts:146-156`).
/// Upstream throws `TypeError` for non-positive limits; the port returns
/// `Err` (the storage methods surface it as a rejected read).
pub fn resolve_list_read_options(
    options: Option<&ListReadOptions>,
) -> anyhow::Result<ResolvedListReadOptions> {
    let fallback = ListReadOptions::default();
    let options = options.unwrap_or(&fallback);
    let requested_limit = options.limit.unwrap_or(1_000);
    if requested_limit == 0 {
        anyhow::bail!("List read limit must be a positive safe integer");
    }
    Ok(ResolvedListReadOptions {
        cursor: options.cursor,
        order: options.order.unwrap_or(super::types::AscDescOrder::Asc),
        limit: requested_limit.min(10_000),
    })
}

/// Upstream `branchTip(branch)` (`values.ts:158`).
pub fn branch_tip(branch: &str) -> ValueAddress {
    value("pi.branch.tip", branch)
}

/// Upstream `branchTipInventoryPrefix()` (`values.ts:159`).
pub fn branch_tip_inventory_prefix() -> ValueAddress {
    value("pi.branch.tip", "")
}

/// Upstream `laneConfig(lane)` (`values.ts:160`).
pub fn lane_config(lane: &str) -> ValueAddress {
    value("pi.lane.config", lane)
}

/// Upstream `laneState(lane)` (`values.ts:161`).
pub fn lane_state(lane: &str) -> ValueAddress {
    value("pi.lane.state", lane)
}

/// Upstream `operationResult(operationId)` (`values.ts:162`).
pub fn operation_result(operation_id: &str) -> ValueAddress {
    value("pi.result", operation_id)
}

/// Upstream `operationMeta(operationId)` (`values.ts:164`).
pub fn operation_meta(operation_id: &str) -> ValueAddress {
    value("pi.op.meta", operation_id)
}

/// Upstream `operationState(operationId)` (`values.ts:165`).
pub fn operation_state(operation_id: &str) -> ValueAddress {
    value("pi.op.state", operation_id)
}

/// Upstream `operationToolArgs(operationId, stepId, sourceIndex)`
/// (`values.ts:166-167`).
pub fn operation_tool_args(operation_id: &str, step_id: &str, source_index: i64) -> ValueAddress {
    value(
        "pi.op.tool_args",
        &format!("{operation_id}:{step_id}:{source_index}"),
    )
}

/// Upstream `operationToolMemo(operationId, invocationId, name)`
/// (`values.ts:168-169`).
pub fn operation_tool_memo(operation_id: &str, invocation_id: &str, name: &str) -> ValueAddress {
    value(
        "pi.op.tool_memo",
        &format!("{operation_id}:{invocation_id}:{name}"),
    )
}

/// Upstream `operationPreparation(operationId, taskId)` (`values.ts:170-171`).
pub fn operation_preparation(operation_id: &str, task_id: &str) -> ValueAddress {
    value("pi.op.preparation", &format!("{operation_id}:{task_id}"))
}

/// Upstream `operationToolArgsPrefix(operationId, stepId?)`
/// (`values.ts:173-177`).
pub fn operation_tool_args_prefix(operation_id: &str, step_id: Option<&str>) -> ValueAddress {
    match step_id {
        Some(step_id) => value("pi.op.tool_args", &format!("{operation_id}:{step_id}:")),
        None => value("pi.op.tool_args", &format!("{operation_id}:")),
    }
}

/// Upstream `operationToolMemoPrefix(operationId, invocationId?)`
/// (`values.ts:178-182`).
pub fn operation_tool_memo_prefix(operation_id: &str, invocation_id: Option<&str>) -> ValueAddress {
    match invocation_id {
        Some(invocation_id) => value(
            "pi.op.tool_memo",
            &format!("{operation_id}:{invocation_id}:"),
        ),
        None => value("pi.op.tool_memo", &format!("{operation_id}:")),
    }
}

/// Upstream `operationPreparationPrefix(operationId)` (`values.ts:183-184`).
pub fn operation_preparation_prefix(operation_id: &str) -> ValueAddress {
    value("pi.op.preparation", &format!("{operation_id}:"))
}

/// Upstream `pendingEntry(entryId)` (`values.ts:186`).
pub fn pending_entry(entry_id: &str) -> ValueAddress {
    value("pi.pending.entry", entry_id)
}

/// Upstream `pendingToolOutput(operationId, invocationId)`
/// (`values.ts:187-188`). The generic payload is the harness-erased
/// [`AgentToolResult`] at runtime.
pub fn pending_tool_output(operation_id: &str, invocation_id: &str) -> ValueAddress {
    value(
        "pi.pending.tool_output",
        &format!("{operation_id}:{invocation_id}"),
    )
}

/// Upstream `pendingAssistantFrames(operationId, responseEntryId)`
/// (`values.ts:189-190`): a list address.
pub fn pending_assistant_frames(operation_id: &str, response_entry_id: &str) -> ValueAddress {
    list(
        "pi.pending.assistant_frame",
        &format!("{operation_id}:{response_entry_id}"),
    )
}

/// Upstream `pendingToolOutputPrefix(operationId)` (`values.ts:191-192`).
pub fn pending_tool_output_prefix(operation_id: &str) -> ValueAddress {
    value("pi.pending.tool_output", &format!("{operation_id}:"))
}

/// Upstream `sessionName` (`values.ts:194`): the empty-key session name
/// address.
pub fn session_name() -> ValueAddress {
    value("pi.session.name", "")
}

/// Upstream `entryLabel(entryId)` (`values.ts:195`).
pub fn entry_label(entry_id: &str) -> ValueAddress {
    value("pi.entry.label", entry_id)
}

/// The `LaneConfiguration` a lane-config value deserializes from (helper for
/// typed consumers; storage keeps the payload as JSON).
pub fn lane_configuration_value(configuration: &LaneConfiguration) -> serde_json::Value {
    serde_json::to_value(configuration).expect("LaneConfiguration serializes")
}

/// The `ThinkingLevel` re-export used by callers composing lane
/// configuration values.
pub type LaneThinkingLevel = ThinkingLevel;

#[cfg(test)]
mod tests;
