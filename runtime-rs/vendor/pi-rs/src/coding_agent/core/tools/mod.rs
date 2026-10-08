//! Coding-agent built-ins, distinct from the harness ExecutionEnv tools.
//! Native filesystem operations, extension context, prompt contributions and
//! constrained sampling follow packages/coding-agent/src/core/tools/.
//! Pure edit matching/diff and truncation are shared with the already-ported
//! harness algorithms and checked against the coding-agent source by oracles.
pub mod edit;
pub mod file_mutation_queue;
pub mod output_accumulator;
pub mod path_utils;
pub mod read;
pub mod truncate;
pub mod write;

use crate::coding_agent::extensions::types::{AbortSignal, ExtensionContext, ToolDefinition};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::Arc;

pub type ToolFuture<T> = BoxFuture<'static, Result<T, String>>;
pub fn text_result(text: impl Into<String>) -> Value {
    json!({"content":[{"type":"text","text":text.into()}]})
}
pub fn check_abort(signal: Option<&Arc<AbortSignal>>) -> Result<(), String> {
    if signal.is_some_and(|s| s.is_aborted()) {
        Err("Operation aborted".into())
    } else {
        Ok(())
    }
}
pub async fn cancellable<T>(
    signal: Option<&Arc<AbortSignal>>,
    future: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    check_abort(signal)?;
    match signal {
        Some(signal) => {
            tokio::select! { biased; _ = signal.cancelled() => Err("Operation aborted".into()), result = future => result }
        }
        None => future.await,
    }
}
pub fn context_cwd(ctx: &ExtensionContext, fallback: &str) -> Result<String, String> {
    let cwd = ctx.cwd()?;
    Ok(if cwd.is_empty() {
        fallback.to_owned()
    } else {
        cwd
    })
}
pub(crate) fn definition(
    name: &str,
    description: &str,
    snippet: &str,
    guidelines: &[&str],
    parameters: Value,
    strict: bool,
) -> ToolDefinition {
    let mut def = ToolDefinition::new(name, name, description, parameters);
    def.prompt_snippet = Some(snippet.into());
    if !guidelines.is_empty() {
        def.prompt_guidelines = Some(guidelines.iter().map(|s| (*s).into()).collect());
    }
    if strict {
        def.constrained_sampling = Some(json!({"type":"json_schema","strict":"prefer"}));
    }
    def
}
pub(crate) fn io_code(error: &std::io::Error) -> &'static str {
    use std::io::ErrorKind::*;
    match error.kind() {
        NotFound => "ENOENT",
        PermissionDenied => "EACCES",
        AlreadyExists => "EEXIST",
        NotADirectory => "ENOTDIR",
        IsADirectory => "EISDIR",
        _ => "EIO",
    }
}
pub(crate) fn io_error(error: std::io::Error, operation: &str, path: &str) -> String {
    let reason = match error.kind() {
        std::io::ErrorKind::NotFound => "no such file or directory".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        std::io::ErrorKind::NotADirectory => "not a directory".to_string(),
        std::io::ErrorKind::IsADirectory => "illegal operation on a directory".to_string(),
        _ => error.to_string(),
    };
    format!("{}: {reason}, {operation} '{path}'", io_code(&error))
}

#[cfg(test)]
mod tests;

pub mod bash;

pub mod bash_process;

pub mod powershell;

pub mod find;
pub mod grep;
pub mod search_process;
#[cfg(test)]
mod search_tools_tests;

pub mod ls;
