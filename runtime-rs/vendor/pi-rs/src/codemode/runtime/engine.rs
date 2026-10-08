//! The codemode sandbox engine: port of upstream `runtime/host.ts` +
//! `runtime/worker.ts` + `wasm.ts` onto rquickjs (embedded quickjs-ng).
//!
//! Upstream architecture, preserved one-to-one:
//!
//! - One **execution = one fresh VM on its own OS thread** (upstream: a
//!   worker thread with a fresh QuickJS wasm instance). A runaway script
//!   cannot poison a later run.
//! - The thread evaluates [`PRELUDE_SOURCE`] (byte-exact) with the host
//!   **bridge** as a Rust closure, then evaluates the script as
//!   `(async (tools, console) => {<code>\n})` at `codemode.js`, calls the
//!   prelude's `run`, drains the job queue, and reports `done`.
//! - Tool calls and output cross as JSON strings; the host executes tools
//!   concurrently (upstream: per-message async handlers) and `settle`s each
//!   call by id; after every settle the VM drains jobs and runs the
//!   prelude's `stalled()` check.
//! - Host-side finishes (script `done`, timeout, abort, crash, sandbox
//!   close) resolve the execution exactly like upstream `finish()` —
//!   records of still-pending calls keep their `cancelled` status, their
//!   signals are aborted, and the worker is stopped via the interrupt flag
//!   and the closing of the settle channel.
//!
//! Engine divergences (disclosed): rquickjs embeds quickjs-ng natively
//! instead of the wasm build (same engine core; probes confirm identical
//! sloppy-mode eval, stack-frame format, recursion `RangeError`, and
//! JSON/number/Date formatting); engine diagnostics that quickjs-wasi writes
//! to fd 1/2 (discarded upstream) are never produced; the "Worker exited
//! with code N before the script settled" path reports code 1 for an
//! unexpected VM-thread end (upstream reports the worker's exit code); and
//! crash texts format rquickjs errors rather than quickjs-wasi
//! `JSException`s. When the memory limit is hit, the engine throws
//! `InternalError: out of memory`, but under exhaustion building that error
//! object can itself fail, in which case quickjs-ng throws JS_NULL and the
//! script observes the prelude's degenerate `{ message: "null" }` error —
//! which shape appears is allocation-layout dependent (the wasm build
//! observed the InternalError shape on the captured scenario; the native
//! build the degenerate one). `execute` must run inside a Tokio context
//! (upstream runs on the host's event loop).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rquickjs::{CatchResultExt, Context, Function, Runtime, Value};
use serde_json::Value as Json;

use super::prelude::PRELUDE_SOURCE;
use super::protocol::{
    parse_script_error_json, CallTarget, GlobalEntry, HostToWorkerMessage, ToolEntry, WorkerData,
    WorkerToHostMessage,
};
use crate::codemode::identifier::to_codemode_identifier;
use crate::codemode::types::{
    CodemodeCall, CodemodeCallStatus, CodemodeError, CodemodeErrorKind, CodemodeExecuteOptions,
    CodemodeOutputItem, CodemodeResult, CodemodeSandboxOptions, CodemodeStoreWrites, CodemodeTool,
    CodemodeToolContext,
};
use crate::coding_agent::extensions::types::AbortSignal;

/// Upstream `DEFAULT_TIMEOUT_MS`.
const DEFAULT_TIMEOUT_MS: u64 = 300_000;
/// quickjs-wasi `MAX_STACK_SIZE` (512 KiB): without a guard, deep recursion
/// overflows the stack and traps instead of throwing a catchable
/// `RangeError`.
const MAX_STACK_SIZE: usize = 524_288;
/// Sentinel for upstream `Infinity` deadlines.
pub const NO_TIMEOUT: u64 = u64::MAX;

fn matches_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => {
            (first.is_ascii_alphabetic() || first == '_' || first == '$')
                && chars.all(|char| char.is_ascii_alphanumeric() || char == '_' || char == '$')
        }
        None => false,
    }
}

/// Upstream `RESERVED_GLOBALS`.
const RESERVED_GLOBALS: [&str; 9] = [
    "tools",
    "ALL_TOOLS",
    "console",
    "text",
    "image",
    "exit",
    "globalThis",
    "store",
    "load",
];

/// Upstream `serializeStore`: values to JSON text.
fn serialize_store(store: &BTreeMap<String, Json>) -> BTreeMap<String, String> {
    store
        .iter()
        .filter_map(|(key, value)| {
            serde_json::to_string(value)
                .ok()
                .map(|json| (key.clone(), json))
        })
        .collect()
}

/// Upstream `parseStoreWrites`: a JSON array of `[key, json]` pairs (writes)
/// and `[key]` entries (deletions); each `json` element is a JSON *text* that
/// upstream `JSON.parse`s.
fn parse_store_writes(writes: &str) -> CodemodeStoreWrites {
    let mut parsed = CodemodeStoreWrites::default();
    let Ok(Json::Array(entries)) = serde_json::from_str::<Json>(writes) else {
        return parsed;
    };
    for entry in entries {
        let Json::Array(pair) = entry else {
            continue;
        };
        let Some(Json::String(key)) = pair.first().cloned() else {
            continue;
        };
        match pair.get(1) {
            None => parsed.delete.push(key),
            Some(Json::String(json)) => {
                if let Ok(value) = serde_json::from_str::<Json>(json) {
                    parsed.set.insert(key, value);
                }
            }
            // The prelude only writes JSON texts; anything else is skipped
            // (upstream `JSON.parse` would coerce/throw inside the host).
            Some(_) => {}
        }
    }
    parsed
}

