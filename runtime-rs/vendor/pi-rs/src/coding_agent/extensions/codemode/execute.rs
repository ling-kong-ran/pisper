//! Port of upstream `coding-agent/src/extensions/codemode/execute.ts` — runs
//! one codemode script in the sandbox (upstream splits it from `tool.ts` and
//! loads it through `execute.lazy.ts`; the port loads it directly).
//!
//! Pipeline (upstream `executeCodemode`): parse the source, build sandbox
//! tools from the session's callable tools (nested calls run through
//! `ctx.executeTool`, so validation, `tool_call`/`tool_result` hooks, and
//! permission checks apply exactly as for direct calls), register the
//! discovery and `models` globals, run the script, then format the result
//! with a "Script completed"/"Script failed" header, the token budget, and
//! the nested-call snapshot.
//!
//! Disclosed divergences (seam-level; one per item):
//!
//! - `AgentToolCallOutcome` travels as JSON; `structuredContent` is read off
//!   the outcome's `result` value.
//! - Upstream returns an `isError: true` tool result (keeping partial output)
//!   for failed scripts. The port's `AgentToolResult` (agent-core seam) has
//!   no `isError` field, so the flag cannot travel through the execute seam;
//!   the failed-script result keeps byte-identical content (header, partial
//!   output, "Script error:" block) and the upstream `isError` key.
//! - `models.*` globals need the model-registry seam (`ctx.modelRegistry`),
//!   which the runner does not surface yet; they are created only when a
//!   [`CodemodeModelRuntime`] handle is supplied via the tool options.
//! - Durations use [`Instant`] (upstream `performance.now()`); the spill file
//!   uses `std::env::temp_dir` with 8 random bytes (`rand`), matching
//!   upstream's `pi-codemode-<hex>.txt` name shape.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::tool::{
    get_codemode_callable_tools, to_codemode_declaration, CodemodeModelRuntime, CodemodeNestedCall,
    CodemodeNestedCallStatus, CodemodeStoreEntryData, CodemodeToolDetails, CodemodeToolOptions,
    CODEMODE_STORE_ENTRY_TYPE,
};
use crate::agent_core::types::AgentTool;
use crate::codemode::declarations::render_tool_sample;
use crate::codemode::identifier::to_codemode_identifier;
use crate::codemode::source::parse_codemode_source;
use crate::codemode::types::{
    CodemodeError, CodemodeErrorKind, CodemodeExecuteOptions, CodemodeOutputItem, CodemodeResult,
    CodemodeSandboxOptions, CodemodeTool,
};
use crate::codemode::CodemodeSandbox;
use crate::coding_agent::extensions::tool_search::{
    create_tool_search_document, Bm25Ranker, ToolRanker, DEFAULT_TOOL_SEARCH_LIMIT,
};
use crate::coding_agent::extensions::types::{
    AgentToolUpdateCallbackValue, ExtensionContext, HandlerError, ToolNamespace,
};

/// Upstream `ARGS_PREVIEW_CHARS`.
const ARGS_PREVIEW_CHARS: usize = 200;
/// Upstream `ERROR_PREVIEW_CHARS`.
const ERROR_PREVIEW_CHARS: usize = 500;
/// Upstream `MAX_CONCURRENT_MODEL_CALLS` (classifier calls one script may
/// have in flight; `Promise.all` over many items queues the rest).
const MAX_CONCURRENT_MODEL_CALLS: usize = 4;
/// Upstream `CODEMODE_MEMORY_LIMIT_BYTES` (256 MiB heap limit).
const CODEMODE_MEMORY_LIMIT_BYTES: usize = 256 * 1024 * 1024;
/// Upstream `DEFAULT_MAX_OUTPUT_TOKENS`.
const DEFAULT_MAX_OUTPUT_TOKENS: f64 = 10_000.0;
/// Upstream `CHARS_PER_TOKEN` (budget estimate).
const CHARS_PER_TOKEN: f64 = 4.0;

/// Upstream `truncateText` (`text.length` is the UTF-16 length; slices take
/// UTF-16 units — unpaired surrogate tails cannot exist in JSON strings and
/// are dropped from the display-only preview).
fn truncate_text(text: &str, max_chars: usize) -> String {
    let limit = max_chars.saturating_sub(3);
    let mut units = 0usize;
    let mut cut = String::new();
    for character in text.chars() {
        if units + character.len_utf16() > limit {
            break;
        }
        units += character.len_utf16();
        cut.push(character);
    }
    if text.encode_utf16().count() > max_chars {
        format!("{cut}...")
    } else {
        text.to_string()
    }
}

