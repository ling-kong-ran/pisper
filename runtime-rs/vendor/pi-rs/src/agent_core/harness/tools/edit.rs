//! Upstream tools/edit.ts.
use super::super::{AgentHarnessTool, FileContent, FileError, FileKind};
use super::edit_diff::*;
use super::file_mutation_queue::with_file_mutation_queue;
use super::path_utils::resolve_tool_path;
use super::{check_abort, text_result, ExecutionToolContext, HasExecutionEnv};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditToolInput {
    pub path: String,
    pub edits: Vec<Edit>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditToolDetails {
    pub diff: String,
    pub patch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_changed_line: Option<usize>,
}
fn is_edit(value: &Value) -> bool {
    value.is_object()
        && value.get("oldText").is_some_and(Value::is_string)
        && value.get("newText").is_some_and(Value::is_string)
}
pub fn prepare_edit_arguments(mut input: Value) -> Value {
    let Some(args) = input.as_object_mut() else {
        return input;
    };
    if let Some(Value::String(text)) = args.get("edits") {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            if parsed.is_array() {
                args.insert("edits".into(), parsed);
            } else if is_edit(&parsed) {
                args.insert("edits".into(), json!([parsed]));
            }
        }
    } else if args.get("edits").is_some_and(is_edit) {
        let edit = args["edits"].clone();
        args.insert("edits".into(), json!([edit]));
    }
    if args.get("oldText").is_some_and(Value::is_string)
        && args.get("newText").is_some_and(Value::is_string)
    {
        let mut edits = args
            .get("edits")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let old = args.shift_remove("oldText").unwrap();
        let new = args.shift_remove("newText").unwrap();
        edits.push(json!({"oldText":old,"newText":new}));
        args.insert("edits".into(), Value::Array(edits));
    }
    input
}
fn access_error(path: &str, error: FileError) -> anyhow::Error {
    let code = serde_json::to_value(error.code).expect("file error code");
    let message = format!(
        "Could not edit file: {path}. Error code: {}.",
        code.as_str().unwrap()
    );
    anyhow::Error::new(error).context(message)
}
pub fn create_edit_tool() -> AgentHarnessTool<ExecutionToolContext> {
    create_edit_tool_for()
}
pub fn create_edit_tool_for<T: HasExecutionEnv>() -> AgentHarnessTool<T> {
    AgentHarnessTool {
        name:"edit".into(),label:"edit".into(),description:"Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.".into(),
        parameters:json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the file to edit (relative or absolute)"},"edits":{"type":"array","description":"One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.","items":{"type":"object","properties":{"oldText":{"type":"string","description":"Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."},"newText":{"type":"string","description":"Replacement text for this targeted edit."}},"required":["oldText","newText"]}}},"required":["path","edits"]}),
        constrained_sampling:None,prepare_arguments:Some(Arc::new(prepare_edit_arguments)),replay:None,execution_mode:None,
        execute:Arc::new(|_,input,_,tool_context:T,_,context|Box::pin(async move {
            anyhow::ensure!(input.get("edits").and_then(Value::as_array).is_some_and(|edits|!edits.is_empty()),"Edit tool input is invalid. edits must contain at least one replacement.");
            let input:EditToolInput=serde_json::from_value(input)?;
            let env=tool_context.execution_env();let absolute=resolve_tool_path(env.as_ref(),&input.path,context.clone()).await?;
            with_file_mutation_queue(&env,&absolute,||async {
                check_abort(&context)?;
                let info=env.file_info(&absolute,context.clone()).await.map_err(|error|access_error(&input.path,error))?;
                anyhow::ensure!(matches!(info.kind,FileKind::File|FileKind::Symlink),"Could not edit file: {}. Path is not a file.",input.path);
                let content=env.read_text_file(&absolute,context.clone()).await.map_err(|error|access_error(&input.path,error))?;
                check_abort(&context)?;
                let(bom,content)=strip_bom(&content);let ending=detect_line_ending(content);let normalized=normalize_to_lf(content);
                let applied=apply_edits_to_normalized_content(&normalized,&input.edits,&input.path)?;
                check_abort(&context)?;
                let final_content=format!("{bom}{}",restore_line_endings(&applied.new_content,ending));
                env.write_file(&absolute,FileContent::Text(final_content),context.clone()).await.map_err(|error|access_error(&input.path,error))?;
                check_abort(&context)?;
                let diff=generate_diff_string(&applied.base_content,&applied.new_content,4);
                let mut result=text_result(format!("Successfully replaced {} block(s) in {}.",input.edits.len(),input.path));
                result.details=Some(json!(EditToolDetails {diff:diff.diff,patch:generate_unified_patch(&input.path,&applied.base_content,&applied.new_content,4),first_changed_line:diff.first_changed_line}));
                Ok(result)
            },context.clone()).await
        })),
    }
}

#[cfg(test)]
mod json_order_tests {
    use super::*;

    #[test]
    fn json_order_legacy_arguments_use_rest_then_overwrite_edits_slot() {
        for (raw, expected) in [
            (
                r#"{"oldText":"old","z":1,"newText":"new","path":"a.txt","a":2}"#,
                r#"{"z":1,"path":"a.txt","a":2,"edits":[{"oldText":"old","newText":"new"}]}"#,
            ),
            (
                r#"{"edits":[{"oldText":"a","newText":"b"}],"oldText":"old","z":1,"newText":"new","path":"a.txt","a":2}"#,
                r#"{"edits":[{"oldText":"a","newText":"b"},{"oldText":"old","newText":"new"}],"z":1,"path":"a.txt","a":2}"#,
            ),
        ] {
            assert_eq!(
                prepare_edit_arguments(serde_json::from_str(raw).unwrap()).to_string(),
                expected
            );
        }
    }
}
