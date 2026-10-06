//! Port of `src/tools/edit.ts`: edit a single file using exact text
//! replacement. Every `edits[].oldText` must match a unique, non-overlapping
//! region of the original file. `prepareArguments` repairs the argument
//! shapes models commonly send.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::agent_core::chord_support::context::Context;
use crate::ai::types::{TextContent, TextOrImageBlock, Tool as AiTool};
use crate::durable::env::{FileContent, FileError, FileKind};
use crate::durable::errors::PlainError;
use crate::durable::harness::types::{
    PrepareArgumentsFn, ToolExecutionApiLike, ToolExecutionResult, ToolRegistration,
};
use crate::durable::tools::edit_diff::{
    apply_edits_to_normalized_content, detect_line_ending, generate_diff_string,
    generate_unified_patch, normalize_to_lf, restore_line_endings, strip_bom, Edit,
};
use crate::durable::tools::env::require_env;
use crate::durable::tools::file_mutation_queue::with_file_mutation_queue;
use crate::durable::tools::path_utils::resolve_tool_path;

/// `EditToolInput` (`tools/edit.ts`); validated upstream by the tool
/// declaration after `prepareArguments` repairs it (D32).
#[derive(Debug, Clone, Deserialize)]
pub struct EditToolInput {
    pub path: String,
    pub edits: Vec<Edit>,
}

/// `EditToolDetails` (`tools/edit.ts`). Key order on the wire is
/// `{diff, patch, firstChangedLine?}`.
#[derive(Debug, Clone, PartialEq)]
pub struct EditToolDetails {
    pub diff: String,
    pub patch: String,
    pub first_changed_line: Option<usize>,
}

impl EditToolDetails {
    /// The JSON wire form.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert(String::from("diff"), json!(self.diff));
        map.insert(String::from("patch"), json!(self.patch));
        if let Some(first_changed_line) = self.first_changed_line {
            map.insert(String::from("firstChangedLine"), json!(first_changed_line));
        }
        Value::Object(map)
    }
}

fn is_single_edit_input(value: &Value) -> bool {
    value.as_object().is_some_and(|edit| {
        edit.get("oldText").is_some_and(Value::is_string)
            && edit.get("newText").is_some_and(Value::is_string)
    })
}

/// `prepareEditArguments(input)` (`tools/edit.ts`): repair `edits` as a JSON
/// string or as a single edit object, and a top-level `oldText`/`newText`
/// pair. Works on a copy; the call's arguments stay unchanged.
pub fn prepare_edit_arguments(input: &Map<String, Value>) -> Map<String, Value> {
    let mut args = input.clone();
    if let Some(edits) = args.get("edits") {
        if let Some(text) = edits.as_str() {
            let parsed: Result<Value, _> = serde_json::from_str(text);
            if let Ok(parsed) = parsed {
                if parsed.is_array() {
                    args.insert(String::from("edits"), parsed);
                } else if is_single_edit_input(&parsed) {
                    args.insert(String::from("edits"), Value::Array(vec![parsed]));
                }
            }
        } else if is_single_edit_input(edits) {
            args.insert(String::from("edits"), Value::Array(vec![edits.clone()]));
        }
    }

    let legacy_old = args.get("oldText").and_then(Value::as_str);
    let legacy_new = args.get("newText").and_then(Value::as_str);
    let (Some(legacy_old), Some(legacy_new)) = (legacy_old, legacy_new) else {
        return args;
    };
    let mut edits = match args.get("edits") {
        Some(Value::Array(existing)) => existing.clone(),
        _ => Vec::new(),
    };
    edits.push(json!({ "oldText": legacy_old, "newText": legacy_new }));
    // The spread drops `oldText`/`newText` while keeping every other key's
    // position, then assigns `edits` (overwriting in place when present).
    let keys: Vec<String> = args
        .keys()
        .filter(|key| key.as_str() != "oldText" && key.as_str() != "newText")
        .cloned()
        .collect();
    let mut rest = Map::new();
    for key in keys {
        rest.insert(key.clone(), args[&key].clone());
    }
    rest.insert(String::from("edits"), Value::Array(edits));
    rest
}