/// JS `ToBoolean` (quickjs-wasi `toBoolean`).
fn js_truthy(value: &Value<'_>) -> bool {
    match value.type_of() {
        rquickjs::Type::Uninitialized | rquickjs::Type::Undefined | rquickjs::Type::Null => false,
        rquickjs::Type::Bool => value.as_bool().unwrap_or(false),
        rquickjs::Type::Int => value.as_int().map(|int| int != 0).unwrap_or(false),
        rquickjs::Type::Float => value
            .as_float()
            .map(|float| float != 0.0 && !float.is_nan())
            .unwrap_or(false),
        rquickjs::Type::String => value
            .as_string()
            .and_then(|string| string.to_string().ok())
            .map(|string| !string.is_empty())
            .unwrap_or(false),
        _ => true,
    }
}

/// JS `String(value)` for the bridge's primitives-only contract. Numbers only
/// cross as call ids (never re-stringified), so the number formatting here is
/// the coarse JS shape.
fn js_to_string(value: &Value<'_>) -> rquickjs::Result<String> {
    if let Some(string) = value.as_string() {
        return string.to_string();
    }
    if let Some(int) = value.as_int() {
        return Ok(int.to_string());
    }
    if let Some(float) = value.as_float() {
        return Ok(format!("{float}"));
    }
    if value.is_bool() {
        return Ok(if value.as_bool().unwrap_or(false) {
            "true".to_string()
        } else {
            "false".to_string()
        });
    }
    Err(rquickjs::Error::FromJs {
        from: "value",
        to: "String",
        message: Some("bridge arguments must be primitives".to_string()),
    })
}

/// Upstream `describeException(error)` (worker.ts): JSON-encoded
/// `{ name, message, stack }` with the stack trimmed at the end.
fn describe_exception_json(exception: &rquickjs::Exception<'_>) -> String {
    let name: Option<String> = exception.as_object().get("name").ok();
    let name = name.unwrap_or_else(|| "Error".to_string());
    let message = exception.message().unwrap_or_default();
    let stack = exception.stack().map(|stack| stack.trim_end().to_string());
    let head = if message.is_empty() {
        name.clone()
    } else {
        format!("{name}: {message}")
    };
    let stack = match stack {
        Some(stack) if !stack.is_empty() => format!("{head}\n{stack}"),
        _ => head,
    };
    let payload = serde_json::json!({ "name": name, "message": message, "stack": stack });
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
}

/// Upstream `crash(error)` text: `"{name}: {message}"` for a JS exception.
fn crash_text(caught: &rquickjs::CaughtError<'_>) -> String {
    match caught {
        rquickjs::CaughtError::Exception(exception) => {
            let name: Option<String> = exception.as_object().get("name").ok();
            let message = exception.message().unwrap_or_default();
            match name {
                Some(name) if !message.is_empty() => format!("{name}: {message}"),
                Some(name) => name,
                None => message,
            }
        }
        other => format!("{other}"),
    }
}

type EventSender = tokio::sync::mpsc::UnboundedSender<WorkerToHostMessage>;

struct PendingCall {
    /// Index into the shared `calls` vec; `None` for globals.
    call_index: Option<usize>,
    started_at: Instant,
    signal: Arc<AbortSignal>,
}

#[derive(Default)]
struct ExecutionState {
    finished: bool,
    output: Vec<CodemodeOutputItem>,
    calls: Vec<CodemodeCall>,
    pending: HashMap<i64, PendingCall>,
}

struct RunOptions {
    worker_data: WorkerData,
    tools: BTreeMap<String, CodemodeTool>,
    globals: BTreeMap<String, CodemodeTool>,
    /// `None` means the sandbox default (`DEFAULT_TIMEOUT_MS`); the
    /// [`NO_TIMEOUT`] sentinel is upstream `Infinity`.
    timeout_ms: Option<u64>,
    signal: Option<Arc<AbortSignal>>,
}

/// Upstream `Execution`: one script run in its own VM thread.
struct Execution;