/// Upstream `previewArgs`: compact JSON, truncated for display.
fn preview_args(args: Option<&Value>) -> String {
    let Some(args) = args else {
        return String::new();
    };
    match serde_json::to_string(args) {
        Ok(json) => truncate_text(&json, ARGS_PREVIEW_CHARS),
        Err(_) => String::new(),
    }
}

/// Upstream `textOf`: the tool result's text blocks joined with "\n".
fn text_of(result: &Value) -> String {
    result
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Upstream `toModelType`.
fn to_model_type(value: &Value) -> Result<String, String> {
    const MODEL_TYPES: [&str; 3] = ["chat", "image", "classifier"];
    if let Some(text) = value.as_str() {
        if MODEL_TYPES.contains(&text) {
            return Ok(text.to_string());
        }
    }
    Err(format!(
        "Unknown model type {}. Use \"chat\", \"image\", or \"classifier\".",
        serde_json::to_string(value).unwrap_or_else(|_| "undefined".to_string())
    ))
}

/// Upstream `toProvider`.
fn to_provider(value: &Value) -> Result<Option<String>, String> {
    if value.is_null() {
        return Ok(None);
    }
    match value.as_str() {
        Some(provider) => Ok(Some(provider.to_string())),
        None => Err("provider must be a string".to_string()),
    }
}

/// Upstream `toModelInfo`: catalog entry for scripts. `headers` is dropped
/// because models.json headers can carry credentials.
fn to_model_info(model: &Value) -> Value {
    let mut info = model.clone();
    if let Some(object) = info.as_object_mut() {
        object.remove("headers");
    }
    info
}

/// Upstream `isStoreEntryData`.
fn is_store_entry_data(data: &Value) -> Option<CodemodeStoreEntryData> {
    let object = data.as_object()?;
    let set = object.get("set")?.as_object()?;
    let deleted = object.get("delete")?.as_array()?;
    if !deleted.iter().all(Value::is_string) {
        return None;
    }
    Some(CodemodeStoreEntryData {
        set: set
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        delete: deleted
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    })
}

/// Upstream `readCodemodeStore`: values of `load()` — the `codemode-store`
/// entries on the branch, applied from the root.
pub fn read_codemode_store(
    branch: &[crate::coding_agent::session_manager::SessionEntry],
) -> BTreeMap<String, Value> {
    let mut store: BTreeMap<String, Value> = BTreeMap::new();
    for entry in branch {
        let crate::coding_agent::session_manager::SessionEntry::Custom(custom) = entry else {
            continue;
        };
        if custom.custom_type != CODEMODE_STORE_ENTRY_TYPE {
            continue;
        }
        let Some(data) = custom.data.as_ref().and_then(is_store_entry_data) else {
            continue;
        };
        for key in &data.delete {
            store.remove(key);
        }
        for (key, value) in &data.set {
            store.insert(key.clone(), value.clone());
        }
    }
    store
}

/// Upstream `valueText`: like the script's `text()` — strings as is, other
/// values as compact JSON.
fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// Upstream `formatCallSummary`.
fn format_call_summary(calls: &[CodemodeNestedCall]) -> String {
    if calls.is_empty() {
        return "No tool calls were made.".to_string();
    }
    let list = calls
        .iter()
        .map(|call| format!("{} ({})", call.name, call.status.to_str()))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Tool calls made before the failure (they are not undone): {list}")
}

/// Upstream `formatError`.
fn format_error(error: &CodemodeError, calls: &[CodemodeNestedCall]) -> String {
    let head = match error.kind {
        CodemodeErrorKind::Script => error.stack.clone().unwrap_or_else(|| {
            format!(
                "{}: {}",
                error.name.as_deref().unwrap_or("Error"),
                error.message
            )
        }),
        CodemodeErrorKind::Timeout => format!("Script timed out: {}", error.message),
        CodemodeErrorKind::Aborted => format!("Script aborted: {}", error.message),
        CodemodeErrorKind::Sandbox => format!("Script sandbox failed: {}", error.message),
    };
    format!("{head}\n\n{}", format_call_summary(calls))
}

/// Upstream `spillOutput`: write the full text output to a temp file, like
/// bash does for truncated output.
fn spill_output(text: &str) -> Result<String, String> {
    let bytes: [u8; 8] = rand::random();
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = std::env::temp_dir().join(format!("pi-codemode-{hex}.txt"));
    std::fs::write(&path, text).map_err(|error| error.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Upstream `truncateOutput`: apply the token budget — when the combined text
/// exceeds it, the text items become one item that keeps the start and end of
/// the text, and images follow it. Returns the new items and the spilled full
/// output path. Text is sliced on UTF-16 units like JS `.slice` (unpaired
/// surrogate tails are dropped; the text is display output).
fn truncate_output(
    items: &[Value],
    max_tokens: f64,
) -> Result<(Vec<Value>, Option<String>), String> {
    let texts: Vec<&str> = items
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect();
    let combined = texts.join("\n");
    let budget = max_tokens * CHARS_PER_TOKEN;
    let combined_units = combined.encode_utf16().count();
    if texts.is_empty() || (combined_units as f64) <= budget {
        return Ok((items.to_vec(), None));
    }
    let budget = budget as usize;
    let head_chars = budget / 2;
    let tail_chars = budget - head_chars;
    let units: Vec<u16> = combined.encode_utf16().collect();
    let removed = units.len() - head_chars - tail_chars;
    let head = utf16_slice(&units, 0, head_chars);
    let tail = if tail_chars > 0 {
        utf16_slice(&units, units.len() - tail_chars, units.len())
    } else {
        String::new()
    };
    let mut text = format!(
        "Warning: truncated output (original token count: {})\nTotal output lines: {}\n\n{}…{} tokens truncated…{}",
        (units.len() as f64 / CHARS_PER_TOKEN).ceil(),
        combined.split('\n').count(),
        head,
        (removed as f64 / CHARS_PER_TOKEN).ceil(),
        tail
    );
    let full_output_path = match spill_output(&combined) {
        Ok(path) => {
            text.push_str(&format!(
                "\n\n[Full output: {path} (read with offset/limit)]"
            ));
            Some(path)
        }
        Err(error) => {
            text.push_str(&format!("\n\n[Could not save the full output: {error}]"));
            None
        }
    };
    let mut result = vec![json!({ "type": "text", "text": text })];
    result.extend(
        items
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("image"))
            .cloned(),
    );
    Ok((result, full_output_path))
}

/// JS `text.slice(start, end)` over UTF-16 units, decoded (unpaired
/// surrogates dropped).
fn utf16_slice(units: &[u16], start: usize, end: usize) -> String {
    units[start..end]
        .iter()
        .filter_map(|unit| char::from_u32(*unit as u32))
        .collect()
}

/// Upstream `toScriptValue`: the value a script receives for a nested call —
/// a tool that declares `outputSchema` resolves to its `structuredContent`
/// (also for error results that carry one, such as MCP results with
/// `isError`); any other tool resolves to its text content. Failures reject
/// with the tool's error text.
fn to_script_value(tool: &AgentTool, outcome: &Value) -> Result<Value, String> {
    let result = &outcome["result"];
    if result
        .get("structuredContent")
        .is_some_and(Value::is_object)
    {
        return Ok(result["structuredContent"].clone());
    }
    let text = text_of(result);
    if outcome
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        if text.is_empty() {
            return Err(format!("Tool \"{}\" failed", tool.name));
        }
        return Err(text);
    }
    Ok(json!(text))
}

/// Publishes a details-only update (upstream `publish`).
fn publish_details(
    calls: &Mutex<Vec<CodemodeNestedCall>>,
    on_update: Option<&AgentToolUpdateCallbackValue>,
) {
    if let Some(on_update) = on_update {
        let locked = calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        on_update(&json!({
            "content": [],
            "details": CodemodeToolDetails {
                calls: locked.clone(),
                full_output_path: None,
            }
            .to_value(),
        }));
    }
}

/// Upstream `executeCodemode`. `ctx` is the extension tool context created for
/// this tool call; without a session context scripts cannot call tools,
/// `store()` starts empty, and writes are dropped.
pub async fn execute_codemode(
    tool_call_id: &str,
    input: &Value,
    signal: Option<Arc<crate::coding_agent::extensions::types::AbortSignal>>,
    on_update: Option<AgentToolUpdateCallbackValue>,
    ctx: &ExtensionContext,
    options: CodemodeToolOptions,
) -> Result<Value, HandlerError> {
    let started_at = std::time::Instant::now();
    let code = input
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Upstream lets `parseCodemodeSource` throw; a rejected input becomes an
    // execute error carrying the message.
    let source = parse_codemode_source(code).map_err(|error| error.0)?;
    let calls: Arc<Mutex<Vec<CodemodeNestedCall>>> = Arc::new(Mutex::new(Vec::new()));
    // Usage of the script's `models.*` calls. Nested tool calls report theirs
    // through the session.
    let model_usage: Arc<Mutex<Option<crate::ai::types::primitives::Usage>>> =
        Arc::new(Mutex::new(None));

    let session_tools = ctx.callable_tools();
    let callable = get_codemode_callable_tools(&session_tools);
    // ALL_TOOLS entries carry the declaration.
    let samples: BTreeMap<String, String> = callable
        .iter()
        .map(|tool| {
            (
                tool.name.clone(),
                render_tool_sample(&to_codemode_declaration(tool), None),
            )
        })
        .collect();

    let sandbox_tools: Vec<CodemodeTool> = callable
        .iter()
        .map(|tool| {
            let tool = tool.clone();
            let name = tool.name.clone();
            let description = samples.get(&name).cloned();
            let calls = Arc::clone(&calls);
            let on_update = on_update.clone();
            let ctx = ctx.clone();
            CodemodeTool {
                name,
                description,
                input_schema: None,
                output_schema: None,
                spread: false,
                signature: None,
                execute: Arc::new(move |args, context| {
                    let tool = tool.clone();
                    let calls = Arc::clone(&calls);
                    let on_update = on_update.clone();
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let running_key = format!("{}/?", tool.name);
                        let call_started_at = std::time::Instant::now();
                        {
                            let mut locked = calls
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            locked.push(CodemodeNestedCall {
                                id: running_key.clone(),
                                name: tool.name.clone(),
                                args: preview_args(args.as_ref()),
                                status: CodemodeNestedCallStatus::Running,
                                duration_ms: None,
                                error: None,
                                cost: None,
                            });
                        }
                        publish_details(&calls, on_update.as_ref());
                        // Only tools from ctx.tools are callable, so a tool
                        // context exists here (the upstream "Tool calls need a
                        // session" throw has no reachable equivalent).
                        let outcome = ctx
                            .execute_tool(
                                &tool.name,
                                args.as_ref().unwrap_or(&Value::Null),
                                crate::coding_agent::extensions::types::ExecuteToolOptions {
                                    signal: Some(Arc::clone(&context.signal)),
                                    on_update: None,
                                },
                            )
                            .await?;
                        let duration = call_started_at.elapsed().as_secs_f64() * 1000.0;
                        {
                            let mut locked = calls
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if let Some(record) =
                                locked.iter_mut().find(|record| record.id == running_key)
                            {
                                record.id = outcome
                                    .get("toolCall")
                                    .and_then(|call| call.get("id"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                record.duration_ms = Some(duration);
                                if outcome
                                    .get("isError")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false)
                                {
                                    record.status = if context.signal.is_aborted() {
                                        CodemodeNestedCallStatus::Cancelled
                                    } else {
                                        CodemodeNestedCallStatus::Error
                                    };
                                    let text = text_of(&outcome["result"]);
                                    let failed_text = if text.is_empty() {
                                        format!("Tool \"{}\" failed", tool.name)
                                    } else {
                                        text
                                    };
                                    record.error =
                                        Some(truncate_text(&failed_text, ERROR_PREVIEW_CHARS));
                                } else {
                                    record.status = CodemodeNestedCallStatus::Ok;
                                }
                            }
                        }
                        publish_details(&calls, on_update.as_ref());
                        to_script_value(&tool, &outcome).map(Some)
                    })
                }),
            }
        })
        .collect();

    let globals = [
        create_discovery_globals(&callable, &samples, &options),
        create_model_globals_if_bound(&options, tool_call_id, &calls, &on_update, &model_usage),
    ]
    .concat();

    let sandbox = CodemodeSandbox::new(CodemodeSandboxOptions {
        tools: sandbox_tools,
        globals,
        timeout_ms: source.options.timeout_ms,
        memory_limit_bytes: Some(CODEMODE_MEMORY_LIMIT_BYTES),
    })?;

    let store = session_store(ctx);
    let result = sandbox
        .execute(
            &source.code,
            CodemodeExecuteOptions {
                signal,
                timeout_ms: None,
                store,
            },
        )
        .await;
    drop(sandbox);

    let result = result?;
    // Calls still marked running were cut off by the script ending, a timeout,
    // or an abort.
    {
        let mut locked = calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for call in locked.iter_mut() {
            if call.status == CodemodeNestedCallStatus::Running {
                call.status = CodemodeNestedCallStatus::Cancelled;
            }
        }
    }

    let items: Vec<Value> = result
        .output()
        .iter()
        .map(|item| match item {
            CodemodeOutputItem::Text { text } => json!({ "type": "text", "text": text }),
            CodemodeOutputItem::Image { data, mime_type } => {
                json!({ "type": "image", "data": data, "mimeType": mime_type })
            }
        })
        .collect();

    let mut content = items;
    match &result {
        CodemodeResult::Ok {
            value,
            store_writes,
            ..
        } => {
            if !store_writes.set.is_empty() || !store_writes.delete.is_empty() {
                if let Some(append_entry) = &options.append_entry {
                    append_entry(
                        CODEMODE_STORE_ENTRY_TYPE,
                        &CodemodeStoreEntryData {
                            set: store_writes.set.clone(),
                            delete: store_writes.delete.clone(),
                        },
                    );
                }
            }
            // pi extension: a returned value is appended like text().
            if let Some(value) = value {
                content.push(json!({ "type": "text", "text": value_text(value) }));
            }
        }
        CodemodeResult::Err { error, .. } => {
            let locked = calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            content.push(json!({
                "type": "text",
                "text": format!("Script error:\n{}", format_error(error, &locked)),
            }));
        }
    }

    let (truncated, full_output_path) = truncate_output(
        &content,
        source
            .options
            .max_output_tokens
            .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS),
    )?;
    let wall_time = format!("{:.1}", started_at.elapsed().as_secs_f64());
    let header = format!(
        "{}\nWall time {} seconds\nOutput:\n",
        if result.is_ok() {
            "Script completed"
        } else {
            "Script failed"
        },
        wall_time
    );
    let mut details = CodemodeToolDetails {
        calls: calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
        full_output_path: full_output_path.clone(),
    };
    if full_output_path.is_none() {
        details.full_output_path = None;
    }

    let mut content_value = vec![json!({ "type": "text", "text": header })];
    content_value.extend(truncated);

    let mut result_value = serde_json::Map::new();
    result_value.insert("content".to_string(), Value::Array(content_value));
    result_value.insert("details".to_string(), details.to_value());
    if let CodemodeResult::Err { .. } = &result {
        // Upstream marks the tool result as an error (`isError: true`) while
        // keeping the partial output; see the module docs (the port's
        // AgentToolResult has no isError field — the key is still written so
        // downstream JSON consumers see the upstream shape).
        result_value.insert("isError".to_string(), json!(true));
    }
    let usage = *model_usage
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(usage) = usage {
        result_value.insert(
            "usage".to_string(),
            serde_json::to_value(usage).map_err(|error| error.to_string())?,
        );
    }
    Ok(Value::Object(result_value))
}

