//! Native and injectable directory listing from core/tools/ls.ts. TUI
//! renderers are a separate migration surface; execution is wired into CLI.
use super::path_utils::{path_exists, resolve_to_cwd};
use super::truncate::{as_value, format_size, truncate_head, TruncationOptions, DEFAULT_MAX_BYTES};
use super::{cancellable, context_cwd, definition, io_error, text_result, ToolFuture};
use crate::coding_agent::{
    core::path_join,
    extensions::types::{AbortSignal, ToolDefinition},
    utils::locale::{default_sort_locale, LocaleComparator},
};
use crate::serde_support::js_number_string;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub type LsExists = Arc<dyn Fn(String) -> ToolFuture<bool> + Send + Sync>;
pub type LsStat = Arc<dyn Fn(String) -> ToolFuture<bool> + Send + Sync>;
pub type LsReaddir = Arc<dyn Fn(String) -> ToolFuture<Vec<String>> + Send + Sync>;
#[derive(Clone)]
pub struct LsOperations {
    pub exists: LsExists,
    /// True for directories; follows symbolic links just like fs.stat.
    pub stat: LsStat,
    pub readdir: LsReaddir,
}
impl Default for LsOperations {
    fn default() -> Self {
        Self {
            exists: Arc::new(|path| Box::pin(async move { Ok(path_exists(&path).await) })),
            stat: Arc::new(|path| {
                Box::pin(async move {
                    tokio::fs::metadata(&path)
                        .await
                        .map(|meta| meta.is_dir())
                        .map_err(|e| io_error(e, "stat", &path))
                })
            }),
            readdir: Arc::new(|path| {
                Box::pin(async move {
                    let mut dir = tokio::fs::read_dir(&path)
                        .await
                        .map_err(|e| io_error(e, "scandir", &path))?;
                    let mut entries = vec![];
                    while let Some(entry) = dir
                        .next_entry()
                        .await
                        .map_err(|e| io_error(e, "scandir", &path))?
                    {
                        entries.push(entry.file_name().to_string_lossy().into_owned());
                    }
                    Ok(entries)
                })
            }),
        }
    }
}
#[derive(Clone, Default)]
pub struct LsToolOptions {
    pub operations: Option<LsOperations>,
    /// Native host seam for an embedding with a custom Intl default locale.
    /// None uses the cached regional/process default, not the UI language.
    pub locale: Option<String>,
}
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LsToolInput {
    pub path: Option<String>,
    pub limit: Option<f64>,
}
pub fn create_ls_tool_definition(cwd: &str, options: LsToolOptions) -> Arc<ToolDefinition> {
    let mut def = definition("ls", "List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. Includes dotfiles. Output is truncated to 500 entries or 50KB (whichever is hit first).", "List directory contents", &[], json!({"type":"object","properties":{"path":{"type":"string","description":"Directory to list (default: current directory)"},"limit":{"type":"number","description":"Maximum number of entries to return (default: 500)"}}}), false);
    let cwd = cwd.to_owned();
    def.execute_async = Some(Arc::new(move |_, params, signal, _, ctx| {
        let options = options.clone();
        let cwd = cwd.clone();
        Box::pin(async move {
            let input = serde_json::from_value(params).map_err(|e| e.to_string())?;
            execute_ls(&input, &context_cwd(&ctx, &cwd)?, &options, signal.as_ref()).await
        })
    }));
    Arc::new(def)
}
pub async fn execute_ls(
    input: &LsToolInput,
    cwd: &str,
    options: &LsToolOptions,
    signal: Option<&Arc<AbortSignal>>,
) -> Result<Value, String> {
    cancellable(signal, async {
        let root = resolve_to_cwd(
            input
                .path
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("."),
            cwd,
        )?;
        let limit = input.limit.unwrap_or(500.0);
        let ops = options.operations.clone().unwrap_or_default();
        if !(ops.exists)(root.clone()).await? {
            return Err(format!("Path not found: {root}"));
        }
        if !(ops.stat)(root.clone()).await? {
            return Err(format!("Not a directory: {root}"));
        }
        let entries = (ops.readdir)(root.clone())
            .await
            .map_err(|e| format!("Cannot read directory: {e}"))?;
        let comparator = LocaleComparator::new(
            options
                .locale
                .as_deref()
                .unwrap_or_else(|| default_sort_locale()),
        )?;
        let entries = comparator.sort_case_insensitive(entries);
        let mut results = vec![];
        let mut limit_reached = false;
        for entry in entries {
            if results.len() as f64 >= limit {
                limit_reached = true;
                break;
            }
            let Ok(is_directory) = (ops.stat)(path_join(&root, &entry)).await else {
                continue;
            };
            results.push(if is_directory {
                format!("{entry}/")
            } else {
                entry
            });
        }
        if results.is_empty() {
            return Ok(text_result("(empty directory)"));
        }
        let truncation = truncate_head(
            &results.join("\n"),
            TruncationOptions {
                max_lines: Some(9_007_199_254_740_991),
                max_bytes: None,
            },
        );
        let mut output = truncation.content.clone();
        let mut details = json!({});
        let mut notices = vec![];
        if limit_reached {
            notices.push(format!(
                "{} entries limit reached. Use limit={} for more",
                js_number_string(limit),
                js_number_string(limit * 2.0)
            ));
            details["entryLimitReached"] =
                serde_json::from_str(&js_number_string(limit)).unwrap_or(Value::Null);
        }
        if truncation.truncated {
            notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
            details["truncation"] = as_value(&truncation);
        }
        if !notices.is_empty() {
            output.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }
        let mut result = text_result(output);
        if !details.as_object().unwrap().is_empty() {
            result["details"] = details;
        }
        Ok(result)
    })
    .await
}
#[cfg(test)]
#[path = "ls_tests.rs"]
mod tests;
