//! Port of upstream `codemode/src/runtime/protocol.ts` — the messages between
//! the host and the per-execution VM thread. Tool arguments, results, and
//! values cross as JSON strings (never structured values), exactly like the
//! upstream worker protocol.

use serde_json::Value;

use super::super::types::CodemodeOutputItem;

/// Upstream `WorkerData`: everything a fresh VM thread needs. The upstream
/// compiled-wasm and `SharedArrayBuffer` interrupt members map to the engine
/// (no wasm artifact; an `AtomicBool` interrupt).
#[derive(Clone)]
pub struct WorkerData {
    pub code: String,
    /// `js_name` is the identifier the script uses; `description` is listed
    /// in `ALL_TOOLS`.
    pub tools: Vec<ToolEntry>,
    pub globals: Vec<GlobalEntry>,
    pub memory_limit_bytes: Option<usize>,
    /// Snapshot for `load()`: key to JSON text.
    pub store: std::collections::BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct ToolEntry {
    pub name: String,
    pub js_name: String,
    pub description: String,
}

#[derive(Clone)]
pub struct GlobalEntry {
    pub name: String,
    pub spread: bool,
}

/// Upstream `WorkerToHostMessage`.
#[derive(Debug, Clone)]
pub enum WorkerToHostMessage {
    Call {
        id: f64,
        target: CallTarget,
        name: String,
        args: Option<String>,
    },
    Output {
        item: CodemodeOutputItem,
    },
    Done {
        ok: bool,
        /// JSON-encoded result value when `ok`; the error JSON otherwise.
        value: Option<String>,
        /// JSON array of `[key, json]` writes and `[key]` deletions.
        writes: String,
    },
    /// The VM failed outside the script's control (for example a job
    /// exception).
    Crash {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallTarget {
    Tool,
    Global,
}

/// Upstream `HostToWorkerMessage`: the JSON result when `ok`, otherwise the
/// error message.
#[derive(Debug, Clone)]
pub struct HostToWorkerMessage {
    pub id: f64,
    pub ok: bool,
    pub payload: Option<String>,
}

/// Shape of a script error's JSON (`{ name?, message, stack? }`).
pub fn parse_script_error_json(error: &str) -> (Option<String>, String, Option<String>) {
    match serde_json::from_str::<Value>(error) {
        Ok(Value::Object(map)) => (
            map.get("name").and_then(Value::as_str).map(str::to_string),
            map.get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    if map.get("message").is_some() {
                        Some(String::new())
                    } else {
                        None
                    }
                })
                .unwrap_or_default(),
            map.get("stack").and_then(Value::as_str).map(str::to_string),
        ),
        _ => (None, error.to_string(), None),
    }
}