/// The `models.*` globals when a model runtime handle is bound (upstream:
/// `options.models && ctx ? createModelGlobals(ctx.modelRegistry, …) : []`).
fn create_model_globals_if_bound(
    options: &CodemodeToolOptions,
    tool_call_id: &str,
    calls: &Arc<Mutex<Vec<CodemodeNestedCall>>>,
    on_update: &Option<AgentToolUpdateCallbackValue>,
    model_usage: &Arc<Mutex<Option<crate::ai::types::primitives::Usage>>>,
) -> Vec<CodemodeTool> {
    if !options.models {
        return Vec::new();
    }
    let Some(runtime) = options.model_runtime.clone() else {
        // No model-registry seam yet: the `models` namespace is not created
        // (disclosed).
        return Vec::new();
    };
    let publish = {
        let calls = Arc::clone(calls);
        let on_update = on_update.clone();
        move || publish_details(&calls, on_update.as_ref())
    };
    let add_usage = {
        let model_usage = Arc::clone(model_usage);
        Arc::new(move |usage: crate::ai::types::primitives::Usage| {
            let mut locked = model_usage
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *locked = Some(match locked.take() {
                Some(running) => {
                    crate::coding_agent::core::usage_totals::combine_usage(running, usage)
                }
                None => usage,
            });
        })
    };
    create_model_globals(runtime, tool_call_id, Arc::clone(calls), publish, add_usage)
}

