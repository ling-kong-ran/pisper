//! Upstream write.ts; filesystem effects settle before releasing the mutation queue.
use super::{check_abort, context_cwd, definition, text_result, ToolFuture};
use super::{file_mutation_queue::with_file_mutation_queue, path_utils::resolve_to_cwd};
use crate::coding_agent::extensions::types::{AbortSignal, ToolDefinition};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};
#[derive(Clone)]
pub struct WriteOperations {
    pub write_file: Arc<dyn Fn(String, String) -> ToolFuture<()> + Send + Sync>,
    pub mkdir: Arc<dyn Fn(String) -> ToolFuture<()> + Send + Sync>,
}
impl Default for WriteOperations {
    fn default() -> Self {
        Self {
            write_file: Arc::new(|p, content| {
                Box::pin(async move {
                    tokio::fs::write(&p, content)
                        .await
                        .map_err(|e| super::io_error(e, "open", &p))
                })
            }),
            mkdir: Arc::new(|p| {
                Box::pin(async move {
                    tokio::fs::create_dir_all(&p)
                        .await
                        .map_err(|e| super::io_error(e, "mkdir", &p))
                })
            }),
        }
    }
}
#[derive(Clone, Default)]
pub struct WriteToolOptions {
    pub operations: Option<WriteOperations>,
}
pub fn create_write_tool_definition(cwd: &str, options: WriteToolOptions) -> Arc<ToolDefinition> {
    let mut def=definition("write","Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.","Create or overwrite files",&["Use write only for new files or complete rewrites."],json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the file to write (relative or absolute)"},"content":{"type":"string","description":"Content to write to the file"}},"required":["path","content"]}),true);
    let cwd = cwd.to_owned();
    let ops = options.operations.unwrap_or_default();
    def.execute_async = Some(Arc::new(move |_, args, signal, _, ctx| {
        let cwd = cwd.clone();
        let ops = ops.clone();
        Box::pin(async move {
            let path = args["path"].as_str().ok_or("path must be a string")?;
            let content = args["content"].as_str().ok_or("content must be a string")?;
            execute_write(
                path,
                content,
                &context_cwd(&ctx, &cwd)?,
                &ops,
                signal.as_ref(),
            )
            .await
        })
    }));
    Arc::new(def)
}
pub async fn execute_write(
    path: &str,
    content: &str,
    cwd: &str,
    ops: &WriteOperations,
    signal: Option<&Arc<AbortSignal>>,
) -> Result<Value, String> {
    let absolute = resolve_to_cwd(path, cwd)?;
    with_file_mutation_queue(&absolute, || async {
        check_abort(signal)?;
        let dir = Path::new(&absolute)
            .parent()
            .ok_or("path has no parent")?
            .to_string_lossy()
            .into_owned();
        (ops.mkdir)(dir).await?;
        check_abort(signal)?;
        (ops.write_file)(absolute.clone(), content.to_owned()).await?;
        check_abort(signal)?;
        Ok(text_result(format!("Successfully wrote to {path}")))
    })
    .await
}
