//! Port of upstream `codemode/src/types.ts` — the sandbox's data model.
//!
//! The tool `execute` seam is a Rust closure returning a boxed future
//! (upstream: `execute(args, context): Promise<unknown> | unknown`); `Err`
//! carries the error message and surfaces in the script as an `Error` with
//! that message (upstream rethrows the thrown error's message). Values cross
//! the seam as JSON (`serde_json::Value`); `None` is JavaScript `undefined`.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::coding_agent::extensions::types::AbortSignal;

/// Upstream `CodemodeToolContext`: aborted when the script finishes
/// (including unawaited calls), the execution times out, the caller aborts,
/// or the sandbox is closed.
#[derive(Clone)]
pub struct CodemodeToolContext {
    pub signal: Arc<AbortSignal>,
}

pub type CodemodeToolExecuteFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<Value>, String>> + Send>>;
pub type CodemodeToolExecuteFn =
    Arc<dyn Fn(Option<Value>, CodemodeToolContext) -> CodemodeToolExecuteFuture + Send + Sync>;

/// A JSON Schema document. Only used to render declarations; values are not
/// validated against it (upstream `CodemodeJsonSchema`).
pub type CodemodeJsonSchema = Value;

/// Upstream `CodemodeTool`. `spread` and `signature` are globals-only.
#[derive(Clone)]
pub struct CodemodeTool {
    /// The script calls tools as `tools.<id>(args)` / `tools["<name>"](args)`;
    /// globals as `<name>(args)` or `<namespace>.<member>(args)`.
    pub name: String,
    /// Doc comment in declarations; listed in `ALL_TOOLS` for tools.
    pub description: Option<String>,
    /// Schema of the single argument; `None` renders `unknown`.
    pub input_schema: Option<CodemodeJsonSchema>,
    /// Schema of the resolved value; `None` renders `unknown`.
    pub output_schema: Option<CodemodeJsonSchema>,
    /// Globals only: `execute` receives all call arguments as an array.
    pub spread: bool,
    /// Globals only: explicit TS parameter list / return type.
    pub signature: Option<String>,
    /// `args` is whatever the script passed, after a JSON round trip.
    pub execute: CodemodeToolExecuteFn,
}

impl std::fmt::Debug for CodemodeTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodemodeTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("spread", &self.spread)
            .field("signature", &self.signature)
            .finish_non_exhaustive()
    }
}

/// One item of the script's output, in production order (upstream
/// `CodemodeOutputItem`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodemodeOutputItem {
    Text { text: String },
    Image { data: String, mime_type: String },
}

impl CodemodeOutputItem {
    /// The upstream wire shape uses `"mimeType"`; the port serializes with
    /// the upstream field names (the protocol messages travel verbatim).
    pub fn to_protocol_value(&self) -> Value {
        match self {
            CodemodeOutputItem::Text { text } => {
                serde_json::json!({ "type": "text", "text": text })
            }
            CodemodeOutputItem::Image { data, mime_type } => serde_json::json!({
                "type": "image",
                "data": data,
                "mimeType": mime_type
            }),
        }
    }
}

/// Upstream `CodemodeCallStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodemodeCallStatus {
    Ok,
    Error,
    Cancelled,
}

impl std::fmt::Display for CodemodeCallStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            CodemodeCallStatus::Ok => "ok",
            CodemodeCallStatus::Error => "error",
            CodemodeCallStatus::Cancelled => "cancelled",
        };
        f.write_str(text)
    }
}

/// Upstream `CodemodeCall`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodemodeCall {
    pub name: String,
    pub status: CodemodeCallStatus,
    pub duration_ms: f64,
}

/// Upstream `CodemodeErrorKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodemodeErrorKind {
    Script,
    Timeout,
    Aborted,
    Sandbox,
}

/// Upstream `CodemodeError`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodemodeError {
    pub kind: CodemodeErrorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

/// Upstream `CodemodeStoreWrites`: keys the script changed with `store()`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CodemodeStoreWrites {
    #[serde(default)]
    pub set: BTreeMap<String, Value>,
    /// Keys stored as `undefined`.
    #[serde(default)]
    pub delete: Vec<String>,
}

/// Upstream `CodemodeResult`. `output` is kept for failed executions too, up
/// to the failure; `exit()` completes with `value: None`.
#[derive(Debug, Clone, PartialEq)]
pub enum CodemodeResult {
    Ok {
        value: Option<Value>,
        output: Vec<CodemodeOutputItem>,
        calls: Vec<CodemodeCall>,
        store_writes: CodemodeStoreWrites,
    },
    Err {
        error: CodemodeError,
        output: Vec<CodemodeOutputItem>,
        calls: Vec<CodemodeCall>,
    },
}

impl CodemodeResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, CodemodeResult::Ok { .. })
    }

    pub fn output(&self) -> &[CodemodeOutputItem] {
        match self {
            CodemodeResult::Ok { output, .. } => output,
            CodemodeResult::Err { output, .. } => output,
        }
    }

    pub fn calls(&self) -> &[CodemodeCall] {
        match self {
            CodemodeResult::Ok { calls, .. } => calls,
            CodemodeResult::Err { calls, .. } => calls,
        }
    }
}

/// Upstream `CodemodeSandboxOptions`. `timeout_ms: None` is upstream
/// `Infinity` (no deadline); `memory_limit_bytes: None` is upstream
/// `undefined` (no limit beyond wasm32's 4 GiB).
#[derive(Default)]
pub struct CodemodeSandboxOptions {
    pub tools: Vec<CodemodeTool>,
    pub globals: Vec<CodemodeTool>,
    pub timeout_ms: Option<u64>,
    pub memory_limit_bytes: Option<usize>,
}

/// Upstream `CodemodeExecuteOptions`.
#[derive(Default)]
pub struct CodemodeExecuteOptions {
    pub signal: Option<Arc<AbortSignal>>,
    /// Overrides the sandbox default for this execution; `None` is `Infinity`.
    pub timeout_ms: Option<u64>,
    /// Values the script reads with `load(key)`.
    pub store: BTreeMap<String, Value>,
}

// `serde` is used through the derive paths above.
use serde::{Deserialize, Serialize};
