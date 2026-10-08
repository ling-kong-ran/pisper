//! Upstream edit.ts plus the pure matching/diff shared with harness/tools.
use super::{check_abort, context_cwd, definition, text_result, ToolFuture};
use super::{file_mutation_queue::with_file_mutation_queue, path_utils::resolve_to_cwd};
pub use crate::agent_core::harness::tools::edit_diff::{
    apply_edits_to_normalized_content, detect_line_ending, generate_diff_string,
    generate_unified_patch, normalize_to_lf, restore_line_endings, strip_bom, Edit,
};
pub use crate::agent_core::harness::tools::prepare_edit_arguments;
use crate::coding_agent::extensions::types::{AbortSignal, ToolDefinition};
use serde_json::{json, Value};
use std::sync::Arc;
#[derive(Clone)]
pub struct EditOperations {
    pub read_file: Arc<dyn Fn(String) -> ToolFuture<Vec<u8>> + Send + Sync>,
    pub write_file: Arc<dyn Fn(String, String) -> ToolFuture<()> + Send + Sync>,
    /// On failure return Node's error-code payload (for example ENOENT).
    pub access: Arc<dyn Fn(String) -> ToolFuture<()> + Send + Sync>,
}
impl Default for EditOperations {
    fn default() -> Self {
        Self {
            read_file: super::read::ReadOperations::default().read_file,
            write_file: super::write::WriteOperations::default().write_file,
            access: Arc::new(|p| {
                Box::pin(async move {
                    tokio::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&p)
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("Error code: {}", super::io_code(&e)))
                })
            }),
        }
    }
}
#[derive(Clone, Default)]
pub struct EditToolOptions {
    pub operations: Option<EditOperations>,
}
pub fn create_edit_tool_definition(cwd: &str, options: EditToolOptions) -> Arc<ToolDefinition> {
    let mut def=definition("edit","Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.","Make precise file edits with exact text replacement, including multiple disjoint edits in one call",&[
        "Use edit for precise changes (edits[].oldText must match exactly)",
        "When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls",
        "Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.",
        "Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with large unchanged regions.",
    ],json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the file to edit (relative or absolute)"},"edits":{"type":"array","items":{"type":"object","properties":{"oldText":{"type":"string","description":"Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."},"newText":{"type":"string","description":"Replacement text for this targeted edit."}},"required":["oldText","newText"]},"description":"One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead."}},"required":["path","edits"]}),true);
    def.render_shell = Some("self".into());
    def.prepare_arguments = Some(Arc::new(|args| prepare_edit_arguments(args.clone())));
    let cwd = cwd.to_owned();
    let ops = options.operations.unwrap_or_default();
    def.execute_async = Some(Arc::new(move |_, args, signal, _, ctx| {
        let cwd = cwd.clone();
        let ops = ops.clone();
        Box::pin(async move {
            let edits = args
                .get("edits")
                .and_then(Value::as_array)
                .filter(|e| !e.is_empty())
                .ok_or(
                    "Edit tool input is invalid. edits must contain at least one replacement.",
                )?;
            let edits: Vec<Edit> =
                serde_json::from_value(json!(edits)).map_err(|e| e.to_string())?;
            let path = args["path"].as_str().ok_or("path must be a string")?;
            execute_edit(
                path,
                &edits,
                &context_cwd(&ctx, &cwd)?,
                &ops,
                signal.as_ref(),
            )
            .await
        })
    }));
    Arc::new(def)
}
pub async fn execute_edit(
    path: &str,
    edits: &[Edit],
    cwd: &str,
    ops: &EditOperations,
    signal: Option<&Arc<AbortSignal>>,
) -> Result<Value, String> {
    if edits.is_empty() {
        return Err(
            "Edit tool input is invalid. edits must contain at least one replacement.".into(),
        );
    }
    let absolute = resolve_to_cwd(path, cwd)?;
    with_file_mutation_queue(&absolute, || async {
        check_abort(signal)?;
        if let Err(e) = (ops.access)(absolute.clone()).await {
            check_abort(signal)?;
            return Err(format!("Could not edit file: {path}. {e}."));
        }
        check_abort(signal)?;
        let bytes = (ops.read_file)(absolute.clone()).await?;
        let raw = String::from_utf8_lossy(&bytes);
        check_abort(signal)?;
        let (bom, text) = strip_bom(&raw);
        let ending = detect_line_ending(text);
        let normalized = normalize_to_lf(text);
        let changed = apply_edits_to_normalized_content(&normalized, edits, path)
            .map_err(|e| e.to_string())?;
        check_abort(signal)?;
        let final_content = format!(
            "{bom}{}",
            restore_line_endings(&changed.new_content, ending)
        );
        (ops.write_file)(absolute.clone(), final_content).await?;
        check_abort(signal)?;
        let diff = generate_diff_string(&changed.base_content, &changed.new_content, 4);
        let patch = generate_unified_patch(path, &changed.base_content, &changed.new_content, 4);
        let mut result = text_result(format!(
            "Successfully replaced {} block(s) in {path}.",
            edits.len()
        ));
        result["details"] = json!({"diff":diff.diff,"patch":patch});
        if let Some(line) = diff.first_changed_line {
            result["details"]["firstChangedLine"] = json!(line);
        }
        Ok(result)
    })
    .await
}
