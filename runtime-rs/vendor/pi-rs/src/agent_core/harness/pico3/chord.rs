//! Port of `packages/agent/src/harness/pico3/chord.ts` (178 lines) — the
//! chord glue, pure subset.
//!
//! **Task-8 scope (disclosed).** `chord.ts` defines service contracts over
//! the full chord library: `defineService` (`PicoHarnessService`,
//! `PicoConversationService`), `ReplicatedState`/`MutableReplicatedState`
//! publication, and `ConversationHandle`/`Harness` (from `harness.ts`). All
//! of those are M6 (chord services/facets) and Task 9/10 (harness) material,
//! so only the pure surface ports here:
//! - [`PublishedConversationView`] (`chord.ts:18-20`), the published shape;
//! - [`apply_ops_to_view`] — port of `applyTracked` (`chord.ts:145-169`),
//!   the envelope-op application the bridge performs before publishing,
//!   including its exact error messages;
//! - the path resolution helper (`resolve`, `chord.ts:171-178`).
//!
//! [`attachChordView`]/`bridgeWatch` (`chord.ts:75-143`), the service
//! definitions, and `createPicoConversationService` land with chord (M6) +
//! the harness conversation handles (Task 9); the oracle coverage
//! (`chord.test.ts`) needs a live `Harness`, so its bridge-convergence
//! semantics are covered in this task only as far as [`apply_ops_to_view`]
//! goes (see the tests module).

use serde_json::Value;

use crate::agent_core::chord_support::delta::{Op, Seg};

use super::types::{ConversationView, JsonObject, ViewEvent};

/// Upstream `PublishedConversationView` (`chord.ts:18-20`): the conversation
/// view plus the events of the last applied commit, under a `commit` key.
#[derive(Debug, Clone, PartialEq)]
pub struct PublishedConversationView {
    pub view: ConversationView,
    /// `commit.events` (`chord.ts:19`).
    pub commit_events: Vec<ViewEvent>,
}

impl PublishedConversationView {
    /// The JSON shape the chord bridge publishes (`commit.events` inline).
    pub fn to_value(&self) -> anyhow::Result<Value> {
        let mut value = serde_json::to_value(&self.view)?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "commit".to_owned(),
                serde_json::json!({
                    "events": self.commit_events.iter().map(ViewEvent::to_value).collect::<Vec<_>>(),
                }),
            );
        }
        Ok(value)
    }

    /// Parse the published JSON shape.
    pub fn from_value(value: &Value) -> anyhow::Result<PublishedConversationView> {
        let mut object = value
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("published view is not an object"))?;
        let events = object
            .get("commit")
            .and_then(|commit| commit.get("events"))
            .and_then(Value::as_array)
            .map(|events| {
                events
                    .iter()
                    .map(ViewEvent::from_value)
                    .collect::<anyhow::Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        object.shift_remove("commit");
        Ok(PublishedConversationView {
            view: serde_json::from_value(Value::Object(object))?,
            commit_events: events,
        })
    }
}

/// Port of `applyTracked` (`chord.ts:145-169`): apply a commit's ops to the
/// published view. `r` (root replacement) is a contract breach — a live
/// Pico envelope never replaces the view root.
pub fn apply_ops_to_view(root: &mut Value, ops: &[Op]) -> anyhow::Result<()> {
    for op in ops {
        match op {
            Op::Replace(_) => {
                anyhow::bail!("live Pico envelope unexpectedly replaced the view root");
            }
            Op::Splice {
                path,
                index,
                remove,
                items,
            } => {
                let target = resolve_mut(root, path)?;
                let array = target.as_array_mut().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Pico splice path is not an array: {}",
                        path.iter()
                            .map(Seg::to_string)
                            .collect::<Vec<_>>()
                            .join(".")
                    )
                })?;
                let start = (*index).min(array.len());
                let end = (start + *remove).min(array.len());
                array.drain(start..end);
                array.splice(start..start, items.clone());
            }
            other => {
                let path = other.path().expect("non-replace op has a path");
                let (parent_path, key) = path.split_at(path.len() - 1);
                let parent = resolve_mut(root, parent_path)?;
                if !parent.is_object() {
                    anyhow::bail!(
                        "Pico operation parent is not an object: {}",
                        path.iter()
                            .map(Seg::to_string)
                            .collect::<Vec<_>>()
                            .join(".")
                    );
                }
                let object = parent.as_object_mut().expect("checked above");
                let Seg::Key(key) = &key[0] else {
                    anyhow::bail!(
                        "Pico operation parent is not an object: {}",
                        path.iter()
                            .map(Seg::to_string)
                            .collect::<Vec<_>>()
                            .join(".")
                    );
                };
                match other {
                    Op::Delete { .. } => {
                        object.shift_remove(key);
                    }
                    Op::Set { value, .. } => {
                        object.insert(key.clone(), value.clone());
                    }
                    Op::Append { text, .. } => {
                        let current = object.get(key).and_then(Value::as_str).unwrap_or_default();
                        object.insert(key.clone(), Value::String(format!("{current}{text}")));
                    }
                    Op::Truncate { count, .. } => {
                        let current = object.get(key).and_then(Value::as_str).unwrap_or_default();
                        let sliced = crate::agent_core::chord_support::delta::slice_utf16_from(
                            current, *count,
                        );
                        object.insert(key.clone(), Value::String(sliced.to_owned()));
                    }
                    Op::Replace(_) | Op::Splice { .. } => unreachable!("handled above"),
                }
            }
        }
    }
    Ok(())
}

/// Port of `resolve` (`chord.ts:171-178`), mutable form; a path that does
/// not resolve leaves the walk at `None`.
fn resolve_mut<'a>(root: &'a mut Value, path: &[Seg]) -> anyhow::Result<&'a mut Value> {
    let mut node = root;
    for segment in path {
        let next = match segment {
            Seg::Key(key) => node
                .as_object_mut()
                .and_then(|object| object.get_mut(key.as_str())),
            Seg::Index(index) => node.as_array_mut().and_then(|array| array.get_mut(*index)),
        };
        match next {
            Some(next) => node = next,
            None => anyhow::bail!(
                "Pico operation parent is not an object: {}",
                path.iter()
                    .map(Seg::to_string)
                    .collect::<Vec<_>>()
                    .join(".")
            ),
        }
    }
    Ok(node)
}

/// The `JsonObject` re-export for the module surface.
pub type ChordJsonObject = JsonObject;

#[cfg(test)]
mod json_order_tests {
    use super::*;

    #[test]
    fn json_order_view_delete_preserves_surviving_fields() {
        let mut root = serde_json::json!({"drop":0,"z":1,"a":2,"nested":{"drop":0,"y":3,"b":4}});
        let key = |s: &str| Seg::Key(s.to_owned());
        apply_ops_to_view(
            &mut root,
            &[
                Op::Delete {
                    path: vec![key("drop")],
                },
                Op::Delete {
                    path: vec![key("nested"), key("drop")],
                },
                Op::Delete {
                    path: vec![key("missing")],
                },
            ],
        )
        .unwrap();
        assert_eq!(root.to_string(), r#"{"z":1,"a":2,"nested":{"y":3,"b":4}}"#);
    }
}