/// Upstream `ctx.sessionManager.getBranch()` → the values of `load()`. The
/// native session manager is the same typed handle the extension runner uses
/// (mirrors `core/tools/bash.rs`).
fn session_store(ctx: &ExtensionContext) -> BTreeMap<String, Value> {
    use crate::coding_agent::session_manager::SessionManager;
    let Ok(handle) = ctx.session_manager() else {
        return BTreeMap::new();
    };
    let Some(session) = handle.downcast_ref::<std::sync::Mutex<SessionManager>>() else {
        return BTreeMap::new();
    };
    let session = session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    read_codemode_store(&session.get_branch(None))
}

/// Upstream `createDiscoveryGlobals`: `searchTools()`, `describeTool()`, and
/// `describeNamespace()` — ranked search and lookup over the script's nested
/// tools and their namespaces.
fn create_discovery_globals(
    tools: &[AgentTool],
    samples: &BTreeMap<String, String>,
    options: &CodemodeToolOptions,
) -> Vec<CodemodeTool> {
    fn global(
        name: &str,
        execute: impl Fn(Option<Value>) -> Result<Option<Value>, String> + Send + Sync + Clone + 'static,
    ) -> CodemodeTool {
        CodemodeTool {
            name: name.to_string(),
            description: None,
            input_schema: None,
            output_schema: None,
            spread: true,
            signature: None,
            execute: Arc::new(move |args, _context| {
                let execute = execute.clone();
                Box::pin(async move { execute(args) })
            }),
        }
    }
    let ranker = Bm25Ranker::new();
    fn entry(samples: &BTreeMap<String, String>, name: &str) -> Value {
        json!({
            "name": to_codemode_identifier(name),
            "description": samples.get(name).cloned().unwrap_or_default(),
        })
    }
    vec![
        {
            let tools = tools.to_vec();
            let options = options.clone();
            let samples = samples.clone();
            global("searchTools", move |args| {
                let args = args
                    .and_then(|value| value.as_array().cloned())
                    .unwrap_or_default();
                let query = args.first().cloned().unwrap_or(Value::Null);
                let search_options = args.get(1).cloned().unwrap_or(Value::Null);
                let Value::String(query) = query else {
                    return Err("searchTools() expects a query string".to_string());
                };
                let limit = match search_options.get("limit") {
                    None | Some(Value::Null) => DEFAULT_TOOL_SEARCH_LIMIT,
                    Some(value) => match value.as_f64() {
                        Some(limit) if limit.is_finite() && limit.fract() == 0.0 && limit > 0.0 => {
                            limit as usize
                        }
                        _ => {
                            return Err(
                                "searchTools() limit must be a positive integer".to_string()
                            );
                        }
                    },
                };
                let namespace = match search_options.get("namespace") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(namespace)) => Some(namespace.clone()),
                    Some(_) => {
                        return Err("searchTools() namespace must be a string".to_string());
                    }
                };
                let mut documents = Vec::new();
                for tool in &tools {
                    let tool_namespace = options
                        .get_tool_namespace
                        .as_ref()
                        .and_then(|get| get(&tool.name));
                    if let Some(wanted) = &namespace {
                        match &tool_namespace {
                            Some(tool_namespace) if tool_namespace.name == *wanted => {}
                            _ => continue,
                        }
                    }
                    documents.push(create_tool_search_document(
                        &tool.name,
                        &tool.description,
                        &tool.parameters,
                        tool_namespace.as_ref(),
                    ));
                }
                Ok(Some(json!(ranker
                    .rank(&query, &documents, limit)
                    .into_iter()
                    .map(|matched| entry(&samples, &matched.name))
                    .collect::<Vec<_>>())))
            })
        },
        {
            let tools = tools.to_vec();
            let samples = samples.clone();
            global("describeTool", move |args| {
                let args = args
                    .and_then(|value| value.as_array().cloned())
                    .unwrap_or_default();
                let Value::String(name) = args.first().cloned().unwrap_or(Value::Null) else {
                    return Err("describeTool() expects a tool name".to_string());
                };
                let tool = tools.iter().find(|candidate| {
                    candidate.name == name || to_codemode_identifier(&candidate.name) == name
                });
                Ok(tool
                    .and_then(|tool| samples.get(&tool.name).cloned())
                    .map(Value::String))
            })
        },
        {
            let tools = tools.to_vec();
            let options = options.clone();
            global("describeNamespace", move |args| {
                let args = args
                    .and_then(|value| value.as_array().cloned())
                    .unwrap_or_default();
                let Value::String(name) = args.first().cloned().unwrap_or(Value::Null) else {
                    return Err("describeNamespace() expects a namespace name".to_string());
                };
                let mut namespace: Option<ToolNamespace> = None;
                let mut names: Vec<String> = Vec::new();
                for tool in &tools {
                    let tool_namespace = options
                        .get_tool_namespace
                        .as_ref()
                        .and_then(|get| get(&tool.name));
                    let Some(tool_namespace) = tool_namespace else {
                        continue;
                    };
                    if tool_namespace.name != name {
                        continue;
                    }
                    if namespace.is_none() {
                        namespace = Some(tool_namespace);
                    }
                    names.push(to_codemode_identifier(&tool.name));
                }
                let Some(namespace) = namespace else {
                    return Ok(None);
                };
                let mut result = serde_json::Map::new();
                result.insert("name".to_string(), json!(name));
                if let Some(description) = &namespace.description {
                    result.insert("description".to_string(), json!(description));
                }
                if let Some(instructions) = &namespace.instructions {
                    result.insert("instructions".to_string(), json!(instructions));
                }
                result.insert("tools".to_string(), json!(names));
                Ok(Some(Value::Object(result)))
            })
        },
    ]
}