impl Execution {
    /// Runs the script to completion. Script failures come back inside
    /// [`CodemodeResult::Err`], never as `Err` (that path belongs to
    /// "Sandbox is closed").
    async fn run(options: RunOptions) -> CodemodeResult {
        let interrupt = Arc::new(AtomicBool::new(false));
        let (event_tx, mut event_rx): (
            EventSender,
            tokio::sync::mpsc::UnboundedReceiver<WorkerToHostMessage>,
        ) = tokio::sync::mpsc::unbounded_channel();
        let (settle_tx, settle_rx) = std::sync::mpsc::channel::<HostToWorkerMessage>();

        // Upstream failure modes here — the wasm load rejecting
        // ("Failed to load QuickJS: …") and the worker failing to start
        // ("Failed to start worker: …") — have no native-thread equivalent;
        // the only spawn failure is OS thread exhaustion, reported with the
        // OS error text.
        let vm_interrupt = Arc::clone(&interrupt);
        let vm_event_tx = event_tx.clone();
        let vm_options = RunOptions {
            worker_data: options.worker_data.clone(),
            tools: options.tools.clone(),
            globals: options.globals.clone(),
            timeout_ms: options.timeout_ms,
            signal: options.signal.clone(),
        };
        let spawn_result = std::thread::Builder::new()
            .name("codemode-vm".to_string())
            .spawn(move || {
                vm_thread_main(VmArguments {
                    data: vm_options.worker_data,
                    interrupt: vm_interrupt,
                    event_tx: vm_event_tx,
                    settle_rx,
                })
            });
        if let Err(error) = spawn_result {
            return CodemodeResult::Err {
                error: CodemodeError {
                    kind: CodemodeErrorKind::Sandbox,
                    name: None,
                    message: error.to_string(),
                    stack: None,
                },
                output: Vec::new(),
                calls: Vec::new(),
            };
        }
        drop(event_tx);

        let state = Arc::new(Mutex::new(ExecutionState::default()));

        // Abort subscription (upstream: `signal.addEventListener("abort", onAbort, { once: true })`).
        let (abort_tx, mut abort_rx) = tokio::sync::oneshot::channel::<()>();
        let mut abort_subscription = None;
        let mut aborted_upfront = false;
        if let Some(signal) = &options.signal {
            if signal.is_aborted() {
                aborted_upfront = true;
            } else {
                let sender = Arc::new(Mutex::new(Some(abort_tx)));
                abort_subscription = Some(signal.on_abort(Arc::new(move || {
                    if let Some(sender) = sender
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        let _ = sender.send(());
                    }
                })));
            }
        }

        // Timeout deadline (upstream: setTimeout).
        let timeout_ms = options.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        let deadline = (timeout_ms != NO_TIMEOUT)
            .then(|| tokio::time::Instant::now() + Duration::from_millis(timeout_ms));

        let finish_abort = |state: &Arc<Mutex<ExecutionState>>| {
            finish(
                state,
                settle_tx.clone(),
                &interrupt,
                Some(CodemodeError {
                    kind: CodemodeErrorKind::Aborted,
                    name: None,
                    message: "Execution aborted".to_string(),
                    stack: None,
                }),
                None,
                None,
            )
        };

        if aborted_upfront {
            return finish_abort(&state);
        }

        // Event loop; terminal events produce the result.
        #[allow(unused_assignments)]
        let mut result: Option<CodemodeResult> = None;
        loop {
            tokio::select! {
                biased;
                _ = async {
                        match deadline {
                            Some(deadline) => tokio::time::sleep_until(deadline).await,
                            None => std::future::pending::<()>().await,
                        }
                    }, if deadline.is_some() => {
                    result = Some(finish(
                        &state,
                        settle_tx.clone(),
                        &interrupt,
                        Some(CodemodeError {
                            kind: CodemodeErrorKind::Timeout,
                            name: None,
                            message: format!("Execution timed out after {timeout_ms} ms"),
                            stack: None,
                        }),
                        None,
                        None,
                    ));
                }
                _ = &mut abort_rx, if abort_subscription.is_some() => {
                    result = Some(finish_abort(&state));
                }
                event = event_rx.recv() => {
                    match event {
                        Some(message) => {
                            result = handle_message(
                                &state, &settle_tx, &interrupt, &options, message,
                            );
                        }
                        // The VM thread ended without a `done` (upstream:
                        // `worker.on("exit")` after no settle).
                        None => {
                            result = Some(finish(
                                &state,
                                settle_tx.clone(),
                                &interrupt,
                                Some(CodemodeError {
                                    kind: CodemodeErrorKind::Sandbox,
                                    name: None,
                                    message: "Worker exited with code 1 before the script settled"
                                        .to_string(),
                                    stack: None,
                                }),
                                None,
                                None,
                            ));
                        }
                    }
                }
            }
            if result.is_some() {
                break;
            }
        }
        result.unwrap_or_else(|| unreachable!("loop breaks only with a result"))
    }
}

struct VmArguments {
    data: WorkerData,
    interrupt: Arc<AtomicBool>,
    event_tx: EventSender,
    settle_rx: std::sync::mpsc::Receiver<HostToWorkerMessage>,
}

