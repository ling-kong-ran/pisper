//! Port of `src/harness/usage.ts`: the per-conversation spend ledger
//! (`pi.usage`) and the counter addition shared by generation and tools.
//!
//! Divergences (structural, disclosed): upstream mutates a typed
//! `Draft<UsageState>` whose buckets are JS objects; the port mutates the
//! same JSON through `serde_json`'s insertion-ordered maps. JS number
//! formatting is reproduced by writing integral sums as integers
//! ([`js_number_value`]), matching `JSON.stringify` for the counter sums.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::ai::types::Usage;

use super::super::documents::{define_doc, DefinitionScope, DocToken};
use super::super::errors::PlainError;
use super::super::session::transaction::Transaction;
use super::super::types::{DocumentFork, DocumentHistory, JsonObject};

/// `UsageState` bucket names (`config.ts` through `usage.ts`).
pub const MODEL_BUCKET: &str = "models";
pub const TOOL_BUCKET: &str = "tools";

/// Spend recorded by one conversation's own entries (`usage.ts`
/// `UsageState`); the entries stay authoritative. Wire shape
/// `{models: {}, tools: {}}`.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageState {
    /// Assistant entries, keyed `provider/modelId`.
    pub models: JsonObject,
    /// Tool results, keyed by tool name; their usage has no model identity.
    pub tools: JsonObject,
}

/// The built-in `pi.usage` document (`usage.ts` `UsageDoc`).
pub fn usage_doc() -> DocToken {
    define_doc(super::super::documents::DocDefinition {
        kind: String::from("pi.usage"),
        version: 1,
        scope: DefinitionScope::Conversation,
        history: Some(DocumentHistory::Latest),
        fork: Some(DocumentFork::Initial),
        family: false,
        initial: Arc::new(|_| initial_json()),
        migrate: None,
        checkpoint_when: Some(Arc::new(|_, _, _| true)),
    })
    .expect("the built-in usage document definition is valid")
}

/// `initial()` (`usage.ts:20`): `{ models: {}, tools: {} }`.
pub fn initial_json() -> JsonObject {
    let mut map = Map::new();
    map.insert(String::from("models"), Value::Object(Map::new()));
    map.insert(String::from("tools"), Value::Object(Map::new()));
    map
}

impl UsageState {
    /// The initial state.
    pub fn initial() -> Self {
        UsageState {
            models: Map::new(),
            tools: Map::new(),
        }
    }

    /// One bucket of the state by name (`"models"` / `"tools"`).
    pub fn bucket(&self, bucket: &str) -> Option<&JsonObject> {
        match bucket {
            MODEL_BUCKET => Some(&self.models),
            TOOL_BUCKET => Some(&self.tools),
            _ => None,
        }
    }

    /// Parse from a stored document value.
    pub fn from_json(value: &JsonObject) -> Self {
        UsageState {
            models: value
                .get(MODEL_BUCKET)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            tools: value
                .get(TOOL_BUCKET)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        }
    }
}

/// A JS number as a JSON value: integral sums stay integers, like
/// `JSON.stringify`.
pub fn js_number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_991.0 {
        Value::from(value as i64)
    } else {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// Add every counter of `usage` to the `total` object (`usage.ts`
/// `addUsage`); optional counters are added once either side reports them.
pub fn add_usage(total: &mut Map<String, Value>, usage: &Usage) {
    // Token counters are integers; cost sums are JS numbers.
    let token_sum = |current: Option<&Value>, add: u64| {
        let base = current.and_then(Value::as_u64).unwrap_or(0);
        Value::from(base.saturating_add(add))
    };
    let cost_sum = |current: Option<&Value>, add: f64| {
        js_number_value(current.and_then(Value::as_f64).unwrap_or(0.0) + add)
    };
    total.insert(
        String::from("input"),
        token_sum(total.get("input"), usage.input),
    );
    total.insert(
        String::from("output"),
        token_sum(total.get("output"), usage.output),
    );
    total.insert(
        String::from("cacheRead"),
        token_sum(total.get("cacheRead"), usage.cache_read),
    );
    total.insert(
        String::from("cacheWrite"),
        token_sum(total.get("cacheWrite"), usage.cache_write),
    );
    total.insert(
        String::from("totalTokens"),
        token_sum(total.get("totalTokens"), usage.total_tokens),
    );
    if let Some(cache_write_1h) = usage.cache_write_1h {
        total.insert(
            String::from("cacheWrite1h"),
            token_sum(total.get("cacheWrite1h"), cache_write_1h),
        );
    }
    if let Some(reasoning) = usage.reasoning {
        total.insert(
            String::from("reasoning"),
            token_sum(total.get("reasoning"), reasoning),
        );
    }
    let cost = total
        .entry(String::from("cost"))
        .or_insert_with(|| Value::Object(Map::new()));
    if !cost.is_object() {
        *cost = Value::Object(Map::new());
    }
    let cost = cost.as_object_mut().expect("cost object");
    for (key, value) in [
        ("input", usage.cost.input),
        ("output", usage.cost.output),
        ("cacheRead", usage.cost.cache_read),
        ("cacheWrite", usage.cost.cache_write),
        ("total", usage.cost.total),
    ] {
        cost.insert(String::from(key), cost_sum(cost.get(key), value));
    }
}

/// Add `usage` to one bucket of the conversation's `pi.usage`, in the commit
/// that appends its entry (`usage.ts` `recordUsage`).
pub fn record_usage(
    tx: &Transaction,
    conversation_id: i64,
    bucket: &str,
    key: &str,
    usage: &Usage,
) -> Result<(), PlainError> {
    let doc = usage_doc();
    let draft = tx.doc(&doc.definition, Some(conversation_id), None, None)?;
    // Own keys only (`Object.hasOwn`): the map lookup is the port's hasOwn.
    let totals = draft
        .read(&[bucket.into()])
        .map_err(|error| PlainError::new(error.message().to_string()))?
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let total = totals.get(key).cloned();
    let updated = match total {
        // Providers may leave optional counters `undefined`; the first entry
        // takes strict JSON of the usage value.
        None => serde_json::to_value(usage)
            .map_err(|error| PlainError::new(error.to_string()))?
            .as_object()
            .cloned()
            .unwrap_or_default(),
        Some(mut total) => {
            if let Some(total_object) = total.as_object_mut() {
                add_usage(total_object, usage);
            }
            total.as_object().cloned().unwrap_or_default()
        }
    };
    let mut next = totals;
    next.insert(key.to_string(), Value::Object(updated));
    draft
        .set(&[bucket.into()], Value::Object(next))
        .map_err(|error| PlainError::new(error.message().to_string()))?;
    Ok(())
}

/// Add every bucket of `state` into `sum` (`usage.ts` `addUsageState`).
pub fn add_usage_state(sum: &mut UsageState, state: &UsageState) {
    for (sum_bucket, state_bucket) in [
        (&mut sum.models, &state.models),
        (&mut sum.tools, &state.tools),
    ] {
        for (key, usage) in state_bucket {
            match sum_bucket.get_mut(key) {
                Some(total) => {
                    if let Some(total_object) = total.as_object_mut() {
                        if let Ok(usage_value) = serde_json::from_value::<Usage>(usage.clone()) {
                            add_usage(total_object, &usage_value);
                        }
                    }
                }
                None => {
                    // Define rather than assign: assigning a tool named
                    // `__proto__` would set the prototype (upstream note).
                    sum_bucket.insert(key.clone(), usage.clone());
                }
            }
        }
    }
}
