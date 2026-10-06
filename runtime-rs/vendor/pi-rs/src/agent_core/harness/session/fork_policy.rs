//! Port of `packages/agent/src/harness/session/fork-policy.ts` (67 lines):
//! the branch-fork ancestry selection and the reserved-namespace projection
//! that decides which current-state rows a fork copies.

use super::commit::CommittedValueWrite;
use super::types::{ForkOptions, ForkPosition};

/// Upstream `ForkCurrentStatePlan` (`fork-policy.ts:4-6`).
#[derive(Debug, Clone, PartialEq)]
pub enum ForkCurrentStatePlan {
    Branch {
        branch: String,
        destination_tip: Option<String>,
    },
    Tree,
}

/// Upstream `selectBranchFork` source callbacks (`fork-policy.ts:9-14`).
pub struct BranchForkSource<'a, F>
where
    F: FnMut(&str) -> Option<Option<String>>,
{
    /// The source branch tip: `None` is the upstream `undefined` (unknown
    /// branch), `Some(None)` the null root tip.
    pub tip: Option<Option<String>>,
    /// `getParent(entryId)`: `None` is the upstream `undefined` (missing
    /// entry).
    pub get_parent: F,
    /// `selectEntry(entryId)`.
    pub select_entry: &'a mut dyn FnMut(&str),
}

/// Upstream `selectBranchFork` (`fork-policy.ts:8-37`): walk the tip
/// ancestry, select the copied entries, and place the destination tip
/// according to `position` (`before` stops at the parent). Returns the
/// `(branch, destinationTip)` plan half.
pub fn select_branch_fork<F>(
    options: &ForkOptions,
    mut source: BranchForkSource<'_, F>,
) -> anyhow::Result<(String, Option<String>)>
where
    F: FnMut(&str) -> Option<Option<String>>,
{
    let ForkOptions::Branch {
        branch,
        entry_id,
        position,
        ..
    } = options
    else {
        anyhow::bail!("selectBranchFork requires a branch-scoped fork");
    };
    let Some(tip) = source.tip else {
        anyhow::bail!("Unknown source branch: {branch}");
    };
    // `const requested = options.entryId ?? source.tip` — a string id, or the
    // null tip when neither is set.
    let requested: Option<String> = entry_id.clone().or_else(|| tip.clone());
    let mut found = requested.is_none();
    let mut destination_tip: Option<String> = None;
    let mut current = tip;
    while let Some(entry_id_at) = current {
        let Some(parent_id) = (source.get_parent)(&entry_id_at) else {
            anyhow::bail!("Corrupt source branch: missing parent {entry_id_at}");
        };
        if Some(&entry_id_at) == requested.as_ref() {
            found = true;
            destination_tip = if *position == Some(ForkPosition::Before) {
                parent_id.clone()
            } else {
                Some(entry_id_at.clone())
            };
            if *position != Some(ForkPosition::Before) {
                (source.select_entry)(&entry_id_at);
            }
        } else if found {
            (source.select_entry)(&entry_id_at);
        }
        current = parent_id;
    }
    if !found {
        anyhow::bail!(
            "Fork entry {} is not on source branch {branch:?}",
            requested.as_deref().unwrap_or("null")
        );
    }
    Ok((branch.clone(), destination_tip))
}

/// Upstream's inline idle lane-state literal (`fork-policy.ts:57`).
pub fn idle_lane_state_value() -> serde_json::Value {
    serde_json::json!({
        "currentOperationId": null,
        "lastOperationId": null,
        "inbox": [],
    })
}

/// Upstream `projectForkCurrentStateWrite` (`fork-policy.ts:40-67`) for one
/// current scalar row or surviving list element: decide whether the row is
/// copied and what its destination payload is. `Err` is the upstream thrown
/// `Error` for unknown reserved namespaces.
pub fn project_fork_current_state_value(
    namespace: &str,
    key: &str,
    value: serde_json::Value,
    plan: &ForkCurrentStatePlan,
    is_entry_copied: &dyn Fn(&str) -> bool,
) -> anyhow::Result<Option<(String, String, serde_json::Value)>> {
    let keep =
        |key: &str, value: serde_json::Value| Some((namespace.to_string(), key.to_string(), value));
    match namespace {
        "pi.session.name" => Ok(keep(key, value)),
        "pi.entry.label" => Ok(if is_entry_copied(key) {
            keep(key, value)
        } else {
            None
        }),
        "pi.branch.tip" => match plan {
            ForkCurrentStatePlan::Tree => Ok(keep(key, value)),
            ForkCurrentStatePlan::Branch {
                branch,
                destination_tip,
            } => Ok(if key == branch {
                keep(
                    key,
                    destination_tip
                        .clone()
                        .map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null),
                )
            } else {
                None
            }),
        },
        "pi.lane.config" => match plan {
            ForkCurrentStatePlan::Tree => Ok(keep(key, value)),
            ForkCurrentStatePlan::Branch { branch, .. } => Ok(if key == branch {
                keep(key, value)
            } else {
                None
            }),
        },
        "pi.lane.state" => match plan {
            ForkCurrentStatePlan::Tree => Ok(keep(key, idle_lane_state_value())),
            ForkCurrentStatePlan::Branch { branch, .. } => Ok(if key == branch {
                keep(key, idle_lane_state_value())
            } else {
                None
            }),
        },
        "pi.result" => Ok(None),
        _ => {
            if namespace.starts_with("pi.op.") || namespace.starts_with("pi.pending.") {
                return Ok(None);
            }
            if namespace == "pi" || namespace.starts_with("pi.") {
                anyhow::bail!("Unknown reserved fork namespace: {namespace}");
            }
            Ok(match plan {
                ForkCurrentStatePlan::Tree => keep(key, value),
                ForkCurrentStatePlan::Branch { .. } => None,
            })
        }
    }
}

/// Upstream single-value-row projection signature preserved for the storage
/// state fork ([`super::storage_state`]).
pub fn project_value_set(
    seq: i64,
    namespace: &str,
    key: &str,
    value: serde_json::Value,
    plan: &ForkCurrentStatePlan,
    is_entry_copied: &dyn Fn(&str) -> bool,
) -> anyhow::Result<Option<CommittedValueWrite>> {
    Ok(
        project_fork_current_state_value(namespace, key, value, plan, is_entry_copied)?.map(
            |(namespace, key, value)| CommittedValueWrite::Set {
                seq,
                namespace,
                key,
                value,
            },
        ),
    )
}

#[cfg(test)]
mod tests;