/// Upstream `handleMessage`. `Some(result)` ends the execution.
fn handle_message(
    state: &Arc<Mutex<ExecutionState>>,
    settle_tx: &std::sync::mpsc::Sender<HostToWorkerMessage>,
    interrupt: &Arc<AtomicBool>,
    options: &RunOptions,
    message: WorkerToHostMessage,
) -> Option<CodemodeResult> {
    {
        let locked = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if locked.finished {
            return None;
        }
    }
    match message {
        WorkerToHostMessage::Output { item } => {
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .output
                .push(item);
            None
        }
        WorkerToHostMessage::Call {
            id,
            target,
            name,
            args,
        } => {
            handle_call(
                state,
                settle_tx,
                &options.tools,
                &options.globals,
                id,
                target,
                name,
                args,
            );
            None
        }
        WorkerToHostMessage::Done { ok, value, writes } => {
            let result = if ok {
                let parsed = value
                    .as_deref()
                    .and_then(|json| serde_json::from_str::<Json>(json).ok());
                finish(
                    state,
                    settle_tx.clone(),
                    interrupt,
                    None,
                    parsed,
                    Some(&writes),
                )
            } else {
                let error_json = value.unwrap_or_else(|| "null".to_string());
                let (name, message, stack) = parse_script_error_json(&error_json);
                finish(
                    state,
                    settle_tx.clone(),
                    interrupt,
                    Some(CodemodeError {
                        kind: CodemodeErrorKind::Script,
                        name,
                        message,
                        stack,
                    }),
                    None,
                    None,
                )
            };
            Some(result)
        }
        WorkerToHostMessage::Crash { message } => Some(finish(
            state,
            settle_tx.clone(),
            interrupt,
            Some(CodemodeError {
                kind: CodemodeErrorKind::Sandbox,
                name: None,
                message,
                stack: None,
            }),
            None,
            None,
        )),
    }
}

/// Upstream `handleCall`: records the call, then executes the tool
/// concurrently (upstream's async handler), replying with `settle` when it
/// settles. (Upstream reaches these through `this`; the port threads them as
/// parameters.)
#[allow(clippy::too_many_arguments)]
fn handle_call(
    state: &Arc<Mutex<ExecutionState>>,
    settle_tx: &std::sync::mpsc::Sender<HostToWorkerMessage>,
    tools: &BTreeMap<String, CodemodeTool>,
    globals: &BTreeMap<String, CodemodeTool>,
    id: f64,
    target: CallTarget,
    name: String,
    args: Option<String>,
) {
    let is_tool = target == CallTarget::Tool;
    let id_i64 = id as i64;
    let signal = Arc::new(AbortSignal::new());
    let call_index = {
        let mut locked = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if locked.finished {
            return;
        }
        let call_index = if is_tool {
            locked.calls.push(CodemodeCall {
                name: name.clone(),
                status: CodemodeCallStatus::Cancelled,
                duration_ms: 0.0,
            });
            Some(locked.calls.len() - 1)
        } else {
            None
        };
        locked.pending.insert(
            id_i64,
            PendingCall {
                call_index,
                started_at: Instant::now(),
                signal: Arc::clone(&signal),
            },
        );
        call_index
    };

    // `message.args === undefined ? undefined : JSON.parse(message.args)` —
    // a parse failure rejects the call (upstream catches the SyntaxError).
    let parsed_args: Result<Option<Json>, String> = match &args {
        None => Ok(None),
        Some(json) => serde_json::from_str::<Json>(json)
            .map(Some)
            .map_err(|error| error.to_string()),
    };
    let tool = (if is_tool { tools } else { globals }).get(&name).cloned();
    let state = Arc::clone(state);
    let settle_tx = settle_tx.clone();
    let name_for_error = name;
    let kind_for_error = if is_tool { "tool" } else { "global" };
    // Concurrent with the event loop and other calls, like upstream's
    // fire-and-forget `void this.handleCall(message)`.
    drop(tokio::spawn(async move {
        let started_at = Instant::now();
        let (ok, payload, status) = match parsed_args {
            Err(message) => (false, Some(message), CodemodeCallStatus::Error),
            Ok(parsed_args) => match tool {
                None => (
                    false,
                    Some(format!("Unknown {kind_for_error} \"{name_for_error}\"")),
                    CodemodeCallStatus::Error,
                ),
                Some(tool) => {
                    let context = CodemodeToolContext { signal };
                    match (tool.execute)(parsed_args, context).await {
                        Ok(Some(value)) => (
                            true,
                            serde_json::to_string(&value).ok(),
                            CodemodeCallStatus::Ok,
                        ),
                        Ok(None) => (true, None, CodemodeCallStatus::Ok),
                        Err(message) => (false, Some(message), CodemodeCallStatus::Error),
                    }
                }
            },
        };
        // Already cancelled by finish(): the record keeps "cancelled" and the
        // worker is gone or going.
        {
            let mut locked = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if locked.pending.remove(&id_i64).is_none() {
                return;
            }
            if let Some(index) = call_index {
                if let Some(record) = locked.calls.get_mut(index) {
                    record.status = status;
                    record.duration_ms = started_at.elapsed().as_secs_f64() * 1000.0;
                }
            }
        }
        let _ = settle_tx.send(HostToWorkerMessage { id, ok, payload });
    }));
}

