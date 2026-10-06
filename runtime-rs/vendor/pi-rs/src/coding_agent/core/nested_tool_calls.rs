//! Port of upstream `coding-agent/src/core/nested-tool-calls.ts` — the
//! recorder half. Tool calls that a tool makes while it runs
//! (`ctx.executeTool()`), for example from codemode scripts: the agent loop
//! does not know about them, the session runs each one through the agent's
//! tool pipeline, and the recorder leaves the calls and their summed usage on
//! the model-issued call's tool result message.
//!
//! The runner half (`NestedToolCallRunner`, the exclusive queue and the
//! `NestedToolCallHost` pipeline hookup) lands with the agent-session delta
//! slice — it needs `runToolCall` and the `tool_execution_*` event emitter.
//!
//! Clocks: upstream stamps durations with `performance.now()`; the `_at_ms`
//! variants take the instant so tests (and hosts with injected clocks) pin
//! durations deterministically.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai::types::primitives::Usage;
use crate::coding_agent::core::usage_totals::combine_usage;

/// Limits of the nested-call record on a tool result: arguments over the
/// per-call or total size are omitted, calls beyond the count are dropped,
/// and the record is marked incomplete when any of that happens.
#[derive(Debug, Clone, Copy)]
pub struct NestedCallLimits {
    pub max_calls: usize,
    pub max_argument_bytes_per_call: usize,
    pub max_argument_bytes_total: usize,
    pub max_error_chars: usize,
}

/// Upstream `NESTED_CALL_LIMITS`.
pub const NESTED_CALL_LIMITS: NestedCallLimits = NestedCallLimits {
    max_calls: 256,
    max_argument_bytes_per_call: 8 * 1024,
    max_argument_bytes_total: 32 * 1024,
    max_error_chars: 500,
};

