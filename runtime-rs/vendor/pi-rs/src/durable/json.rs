//! Port of `src/harness/json.ts`: leaf-by-leaf JSON assignment used by the
//! streaming output assembly.

use serde_json::Value;

/// One assignment slot: an object member name or an array index (upstream
/// `key: string | number` over the `Record | JsonValue[]` union).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot<'a> {
    Key(&'a str),
    Index(usize),
}

impl<'a> From<&'a str> for Slot<'a> {
    fn from(key: &'a str) -> Self {
        Slot::Key(key)
    }
}

impl From<usize> for Slot<'static> {
    fn from(index: usize) -> Self {
        Slot::Index(index)
    }
}

/// Write into one slot of `target`, which must be an object or an array (the
/// upstream `JsonContainer` cast).
fn slot_assign(target: &mut Value, key: Slot<'_>, value: Value) {
    match (target, key) {
        (Value::Object(map), Slot::Key(name)) => {
            map.insert(name.to_string(), value);
        }
        (Value::Array(items), Slot::Index(index)) => {
            if index >= items.len() {
                items.push(value);
            } else {
                items[index] = value;
            }
        }
        _ => panic!("assignJson target slot does not match the container shape"),
    }
}

fn slot_get<'a>(target: &'a Value, key: Slot<'_>) -> Option<&'a Value> {
    match (target, key) {
        (Value::Object(map), Slot::Key(name)) => map.get(name),
        (Value::Array(items), Slot::Index(index)) => items.get(index),
        _ => None,
    }
}

fn slot_take(target: &mut Value, key: Slot<'_>) -> Option<Value> {
    match (target, key) {
        (Value::Object(map), Slot::Key(name)) => map.remove(name),
        (Value::Array(items), Slot::Index(index)) => {
            if index < items.len() {
                Some(items.remove(index))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Assign `value` at `target[key]` leaf by leaf (`harness/json.ts`
/// `assignJson`). Chord records a container assignment as one full set and
/// only emits an append when a string leaf is reassigned with a longer string,
/// so writing the partial whole would store and publish the complete message
/// on every flush.
///
/// Divergence (structural, disclosed): the upstream recursion mutates nested
/// containers in place through shared references; the port threads the
/// mutated child back through [`slot_assign`], which is observationally
/// identical because no other handle sees intermediate states.
pub fn assign_json(target: &mut Value, key: Slot<'_>, value: Value) {
    let current = slot_get(target, key.clone()).cloned();
    match (&current, &value) {
        // Both records: recurse member by member, dropping stale members.
        (Some(current), _) if current.is_object() && value.is_object() => {
            let mut child = current.clone();
            let incoming = value.as_object().unwrap();
            let stale: Vec<String> = child
                .as_object()
                .unwrap()
                .keys()
                .filter(|name| !incoming.contains_key(*name))
                .cloned()
                .collect();
            for name in stale {
                child.as_object_mut().unwrap().shift_remove(&name);
            }
            for (name, item) in incoming {
                assign_json(&mut child, Slot::Key(name.as_str()), item.clone());
            }
            slot_assign(target, key, child);
        }
        // Both arrays with the current no longer than the value: element by
        // element, appending the tail.
        (Some(current), _) if current.is_array() && value.is_array() => {
            let incoming = value.as_array().unwrap();
            if current.as_array().unwrap().len() <= incoming.len() {
                let mut child = current.clone();
                for (index, item) in incoming.iter().enumerate() {
                    assign_json(&mut child, Slot::Index(index), item.clone());
                }
                slot_assign(target, key, child);
                return;
            }
            if current != &value {
                slot_assign(target, key, value);
            }
        }
        // Leaves (or shape changes): write only when different, so the delta
        // log stays minimal. An absent slot (`undefined` upstream) differs
        // from every value, including `null`.
        (current, _) => {
            if current.as_ref() != Some(&value) {
                slot_assign(target, key, value);
            }
        }
    }
}

/// Remove one slot entirely (upstream `delete slots[key]`). Returns the
/// removed value.
pub fn remove_slot(target: &mut Value, key: Slot<'_>) -> Option<Value> {
    slot_take(target, key)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assigns_leaf_by_leaf_into_objects() {
        let mut target = json!({"message": {"role": "assistant", "content": ""}});
        let message = target.get_mut("message").unwrap();
        assign_json(message, Slot::Key("content"), json!("Hel"));
        assign_json(message, Slot::Key("content"), json!("Hello"));
        assert_eq!(
            target,
            json!({"message": {"role": "assistant", "content": "Hello"}})
        );
    }

    #[test]
    fn removes_stale_object_members() {
        let mut target = json!({});
        assign_json(&mut target, Slot::Key("a"), json!({"x": 1, "y": 2}));
        assign_json(&mut target, Slot::Key("a"), json!({"x": 3}));
        assert_eq!(target, json!({"a": {"x": 3}}));
    }

    #[test]
    fn grows_arrays_in_place() {
        let mut target = json!({"items": [1]});
        assign_json(&mut target, Slot::Key("items"), json!([1, 2, 3]));
        assert_eq!(target, json!({"items": [1, 2, 3]}));
    }

    #[test]
    fn replaces_shrunk_arrays_wholesale() {
        let mut target = json!({"items": [1, 2, 3]});
        assign_json(&mut target, Slot::Key("items"), json!([1]));
        assert_eq!(target, json!({"items": [1]}));
    }

    #[test]
    fn scalar_reassignment_writes_only_changes() {
        let mut target = json!({"v": 1});
        assign_json(&mut target, Slot::Key("v"), json!(1));
        assert_eq!(target, json!({"v": 1}));
        assign_json(&mut target, Slot::Key("v"), json!(2));
        assert_eq!(target, json!({"v": 2}));
    }

    #[test]
    fn array_elements_recurse() {
        let mut target = json!({"items": [{"text": ""}]});
        assign_json(
            &mut target,
            Slot::Key("items"),
            json!([{"text": "a"}, {"text": "b"}]),
        );
        assert_eq!(target, json!({"items": [{"text": "a"}, {"text": "b"}]}));
    }
}