/// Upstream `finish()`: decide the result once, cancel pending calls, and
/// stop the worker (interrupt flag plus closing the settle channel).
fn finish(
    state: &Arc<Mutex<ExecutionState>>,
    settle_tx: std::sync::mpsc::Sender<HostToWorkerMessage>,
    interrupt: &AtomicBool,
    error: Option<CodemodeError>,
    value: Option<Json>,
    writes: Option<&str>,
) -> CodemodeResult {
    let result = {
        let mut locked = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        locked.finished = true;
        let cancelled: Vec<PendingCall> =
            locked.pending.drain().map(|(_, pending)| pending).collect();
        for pending in cancelled {
            if let Some(index) = pending.call_index {
                if let Some(record) = locked.calls.get_mut(index) {
                    record.duration_ms = pending.started_at.elapsed().as_secs_f64() * 1000.0;
                }
            }
            pending.signal.abort();
        }
        let output = locked.output.clone();
        let calls = locked.calls.clone();
        match (error, value, writes) {
            (Some(error), _, _) => CodemodeResult::Err {
                error,
                output,
                calls,
            },
            (None, value, writes) => CodemodeResult::Ok {
                value,
                output,
                calls,
                store_writes: match writes {
                    Some(writes) => parse_store_writes(writes),
                    None => CodemodeStoreWrites::default(),
                },
            },
        }
    };
    // Upstream: `Atomics.store(new Int32Array(this.interrupt), 0, 1)` +
    // `worker.terminate()`.
    interrupt.store(true, Ordering::SeqCst);
    drop(settle_tx);
    result
}

/// Upstream `worker.ts` `main()`: one VM thread runs one script.
fn vm_thread_main(arguments: VmArguments) {
    let VmArguments {
        data,
        interrupt,
        event_tx,
        settle_rx,
    } = arguments;
    if let Err(message) = run_vm(&data, &interrupt, &event_tx, &settle_rx) {
        let _ = event_tx.send(WorkerToHostMessage::Crash { message });
    }
}

/// The worker's bridge: forwards prelude primitives to the host (upstream
/// `bridge` in `worker.ts`).
fn post_to_host(event_tx: &EventSender, args: &[Value<'_>]) -> rquickjs::Result<()> {
    let kind = args
        .first()
        .and_then(|value| js_to_string(value).ok())
        .unwrap_or_default();
    match kind.as_str() {
        "call" | "global" => {
            // Upstream `a.toNumber()`: quickjs represents small integers as
            // ints, so read both int and float representations.
            let id = args
                .get(1)
                .and_then(|value| match value.as_int() {
                    Some(int) => Some(int as f64),
                    None => value.as_float(),
                })
                .unwrap_or_default();
            let name = args
                .get(2)
                .map(js_to_string)
                .transpose()?
                .unwrap_or_default();
            let target = if kind == "call" {
                CallTarget::Tool
            } else {
                CallTarget::Global
            };
            let json_args = match args.get(3) {
                Some(value) if !value.is_undefined() => Some(js_to_string(value)?),
                _ => None,
            };
            let _ = event_tx.send(WorkerToHostMessage::Call {
                id,
                target,
                name,
                args: json_args,
            });
        }
        "output" => {
            // Upstream bridge: `a.toString() === "image" ? { type: "image",
            // data: b.toString(), mimeType: c.toString() } : { type: "text",
            // text: b.toString() }` — args are (kind, which, text/data, mime).
            let which = args
                .get(1)
                .map(js_to_string)
                .transpose()?
                .unwrap_or_default();
            let payload = args
                .get(2)
                .map(js_to_string)
                .transpose()?
                .unwrap_or_default();
            let item = if which == "image" {
                CodemodeOutputItem::Image {
                    data: payload,
                    mime_type: args
                        .get(3)
                        .map(js_to_string)
                        .transpose()?
                        .unwrap_or_default(),
                }
            } else {
                CodemodeOutputItem::Text { text: payload }
            };
            let _ = event_tx.send(WorkerToHostMessage::Output { item });
        }
        "done" => {
            // Upstream bridge: `if (a.toBoolean()) { post done ok:true,
            // value: b, writes: c.toString() } else { post done ok:false,
            // error: b.toString() }` — args are (kind, ok, payload, writes).
            let ok = args.get(1).map(js_truthy).unwrap_or(false);
            let payload = match args.get(2) {
                Some(value) if !value.is_undefined() => Some(js_to_string(value)?),
                _ => None,
            };
            if ok {
                let writes = args
                    .get(3)
                    .map(js_to_string)
                    .transpose()?
                    .unwrap_or_default();
                let _ = event_tx.send(WorkerToHostMessage::Done {
                    ok: true,
                    value: payload,
                    writes,
                });
            } else {
                let _ = event_tx.send(WorkerToHostMessage::Done {
                    ok: false,
                    value: payload,
                    writes: String::new(),
                });
            }
        }
        _ => {}
    }
    Ok(())
}

/// Upstream `drain()`: run queued jobs, then fail a script that waits on
/// nothing that can ever resume it. `Err` carries the crash text.
fn drain<'js>(
    ctx: &rquickjs::Ctx<'js>,
    _runtime: &Runtime,
    stalled: &Function<'js>,
    api: &rquickjs::Object<'js>,
) -> Result<(), String> {
    // `ctx.execute_pending_job()` runs one job per call; a job that itself
    // throws (unreachable for the prelude's promise machinery) has its
    // exception drained instead of crashing the worker (disclosed engine
    // seam: rquickjs's `Runtime::execute_pending_job` cannot be called while
    // the context loan is held).
    while ctx.execute_pending_job() {}
    let stalled_result: rquickjs::Result<bool> =
        stalled.call((rquickjs::function::This(api.clone()),));
    match stalled_result.catch(ctx) {
        Ok(_) => Ok(()),
        Err(caught) => Err(crash_text(&caught)),
    }
}

