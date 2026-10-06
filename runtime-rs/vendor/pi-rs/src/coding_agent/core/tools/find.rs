//! Execution half of upstream find.ts. TUI renderers remain a separate slice.
use super::{cancellable, check_abort, context_cwd, definition, text_result, ToolFuture};
use super::{path_utils::resolve_to_cwd, search_process::*, truncate::*};
use crate::coding_agent::extensions::types::{AbortSignal, ToolDefinition};
use crate::coding_agent::utils::{node_path, text::trim_js_whitespace};
use crate::serde_support::js_number_string;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FindToolInput {
    pub pattern: String,
    pub path: Option<String>,
    pub limit: Option<f64>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct GlobOptions {
    pub ignore: Vec<String>,
    pub limit: f64,
}
pub type Exists = Arc<dyn Fn(String) -> ToolFuture<bool> + Send + Sync>;
pub type Glob = Arc<dyn Fn(String, String, GlobOptions) -> ToolFuture<Vec<String>> + Send + Sync>;
#[derive(Clone)]
pub struct FindOperations {
    pub exists: Exists,
    pub glob: Glob,
}
#[derive(Clone, Default)]
pub struct FindToolOptions {
    pub operations: Option<FindOperations>,
    pub ensure_tool: Option<EnsureSearchTool>,
    pub runner: Option<SearchRunner>,
    /// Native repository-ancestor check, separate from custom glob operations.
    pub path_exists: Option<Exists>,
}
pub fn relativize_find_result_path(result: &str, search: &str, windows: bool) -> String {
    let trailing = result.ends_with('/') || (windows && result.ends_with('\\'));
    let absolute = if windows {
        node_path::win32_is_absolute(result)
    } else {
        node_path::posix_is_absolute(result)
    };
    let mut relative = if absolute {
        if windows {
            node_path::win32_relative(search, result, search)
        } else {
            node_path::posix_relative(search, result, search)
        }
    } else {
        result.into()
    };
    if windows {
        relative = relative.replace('\\', "/");
    }
    if trailing && !relative.ends_with('/') {
        relative.push('/');
    }
    relative
}
pub fn build_fd_args(
    pattern: &str,
    path: &str,
    limit: f64,
    inside_git: bool,
    windows: bool,
) -> Vec<String> {
    let mut args = vec!["--glob".into(), "--color=never".into(), "--hidden".into()];
    if !inside_git {
        args.push("--no-require-git".into());
    }
    args.extend(["--max-results".into(), js_number_string(limit)]);
    let mut effective = pattern.to_owned();
    if pattern.contains('/') {
        args.push("--full-path".into());
        if !pattern.starts_with('/') && !pattern.starts_with("**/") && pattern != "**" {
            effective = format!("**/{pattern}");
        }
        if windows {
            effective = effective.replace('/', r"[/\\]");
        }
    }
    args.extend(["--".into(), effective, path.into()]);
    args
}
fn format_results(results: &[String], root: &str, limit: f64, custom: bool) -> Value {
    let paths = results
        .iter()
        .map(|s| relativize_find_result_path(s, root, cfg!(windows)))
        .collect::<Vec<_>>();
    let limit_reached = paths.len() as f64 >= limit;
    let truncation = truncate_head(
        &paths.join("\n"),
        TruncationOptions {
            max_lines: Some(9_007_199_254_740_991),
            max_bytes: None,
        },
    );
    let mut output = truncation.content.clone();
    let mut details = json!({});
    let mut notices = vec![];
    if limit_reached {
        let number = js_number_string(limit);
        notices.push(if custom {
            format!("{number} results limit reached")
        } else {
            format!(
                "{number} results limit reached. Use limit={} for more, or refine pattern",
                js_number_string(limit * 2.0)
            )
        });
        details["resultLimitReached"] =
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
    result
}
pub fn create_find_tool_definition(cwd: &str, options: FindToolOptions) -> Arc<ToolDefinition> {
    let mut def=definition("find","Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to 1000 results or 50KB (whichever is hit first).","Find files by glob pattern (respects .gitignore)",&[],json!({"type":"object","properties":{"pattern":{"type":"string","description":"Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'"},"path":{"type":"string","description":"Directory to search in (default: current directory)"},"limit":{"type":"number","description":"Maximum number of results (default: 1000)"}},"required":["pattern"]}),false);
    let cwd = cwd.to_owned();
    def.execute_async = Some(Arc::new(move |_, params, signal, _, ctx| {
        let options = options.clone();
        let cwd = cwd.clone();
        Box::pin(async move {
            let input = serde_json::from_value(params).map_err(|e| e.to_string())?;
            execute_find(&input, &context_cwd(&ctx, &cwd)?, &options, signal.as_ref()).await
        })
    }));
    Arc::new(def)
}
pub async fn execute_find(
    input: &FindToolInput,
    cwd: &str,
    options: &FindToolOptions,
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
        let limit = input.limit.unwrap_or(1000.0);
        if let Some(ops) = &options.operations {
            if !(ops.exists)(root.clone()).await? {
                return Err(format!("Path not found: {root}"));
            }
            check_abort(signal)?;
            let results = (ops.glob)(
                input.pattern.clone(),
                root.clone(),
                GlobOptions {
                    ignore: vec!["**/node_modules/**".into(), "**/.git/**".into()],
                    limit,
                },
            )
            .await?;
            check_abort(signal)?;
            return Ok(if results.is_empty() {
                text_result("No files found matching pattern")
            } else {
                format_results(&results, &root, limit, true)
            });
        }
        let ensure = options
            .ensure_tool
            .clone()
            .unwrap_or_else(native_ensure_tool);
        let program = ensure("fd").await?;
        check_abort(signal)?;
        let program = program.ok_or("fd is not available and could not be downloaded")?;
        let exists = options.path_exists.clone().unwrap_or_else(|| {
            Arc::new(|path| {
                Box::pin(async move { Ok(super::path_utils::path_exists(&path).await) })
            })
        });
        let mut current = root.clone();
        let mut inside_git = false;
        loop {
            if exists(crate::coding_agent::core::path_join(&current, ".git")).await? {
                inside_git = true;
                break;
            }
            let parent = std::path::Path::new(&current)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| current.clone());
            if parent == current || parent.is_empty() {
                break;
            }
            current = parent;
        }
        let command = SearchCommand {
            program,
            args: build_fd_args(&input.pattern, &root, limit, inside_git, cfg!(windows)),
        };
        let lines = Arc::new(Mutex::new(Vec::new()));
        let on_line: SearchLineCallback = {
            let lines = lines.clone();
            Arc::new(move |line| {
                lines.lock().unwrap().push(line);
                true
            })
        };
        let runner = options.runner.clone().unwrap_or_else(native_runner);
        let exit = runner(command, on_line)
            .await
            .map_err(|e| format!("Failed to run fd: {e}"))?;
        check_abort(signal)?;
        let lines = lines.lock().unwrap();
        let output = lines.join("\n");
        if exit.code != Some(0) && output.is_empty() {
            let error = trim_js_whitespace(&exit.stderr);
            return Err(if error.is_empty() {
                format!(
                    "fd exited with code {}",
                    exit.code.map_or_else(|| "null".into(), |v| v.to_string())
                )
            } else {
                error.into()
            });
        }
        if output.is_empty() {
            return Ok(text_result("No files found matching pattern"));
        }
        let paths = lines
            .iter()
            .map(|line| trim_js_whitespace(line.strip_suffix('\r').unwrap_or(line)))
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        Ok(format_results(&paths, &root, limit, false))
    })
    .await
}