/// Upstream `NestedToolCallRecord` (pi-ai types.ts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedToolCallRecord {
    pub id: String,
    pub name: String,
    pub status: NestedToolCallStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Upstream record `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NestedToolCallStatus {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "unfinished")]
    Unfinished,
}

/// Upstream `NestedToolCalls` (the snapshot shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedToolCalls {
    pub calls: Vec<NestedToolCallRecord>,
    pub complete: bool,
}

/// Upstream `NestedCallSummary`: what the nested calls of one model-issued
/// tool call leave on its tool result message.
#[derive(Debug, Clone, Default)]
pub struct NestedCallSummary {
    /// Becomes `nestedCalls`. `None` when no nested call was made.
    pub calls: Option<NestedToolCalls>,
    /// Summed `usage` of the nested results, added to the message's `usage`.
    pub usage: Option<Usage>,
}

/// Upstream `NestedCallRecorder`: collects the nested calls of one
/// model-issued tool call, including calls made by nested tools.
#[derive(Debug)]
pub struct NestedCallRecorder {
    calls: Vec<NestedToolCallRecord>,
    /// Start instants by record id (upstream keys the map by record object).
    started_at: std::collections::HashMap<String, u64>,
    complete: bool,
    argument_bytes: u64,
    /// Summed usage of every nested result, including calls dropped from the
    /// record.
    usage: Option<Usage>,
}

impl Default for NestedCallRecorder {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            started_at: std::collections::HashMap::new(),
            // Upstream field initializer: `private complete = true`.
            complete: true,
            argument_bytes: 0,
            usage: None,
        }
    }
}

/// The tool-call fields the recorder reads (upstream `AgentToolCall`).
#[derive(Debug, Clone)]
pub struct NestedToolCallInput<'a> {
    pub id: &'a str,
    pub name: &'a str,
    /// Serialized with `JSON.stringify(arguments ?? {})` for the byte cap.
    pub arguments: Option<&'a Value>,
}

impl NestedCallRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a call as it starts. Returns `None` when the call is dropped
    /// (over the count limit).
    pub fn start(&mut self, tool_call: NestedToolCallInput, now_ms: u64) -> Option<String> {
        if self.calls.len() >= NESTED_CALL_LIMITS.max_calls {
            self.complete = false;
            return None;
        }
        let mut record = NestedToolCallRecord {
            id: tool_call.id.to_string(),
            name: tool_call.name.to_string(),
            status: NestedToolCallStatus::Unfinished,
            arguments: None,
            arguments_bytes: None,
            duration_ms: None,
            error: None,
        };
        let json = serde_json::to_string(
            tool_call
                .arguments
                .unwrap_or(&Value::Object(serde_json::Map::new())),
        )
        .unwrap_or_default();
        let bytes = json.len(); // UTF-8 byte length (TextEncoder)
        if bytes > NESTED_CALL_LIMITS.max_argument_bytes_per_call
            || self.argument_bytes as usize + bytes > NESTED_CALL_LIMITS.max_argument_bytes_total
        {
            record.arguments_bytes = Some(bytes as u64);
            self.complete = false;
        } else {
            record.arguments = Some(
                serde_json::from_str(&json)
                    .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
            );
            self.argument_bytes += bytes as u64;
        }
        self.started_at.insert(record.id.clone(), now_ms);
        let id = record.id.clone();
        self.calls.push(record);
        Some(id)
    }

    /// Finish a record (`None` or an unknown id is a no-op, like upstream's
    /// `if (!record) return`).
    pub fn finish(
        &mut self,
        record_id: Option<&str>,
        is_error: bool,
        error_text: &str,
        now_ms: u64,
    ) {
        let Some(record_id) = record_id else {
            return;
        };
        let Some(record) = self.calls.iter_mut().find(|record| record.id == record_id) else {
            return;
        };
        record.status = if is_error {
            NestedToolCallStatus::Error
        } else {
            NestedToolCallStatus::Ok
        };
        let started = self.started_at.remove(record_id).unwrap_or(now_ms);
        record.duration_ms = Some(now_ms.saturating_sub(started));
        if is_error && !error_text.is_empty() {
            record.error = Some(
                error_text
                    .chars()
                    .take(NESTED_CALL_LIMITS.max_error_chars)
                    .collect(),
            );
        }
    }

    /// Sum one nested result's usage into the running total.
    pub fn add_usage(&mut self, usage: Usage) {
        self.usage = Some(match self.usage.take() {
            Some(running) => combine_usage(running, usage),
            None => usage,
        });
    }

    pub fn total_usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    /// Copy of the record so far, or `None` when no nested call was made.
    pub fn snapshot(&self) -> Option<NestedToolCalls> {
        if self.calls.is_empty() && self.complete {
            return None;
        }
        Some(NestedToolCalls {
            calls: self.calls.clone(),
            complete: self.complete
                && self
                    .calls
                    .iter()
                    .all(|call| call.status != NestedToolCallStatus::Unfinished),
        })
    }

    /// The summary handed to the tool-result message builder.
    pub fn summary(&self) -> NestedCallSummary {
        NestedCallSummary {
            calls: self.snapshot(),
            usage: self.usage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call<'a>(
        id: &'a str,
        name: &'a str,
        arguments: Option<&'a Value>,
    ) -> NestedToolCallInput<'a> {
        NestedToolCallInput {
            id,
            name,
            arguments,
        }
    }

    #[test]
    fn records_arguments_and_durations() {
        let mut recorder = NestedCallRecorder::new();
        let args = json!({"path": "src/main.rs"});
        let id = recorder
            .start(call("tc-1", "read", Some(&args)), 1_000)
            .expect("recorded");
        recorder.finish(Some(&id), false, "", 1_250);
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.calls.len(), 1);
        assert_eq!(snapshot.calls[0].status, NestedToolCallStatus::Ok);
        assert_eq!(snapshot.calls[0].arguments.as_ref(), Some(&args));
        assert_eq!(snapshot.calls[0].duration_ms, Some(250));
        assert!(snapshot.complete);
        assert!(snapshot.calls[0].error.is_none());
        assert!(snapshot.calls[0].arguments_bytes.is_none());
        // No usage: summary carries neither calls-usage nor a nestedCalls-less
        // record.
        let summary = recorder.summary();
        assert!(summary.usage.is_none());
        assert!(summary.calls.is_some());
    }

    #[test]
    fn missing_arguments_serialize_as_empty_object() {
        let mut recorder = NestedCallRecorder::new();
        let id = recorder.start(call("tc", "list", None), 0).unwrap();
        recorder.finish(Some(&id), false, "", 1);
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.calls[0].arguments.as_ref(), Some(&json!({})));
    }

    #[test]
    fn oversized_arguments_are_omitted_and_mark_incomplete() {
        let mut recorder = NestedCallRecorder::new();
        let big = json!({"text": "x".repeat(NESTED_CALL_LIMITS.max_argument_bytes_per_call + 1)});
        let id = recorder
            .start(call("tc-big", "write", Some(&big)), 0)
            .unwrap();
        recorder.finish(Some(&id), false, "", 1);
        let snapshot = recorder.snapshot().unwrap();
        assert!(!snapshot.complete);
        let record = &snapshot.calls[0];
        assert!(record.arguments.is_none());
        // JSON.stringify({"text":"…"}) adds the 9-byte prefix and 2-byte suffix.
        assert_eq!(
            record.arguments_bytes,
            Some(NESTED_CALL_LIMITS.max_argument_bytes_per_call as u64 + 12)
        );

        // Total-cap path: several calls under the per-call cap accumulate;
        // once the total is over, the next small call drops too.
        let mut recorder = NestedCallRecorder::new();
        let chunk = json!({"text": "x".repeat(8180)});
        // {"text":"…"} = 9 + 8180 + 2 = 8191 bytes, under the per-call cap.
        for index in 0..4 {
            let boxed_id: &'static str = Box::leak(format!("t{index}").into_boxed_str());
            let id = recorder
                .start(call(boxed_id, "w", Some(&chunk)), index)
                .unwrap();
            recorder.finish(Some(&id), false, "", index + 1);
        }
        let second = recorder
            .start(call("tail", "w", Some(&json!({"a": 1}))), 5)
            .unwrap();
        recorder.finish(Some(&second), false, "", 6);
        let snapshot = recorder.snapshot().unwrap();
        assert!(!snapshot.complete);
        // 4 × 8191 = 32764; +7 for the 8-byte {"a":1} crosses 32768.
        assert!(snapshot
            .calls
            .iter()
            .find(|c| c.id == "tail")
            .unwrap()
            .arguments
            .is_none());
    }

    #[test]
    fn errors_truncate_to_the_char_cap() {
        let mut recorder = NestedCallRecorder::new();
        let id = recorder.start(call("tc", "bash", None), 0).unwrap();
        let message = "e".repeat(NESTED_CALL_LIMITS.max_error_chars + 100);
        recorder.finish(Some(&id), true, &message, 5);
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.calls[0].status, NestedToolCallStatus::Error);
        assert_eq!(
            snapshot.calls[0].error.as_deref().map(str::len),
            Some(NESTED_CALL_LIMITS.max_error_chars)
        );
    }

    #[test]
    fn usage_sums_and_dropped_calls_still_count() {
        let mut recorder = NestedCallRecorder::new();
        let base = Usage {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 15,
            cost: Default::default(),
        };
        recorder.add_usage(base);
        recorder.add_usage(base);
        let summary = recorder.summary();
        assert_eq!(summary.usage.as_ref().map(|u| u.input), Some(20));
        assert_eq!(summary.usage.as_ref().map(|u| u.total_tokens), Some(30));
    }

    #[test]
    fn empty_recorder_snapshots_to_none_and_call_cap_marks_incomplete() {
        let mut recorder = NestedCallRecorder::new();
        assert!(recorder.snapshot().is_none());
        for index in 0..NESTED_CALL_LIMITS.max_calls + 1 {
            let id = recorder.start(
                call(Box::leak(format!("tc-{index}").into_boxed_str()), "t", None),
                index as u64,
            );
            assert_eq!(id.is_some(), index < NESTED_CALL_LIMITS.max_calls);
        }
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.calls.len(), NESTED_CALL_LIMITS.max_calls);
        assert!(!snapshot.complete);
        // A finish for a dropped/unknown id is a no-op.
        recorder.finish(Some("nope"), true, "x", 99);
    }
}