/// `validateEditInput(input)` (`tools/edit.ts`).
fn validate_edit_input(input: &EditToolInput) -> Result<(&str, &[Edit]), PlainError> {
    if input.edits.is_empty() {
        return Err(PlainError::new(
            "Edit tool input is invalid. edits must contain at least one replacement.",
        ));
    }
    Ok((&input.path, &input.edits))
}

/// `editAccessError(path, error)` (`tools/edit.ts`).
fn edit_access_error(path: &str, error: &FileError) -> PlainError {
    PlainError::new(format!(
        "Could not edit file: {path}. Error code: {}.",
        error.code.as_str()
    ))
}

/// `createEditTool()` (`tools/edit.ts`).
pub fn create_edit_tool() -> ToolRegistration {
    ToolRegistration {
        tool: AiTool {
            name: String::from("edit"),
            description: String::from(
                "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.",
            ),
            parameters: json!({
                "type": "object",
                "required": ["path", "edits"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file to edit (relative or absolute)"},
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["oldText", "newText"],
                            "properties": {
                                "oldText": {"type": "string", "description": "Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."},
                                "newText": {"type": "string", "description": "Replacement text for this targeted edit."},
                            },
                        },
                        "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.",
                    },
                },
            }),
            constrained_sampling: None,
        },
        replay: None,
        execution_mode: None,
        prepare_arguments: Some(Arc::new(prepare_edit_arguments) as PrepareArgumentsFn),
        output_limits: None,
        execute: Arc::new(
            |args: serde_json::Map<String, serde_json::Value>,
             api: Arc<dyn ToolExecutionApiLike>,
             context: Context| {
                Box::pin(async move {
                    let input: EditToolInput = serde_json::from_value(Value::Object(args))
                        .map_err(|error| PlainError::new(error.to_string()))?;
                    let (path, edits) = validate_edit_input(&input)?;
                    let (path, edits) = (path.to_string(), edits.to_vec());
                    let env = require_env(api.as_ref())?;
                    let absolute_path = resolve_tool_path(env.as_ref(), &path, &context)?;
                    with_file_mutation_queue(&env, &absolute_path, || async {
                        aborted(&context)?;
                        let info = env
                            .file_info(&absolute_path, &context)
                            .map_err(|error| edit_access_error(&path, &error))?;
                        if info.kind != FileKind::File && info.kind != FileKind::Symlink {
                            return Err(PlainError::new(format!(
                                "Could not edit file: {path}. Path is not a file."
                            )));
                        }

                        let content = env
                            .read_text_file(&absolute_path, &context)
                            .map_err(|error| edit_access_error(&path, &error))?;
                        aborted(&context)?;

                        let (bom, text) = strip_bom(&content);
                        let original_ending = detect_line_ending(text);
                        let normalized_content = normalize_to_lf(text);
                        let applied = apply_edits_to_normalized_content(
                            &normalized_content,
                            &edits,
                            &path,
                        )?;
                        aborted(&context)?;

                        let final_content =
                            format!("{bom}{}", restore_line_endings(&applied.new_content, original_ending));
                        env.write_file(&absolute_path, FileContent::Text(&final_content), &context)
                            .map_err(|error| edit_access_error(&path, &error))?;
                        aborted(&context)?;

                        let diff_result = generate_diff_string(
                            &applied.base_content,
                            &applied.new_content,
                            4,
                        );
                        let details = EditToolDetails {
                            diff: diff_result.diff,
                            patch: generate_unified_patch(
                                &path,
                                &applied.base_content,
                                &applied.new_content,
                                4,
                            ),
                            first_changed_line: diff_result.first_changed_line,
                        };
                        Ok(ToolExecutionResult {
                            content: Some(vec![TextOrImageBlock::Text(TextContent {
                                text: format!(
                                    "Successfully replaced {} block(s) in {path}.",
                                    edits.len()
                                ),
                                text_signature: None,
                            })]),
                            is_error: None,
                            details: Some(details.to_json()),
                            diagnostics: None,
                            usage: None,
                            control: None,
                        })
                    }, &context)
                    .await
                })
            },
        ),
    }
}

/// The repeated `if (context.abortSignal?.aborted) throw new Error("Operation aborted")`.
fn aborted(context: &Context) -> Result<(), PlainError> {
    if context
        .abort_signal()
        .is_some_and(|signal| signal.is_cancelled())
    {
        return Err(PlainError::new("Operation aborted"));
    }
    Ok(())
}