/// Upstream `createModelGlobals`: `models.*` for scripts — the model registry
/// methods declared in `MODEL_GLOBAL_DECLARATIONS`. Classifier calls appear
/// as nested call rows so the renderer shows them, and their usage goes to
/// `addUsage`. Runs at most `MAX_CONCURRENT_MODEL_CALLS` classifiers at once.
fn create_model_globals(
    models: Arc<dyn CodemodeModelRuntime>,
    tool_call_id: &str,
    calls: Arc<Mutex<Vec<CodemodeNestedCall>>>,
    publish: impl Fn() + Send + Sync + 'static,
    add_usage: Arc<dyn Fn(crate::ai::types::primitives::Usage) + Send + Sync>,
) -> Vec<CodemodeTool> {
    // Upstream `createLimiter(MAX_CONCURRENT_MODEL_CALLS)`.
    let limiter = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_MODEL_CALLS));
    let classify_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let publish = Arc::new(publish);
    let tool_call_id = tool_call_id.to_string();

    fn spread_global<F>(name: &str, execute: F) -> CodemodeTool
    where
        F: Fn(
                Option<Value>,
                Arc<crate::coding_agent::extensions::types::AbortSignal>,
            ) -> futures::future::BoxFuture<'static, Result<Option<Value>, String>>
            + Clone
            + Send
            + Sync
            + 'static,
    {
        CodemodeTool {
            name: name.to_string(),
            description: None,
            input_schema: None,
            output_schema: None,
            spread: true,
            signature: None,
            execute: Arc::new(move |args, context| {
                let signal = Arc::clone(&context.signal);
                // One clone per sandbox call (the upstream implementations are
                // fresh closures per sandbox construction).
                let execute = execute.clone();
                Box::pin(async move { execute(args, signal).await })
            }),
        }
    }

    vec![
        {
            let models = Arc::clone(&models);
            spread_global("models.getModelsOfType", move |args, _signal| {
                let models = Arc::clone(&models);
                Box::pin(async move {
                    let args = args
                        .and_then(|value| value.as_array().cloned())
                        .unwrap_or_default();
                    let model_type = to_model_type(args.first().unwrap_or(&Value::Null))?;
                    let provider = to_provider(args.get(1).unwrap_or(&Value::Null))?;
                    let entries = models.get_models_of_type(&model_type, provider.as_deref())?;
                    Ok(Some(json!(entries
                        .iter()
                        .map(to_model_info)
                        .collect::<Vec<_>>())))
                })
            })
        },
        {
            let models = Arc::clone(&models);
            spread_global("models.getAvailableOfType", move |args, signal| {
                let models = Arc::clone(&models);
                Box::pin(async move {
                    let args = args
                        .and_then(|value| value.as_array().cloned())
                        .unwrap_or_default();
                    let model_type = to_model_type(args.first().unwrap_or(&Value::Null))?;
                    let provider = to_provider(args.get(1).unwrap_or(&Value::Null))?;
                    let available = models
                        .get_available_of_type(&model_type, provider.as_deref(), Some(signal))
                        .await?;
                    Ok(Some(json!(available
                        .iter()
                        .map(to_model_info)
                        .collect::<Vec<_>>())))
                })
            })
        },
        {
            let models = Arc::clone(&models);
            spread_global("models.getModelOfType", move |args, _signal| {
                let models = Arc::clone(&models);
                Box::pin(async move {
                    let args = args
                        .and_then(|value| value.as_array().cloned())
                        .unwrap_or_default();
                    let model_type = to_model_type(args.first().unwrap_or(&Value::Null))?;
                    let (Some(provider), Some(id)) = (
                        args.get(1).and_then(Value::as_str),
                        args.get(2).and_then(Value::as_str),
                    ) else {
                        return Err(
                            "models.getModelOfType() expects a type, a provider, and an id"
                                .to_string(),
                        );
                    };
                    let model = models.get_model_of_type(&model_type, provider, id)?;
                    Ok(Some(match model {
                        Some(model) => to_model_info(&model),
                        None => Value::Null,
                    }))
                })
            })
        },
        {
            let models = Arc::clone(&models);
            let calls = Arc::clone(&calls);
            let publish = Arc::clone(&publish);
            let add_usage = Arc::clone(&add_usage);
            let tool_call_id = tool_call_id.clone();
            let limiter = Arc::clone(&limiter);
            let classify_count = Arc::clone(&classify_count);
            spread_global("models.classify", move |args, signal| {
                let models = Arc::clone(&models);
                let calls = Arc::clone(&calls);
                let publish = Arc::clone(&publish);
                let add_usage = Arc::clone(&add_usage);
                let tool_call_id = tool_call_id.clone();
                let limiter = Arc::clone(&limiter);
                let classify_count = Arc::clone(&classify_count);
                Box::pin(async move {
                    let args = args
                        .and_then(|value| value.as_array().cloned())
                        .unwrap_or_default();
                    let context = args.get(1).cloned().unwrap_or(Value::Null);
                    let reference = args.first().cloned().unwrap_or(Value::Null);
                    let (provider, id) = match reference.as_object() {
                        Some(object) => (
                            object.get("provider").and_then(Value::as_str),
                            object.get("id").and_then(Value::as_str),
                        ),
                        None => (None, None),
                    };
                    let (Some(provider), Some(id)) = (provider, id) else {
                        return Err("models.classify() expects a model from models.getModelOfType() or models.getAvailableOfType()".to_string());
                    };
                    // Only provider and id count. A script-supplied baseUrl or
                    // headers must never receive the credentials.
                    let resolved = models
                        .get_model_of_type("classifier", provider, id)?
                        .ok_or_else(|| format!("Unknown classifier model \"{provider}/{id}\""))?;

                    let index =
                        classify_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    let record_id = format!("{tool_call_id}/models.classify/{index}");
                    let model_label = format!(
                        "{}/{}",
                        resolved
                            .get("provider")
                            .and_then(Value::as_str)
                            .unwrap_or(provider),
                        resolved.get("id").and_then(Value::as_str).unwrap_or(id)
                    );
                    {
                        let mut locked = calls
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        locked.push(CodemodeNestedCall {
                            id: record_id.clone(),
                            name: "models.classify".to_string(),
                            args: model_label,
                            status: CodemodeNestedCallStatus::Running,
                            duration_ms: None,
                            error: None,
                            cost: None,
                        });
                    }
                    publish();
                    let started_at = std::time::Instant::now();
                    let _permit = limiter
                        .acquire()
                        .await
                        .map_err(|_| "classifier limiter closed".to_string())?;
                    let result = models.classify(&reference, &context, Some(signal)).await?;
                    let duration = started_at.elapsed().as_secs_f64() * 1000.0;
                    let stop_reason = result
                        .get("stopReason")
                        .and_then(Value::as_str)
                        .unwrap_or("error");
                    {
                        let mut locked = calls
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if let Some(record) =
                            locked.iter_mut().find(|record| record.id == record_id)
                        {
                            record.duration_ms = Some(duration);
                            record.status = match stop_reason {
                                "stop" => CodemodeNestedCallStatus::Ok,
                                "aborted" => CodemodeNestedCallStatus::Cancelled,
                                _ => CodemodeNestedCallStatus::Error,
                            };
                            if let Some(message) =
                                result.get("errorMessage").and_then(Value::as_str)
                            {
                                record.error = Some(truncate_text(message, ERROR_PREVIEW_CHARS));
                            }
                            if let Some(cost) = result
                                .get("usage")
                                .and_then(|usage| usage.get("cost"))
                                .and_then(|cost| cost.get("total"))
                                .and_then(Value::as_f64)
                            {
                                record.cost = Some(cost);
                            }
                        }
                    }
                    if let Some(usage_value) = result.get("usage").cloned() {
                        if let Ok(usage) = serde_json::from_value::<
                            crate::ai::types::primitives::Usage,
                        >(usage_value)
                        {
                            add_usage(usage);
                        }
                    }
                    publish();
                    Ok(Some(result))
                })
            })
        },
    ]
}

/// The `executeAsync` handler for the tool definition (upstream's lazy
/// executor binding).
pub(crate) fn codemode_execute_handler(
    options: CodemodeToolOptions,
) -> crate::coding_agent::extensions::types::AsyncToolExecuteHandler {
    let options = Arc::new(options);
    Arc::new(
        move |tool_call_id: String,
              params: Value,
              signal: Option<Arc<crate::coding_agent::extensions::types::AbortSignal>>,
              on_update: Option<AgentToolUpdateCallbackValue>,
              ctx: ExtensionContext| {
            let options = Arc::clone(&options);
            Box::pin(async move {
                execute_codemode(
                    &tool_call_id,
                    &params,
                    signal,
                    on_update,
                    &ctx,
                    Arc::unwrap_or_clone(options),
                )
                .await
            })
        },
    )
}