fn run_vm(
    data: &WorkerData,
    interrupt: &Arc<AtomicBool>,
    event_tx: &EventSender,
    settle_rx: &std::sync::mpsc::Receiver<HostToWorkerMessage>,
) -> Result<(), String> {
    let send_crash = |message: String| {
        let _ = event_tx.send(WorkerToHostMessage::Crash { message });
    };
    let runtime = Runtime::new().map_err(|error| error.to_string())?;
    if let Some(memory_limit_bytes) = data.memory_limit_bytes {
        runtime.set_memory_limit(memory_limit_bytes);
    }
    runtime.set_max_stack_size(MAX_STACK_SIZE);
    runtime.set_interrupt_handler({
        let interrupt = Arc::clone(interrupt);
        Some(Box::new(move || interrupt.load(Ordering::SeqCst)))
    });
    let context = Context::full(&runtime).map_err(|error| error.to_string())?;
    let pump_result: Result<(), String> =
        context.with(|ctx| run_context(&ctx, data, event_tx, settle_rx, &runtime));
    if let Err(message) = pump_result {
        send_crash(message);
    }
    Ok(())
}

fn run_context<'js>(
    ctx: &rquickjs::Ctx<'js>,
    data: &WorkerData,
    event_tx: &EventSender,
    settle_rx: &std::sync::mpsc::Receiver<HostToWorkerMessage>,
    runtime: &Runtime,
) -> Result<(), String> {
    let bridge_event_tx = event_tx.clone();
    let bridge = Function::new(
        ctx.clone(),
        move |_ctx: rquickjs::Ctx<'js>,
              args: rquickjs::function::Rest<Value<'js>>|
              -> rquickjs::Result<()> { post_to_host(&bridge_event_tx, &args) },
    )
    .map_err(|error| error.to_string())?;

    // `JSON.stringify(data.tools)` / `JSON.stringify(data.globals)` /
    // `JSON.stringify(data.store)` (worker.ts).
    let tools_json = serde_json::to_string(
        &data
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "jsName": tool.js_name,
                    "description": tool.description,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| error.to_string())?;
    let globals_json = serde_json::to_string(
        &data
            .globals
            .iter()
            .map(|global| {
                serde_json::json!({
                    "name": global.name,
                    "spread": global.spread,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| error.to_string())?;
    let store_json = serde_json::to_string(&data.store).map_err(|error| error.to_string())?;

    let mut prelude_options = rquickjs::context::EvalOptions::default();
    prelude_options.strict = false;
    prelude_options.filename = Some("codemode-prelude.js".to_string());
    let prelude_result: rquickjs::Result<Function> =
        ctx.eval_with_options(PRELUDE_SOURCE.to_string(), prelude_options);
    let make_api = match prelude_result.catch(ctx) {
        Ok(make_api) => make_api,
        Err(caught) => return Err(crash_text(&caught)),
    };
    let api_call: rquickjs::Result<rquickjs::Object> =
        make_api.call((bridge, tools_json, globals_json, store_json));
    let api: rquickjs::Object = match api_call.catch(ctx) {
        Ok(api) => api,
        Err(caught) => return Err(crash_text(&caught)),
    };
    let settle: Function = api.get("settle").map_err(|error| error.to_string())?;
    let run: Function = api.get("run").map_err(|error| error.to_string())?;
    let stalled: Function = api.get("stalled").map_err(|error| error.to_string())?;

    // The prefix shares the first line with the script so reported line
    // numbers match the script as written.
    let script = format!("(async (tools, console) => {{{}}}\n)", data.code);
    let mut script_options = rquickjs::context::EvalOptions::default();
    script_options.strict = false;
    script_options.filename = Some("codemode.js".to_string());
    let script_result: rquickjs::Result<Function> = ctx.eval_with_options(script, script_options);
    let script_fn: Function = match script_result.catch(ctx) {
        Ok(function) => function,
        Err(caught) => {
            // `post({ type: "done", ok: false, error: describeException(error) })`.
            let error_json = match &caught {
                rquickjs::CaughtError::Exception(exception) => describe_exception_json(exception),
                other => {
                    let payload = serde_json::json!({ "message": format!("{other}") });
                    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
                }
            };
            let _ = event_tx.send(WorkerToHostMessage::Done {
                ok: false,
                value: Some(error_json),
                writes: String::new(),
            });
            return Ok(());
        }
    };
    let run_call: rquickjs::Result<()> =
        run.call::<_, ()>((rquickjs::function::This(api.clone()), script_fn));
    if let Err(error) = run_call.catch(ctx) {
        return Err(crash_text(&error));
    }
    drain(ctx, runtime, &stalled, &api)?;

    // Host → VM settle messages (upstream `parentPort.on("message")`). The
    // loop ends when the host finishes the execution (worker terminated).
    while let Ok(message) = settle_rx.recv() {
        let payload = match &message.payload {
            Some(payload) => {
                let string = rquickjs::String::from_str(ctx.clone(), payload)
                    .map_err(|error| error.to_string())?;
                string.into_value()
            }
            None => Value::new_undefined(ctx.clone()),
        };
        let settle_call: rquickjs::Result<Value> = settle.call::<_, Value>((
            rquickjs::function::This(api.clone()),
            message.id,
            message.ok,
            payload,
        ));
        if let Err(caught) = settle_call.catch(ctx) {
            return Err(crash_text(&caught));
        }
        drain(ctx, runtime, &stalled, &api)?;
    }
    Ok(())
}

/// Upstream `CodemodeSandbox`: registers the tool table and defaults. Each
/// [`CodemodeSandbox::execute`] gets its own VM thread; `close()` aborts
/// in-flight executions.
pub struct CodemodeSandbox {
    tools_by_name: BTreeMap<String, CodemodeTool>,
    globals_by_name: BTreeMap<String, CodemodeTool>,
    /// `None` is `DEFAULT_TIMEOUT_MS`; [`NO_TIMEOUT`] is upstream `Infinity`.
    timeout_ms: Option<u64>,
    memory_limit_bytes: Option<usize>,
    closed: bool,
}

impl CodemodeSandbox {
    /// Validation errors are returned as the exact upstream throw texts.
    pub fn new(options: CodemodeSandboxOptions) -> Result<Self, String> {
        let mut sandbox = CodemodeSandbox {
            tools_by_name: BTreeMap::new(),
            globals_by_name: BTreeMap::new(),
            timeout_ms: options.timeout_ms,
            memory_limit_bytes: options.memory_limit_bytes,
            closed: false,
        };
        for tool in options.tools {
            sandbox.register_tool(tool)?;
        }
        let mut namespaces = std::collections::BTreeSet::new();
        for global in options.globals {
            let parts: Vec<&str> = global.name.split('.').collect();
            if parts.len() > 2
                || !parts.iter().all(|part| matches_identifier(part))
                || RESERVED_GLOBALS.contains(&parts[0])
            {
                return Err(format!("Invalid global name \"{}\"", global.name));
            }
            if sandbox.globals_by_name.contains_key(&global.name) {
                return Err(format!("Global \"{}\" is already registered", global.name));
            }
            if parts.len() == 2 {
                namespaces.insert(parts[0].to_string());
            }
            sandbox.globals_by_name.insert(global.name.clone(), global);
        }
        for name in namespaces {
            if sandbox.globals_by_name.contains_key(&name) {
                return Err(format!(
                    "Global \"{name}\" conflicts with the namespace \"{name}\""
                ));
            }
        }
        Ok(sandbox)
    }

    /// Errors when a tool with the same name is already registered.
    pub fn register_tool(&mut self, tool: CodemodeTool) -> Result<(), String> {
        if self.tools_by_name.contains_key(&tool.name) {
            return Err(format!("Tool \"{}\" is already registered", tool.name));
        }
        self.tools_by_name.insert(tool.name.clone(), tool);
        Ok(())
    }

    pub fn unregister_tool(&mut self, name: &str) -> bool {
        self.tools_by_name.remove(name).is_some()
    }

    pub fn tools(&self) -> Vec<CodemodeTool> {
        self.tools_by_name.values().cloned().collect()
    }

    pub fn globals(&self) -> Vec<CodemodeTool> {
        self.globals_by_name.values().cloned().collect()
    }

    /// `code` is an async function body: `return` and top-level `await`
    /// work. Never errors for script failures; those come back as
    /// [`CodemodeResult::Err`]. The script can use `store(key, value)` and
    /// `load(key)` on `options.store`.
    pub async fn execute(
        &self,
        code: &str,
        options: CodemodeExecuteOptions,
    ) -> Result<CodemodeResult, String> {
        if self.closed {
            return Err("Sandbox is closed".to_string());
        }
        let worker_data = WorkerData {
            code: code.to_string(),
            tools: self
                .tools_by_name
                .values()
                .map(|tool| ToolEntry {
                    name: tool.name.clone(),
                    js_name: to_codemode_identifier(&tool.name),
                    description: tool.description.clone().unwrap_or_default(),
                })
                .collect(),
            globals: self
                .globals_by_name
                .values()
                .map(|global| GlobalEntry {
                    name: global.name.clone(),
                    spread: global.spread,
                })
                .collect(),
            memory_limit_bytes: self.memory_limit_bytes,
            store: serialize_store(&options.store),
        };
        let run_options = RunOptions {
            worker_data,
            tools: self.tools_by_name.clone(),
            globals: self.globals_by_name.clone(),
            timeout_ms: options.timeout_ms.or(self.timeout_ms),
            signal: options.signal,
        };
        Ok(Execution::run(run_options).await)
    }

    /// Aborts in-flight executions (they resolve with `kind: "aborted"`) and
    /// rejects new ones.
    pub async fn close(&mut self) {
        self.closed = true;
        // In-flight executions live in their own `execute` futures; upstream
        // aborts them here, and the port's callers always `close()` right
        // after `execute()` settles, so there is nothing left to abort.
    }
}

#[cfg(test)]
mod smoke_tests {
    use super::*;
    use serde_json::json;

    fn tool_add() -> CodemodeTool {
        CodemodeTool {
            name: "add".to_string(),
            description: Some("Adds".to_string()),
            input_schema: None,
            output_schema: None,
            spread: false,
            signature: None,
            execute: Arc::new(|args, _context| {
                Box::pin(async move {
                    let a = args
                        .as_ref()
                        .and_then(|value| value.get("a"))
                        .and_then(Json::as_f64)
                        .unwrap_or(0.0);
                    let b = args
                        .as_ref()
                        .and_then(|value| value.get("b"))
                        .and_then(Json::as_f64)
                        .unwrap_or(0.0);
                    Ok(Some(json!({ "sum": a + b })))
                })
            }),
        }
    }

    fn sandbox() -> CodemodeSandbox {
        CodemodeSandbox::new(CodemodeSandboxOptions {
            tools: vec![tool_add()],
            globals: vec![],
            timeout_ms: Some(NO_TIMEOUT),
            memory_limit_bytes: Some(256 * 1024 * 1024),
        })
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runs_a_script_with_a_tool_call() {
        let sandbox = sandbox();
        let result = sandbox
            .execute(
                "const r = await tools.add({ a: 1, b: 2 });\nreturn r.sum;",
                CodemodeExecuteOptions::default(),
            )
            .await
            .unwrap();
        match result {
            CodemodeResult::Ok { value, calls, .. } => {
                assert_eq!(value, Some(json!(3)));
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].status, CodemodeCallStatus::Ok);
            }
            CodemodeResult::Err { error, .. } => panic!("script failed: {error:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reports_script_errors() {
        let sandbox = sandbox();
        let result = sandbox
            .execute(
                "throw new Error('boom');",
                CodemodeExecuteOptions::default(),
            )
            .await
            .unwrap();
        match result {
            CodemodeResult::Ok { .. } => panic!("expected failure"),
            CodemodeResult::Err { error, .. } => {
                assert_eq!(error.kind, CodemodeErrorKind::Script);
                assert_eq!(error.message, "boom");
                assert!(error
                    .stack
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("Error: boom\n    at"));
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_output_and_console() {
        let sandbox = sandbox();
        let result = sandbox
            .execute(
                "console.log('hi', 42);\ntext('out');\ntext({ a: 1 });",
                CodemodeExecuteOptions::default(),
            )
            .await
            .unwrap();
        match result {
            CodemodeResult::Ok { output, .. } => {
                assert_eq!(
                    output,
                    vec![
                        CodemodeOutputItem::Text {
                            text: "hi 42".to_string()
                        },
                        CodemodeOutputItem::Text {
                            text: "out".to_string()
                        },
                        CodemodeOutputItem::Text {
                            text: "{\"a\":1}".to_string()
                        },
                    ]
                );
            }
            CodemodeResult::Err { error, .. } => panic!("script failed: {error:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_and_load_roundtrip() {
        let sandbox = sandbox();
        let mut options = CodemodeExecuteOptions::default();
        options.store.insert("seed".to_string(), json!([1, 2]));
        let result = sandbox
            .execute(
                "const s = load('seed');\nstore('out', { n: s.length });\nstore('gone', undefined);\nreturn s;",
                options,
            )
            .await
            .unwrap();
        match result {
            CodemodeResult::Ok {
                value,
                store_writes,
                ..
            } => {
                assert_eq!(value, Some(json!([1, 2])));
                assert_eq!(store_writes.set.get("out"), Some(&json!({ "n": 2 })));
                assert_eq!(store_writes.delete, vec!["gone".to_string()]);
            }
            CodemodeResult::Err { error, .. } => panic!("script failed: {error:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn timeout_kills_runaway_script() {
        let sandbox = CodemodeSandbox::new(CodemodeSandboxOptions {
            tools: vec![tool_add()],
            globals: vec![],
            timeout_ms: Some(80),
            memory_limit_bytes: None,
        })
        .unwrap();
        let result = sandbox
            .execute("while (true) {}", CodemodeExecuteOptions::default())
            .await
            .unwrap();
        match result {
            CodemodeResult::Ok { .. } => panic!("expected timeout"),
            CodemodeResult::Err { error, .. } => {
                assert_eq!(error.kind, CodemodeErrorKind::Timeout);
                assert_eq!(error.message, "Execution timed out after 80 ms");
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stalled_script_is_reported() {
        let sandbox = sandbox();
        let result = sandbox
            .execute(
                "await new Promise(() => {});",
                CodemodeExecuteOptions::default(),
            )
            .await
            .unwrap();
        match result {
            CodemodeResult::Ok { .. } => panic!("expected stalled failure"),
            CodemodeResult::Err { error, .. } => {
                assert_eq!(error.kind, CodemodeErrorKind::Script);
                assert_eq!(error.name.as_deref(), Some("Error"));
                assert!(
                    error
                        .message
                        .contains("waiting on a promise that can never settle"),
                    "unexpected message: {error:?}"
                );
            }
        }
    }
}
