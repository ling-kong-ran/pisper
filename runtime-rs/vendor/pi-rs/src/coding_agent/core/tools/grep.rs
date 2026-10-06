//! Execution half of upstream grep.ts, using rg JSON events and async file context.
//! UTF-16 line slicing is retained; an isolated surrogate at the 500-unit cut is
//! replaced at Rust's UTF-8 Value boundary (the existing JSON-surrogate seam).
use super::{cancellable, check_abort, context_cwd, definition, text_result, ToolFuture};
use super::{path_utils::resolve_to_cwd, search_process::*, truncate::*};
use crate::coding_agent::extensions::types::{AbortSignal, ToolDefinition};
use crate::coding_agent::utils::{node_path, text::trim_js_whitespace};
use crate::serde_support::js_number_string;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepToolInput {
    pub pattern: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    pub ignore_case: Option<bool>,
    pub literal: Option<bool>,
    pub context: Option<f64>,
    pub limit: Option<f64>,
}
#[derive(Clone)]
pub struct GrepOperations {
    pub is_directory: Arc<dyn Fn(String) -> ToolFuture<bool> + Send + Sync>,
    pub read_file: Arc<dyn Fn(String) -> ToolFuture<String> + Send + Sync>,
}
impl Default for GrepOperations {
    fn default() -> Self {
        Self {
            is_directory: Arc::new(|path| {
                Box::pin(async move {
                    tokio::fs::metadata(&path)
                        .await
                        .map(|m| m.is_dir())
                        .map_err(|e| super::io_error(e, "stat", &path))
                })
            }),
            read_file: Arc::new(|path| {
                Box::pin(async move {
                    tokio::fs::read(&path)
                        .await
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                        .map_err(|e| super::io_error(e, "open", &path))
                })
            }),
        }
    }
}
#[derive(Clone, Default)]
pub struct GrepToolOptions {
    pub operations: Option<GrepOperations>,
    pub ensure_tool: Option<EnsureSearchTool>,
    pub runner: Option<SearchRunner>,
}
#[derive(Clone)]
struct Match {
    path: String,
    line_number: f64,
    line_text: Option<String>,
}
#[derive(Default)]
struct Matches {
    count: usize,
    limit_reached: bool,
    items: Vec<Match>,
}
pub fn build_rg_args(input: &GrepToolInput, root: &str) -> Vec<String> {
    let mut args = vec![
        "--json".into(),
        "--line-number".into(),
        "--color=never".into(),
        "--hidden".into(),
    ];
    if input.ignore_case == Some(true) {
        args.push("--ignore-case".into());
    }
    if input.literal == Some(true) {
        args.push("--fixed-strings".into());
    }
    if let Some(glob) = input.glob.as_ref().filter(|s| !s.is_empty()) {
        args.extend(["--glob".into(), glob.clone()]);
    }
    args.extend(["--".into(), input.pattern.clone(), root.into()]);
    args
}
fn format_path(file: &str, root: &str, is_directory: bool) -> String {
    if is_directory {
        let relative = if cfg!(windows) {
            node_path::win32_relative(root, file, root)
        } else {
            node_path::posix_relative(root, file, root)
        };
        if !relative.is_empty() && !relative.starts_with("..") {
            return relative.replace('\\', "/");
        }
    }
    std::path::Path::new(file)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}
fn truncate_line(line: &str) -> (String, bool) {
    let units = line.encode_utf16().collect::<Vec<_>>();
    if units.len() <= 500 {
        (line.into(), false)
    } else {
        (
            format!("{}... [truncated]", String::from_utf16_lossy(&units[..500])),
            true,
        )
    }
}
pub fn create_grep_tool_definition(cwd: &str, options: GrepToolOptions) -> Arc<ToolDefinition> {
    let mut def=definition("grep","Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to 100 matches or 50KB (whichever is hit first). Long lines are truncated to 500 chars.","Search file contents for patterns (respects .gitignore)",&[],json!({"type":"object","properties":{"pattern":{"type":"string","description":"Search pattern (regex or literal string)"},"path":{"type":"string","description":"Directory or file to search (default: current directory)"},"glob":{"type":"string","description":"Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'"},"ignoreCase":{"type":"boolean","description":"Case-insensitive search (default: false)"},"literal":{"type":"boolean","description":"Treat pattern as literal string instead of regex (default: false)"},"context":{"type":"number","description":"Number of lines to show before and after each match (default: 0)"},"limit":{"type":"number","description":"Maximum number of matches to return (default: 100)"}},"required":["pattern"]}),false);
    let cwd = cwd.to_owned();
    def.execute_async = Some(Arc::new(move |_, params, signal, _, ctx| {
        let cwd = cwd.clone();
        let options = options.clone();
        Box::pin(async move {
            let input = serde_json::from_value(params).map_err(|e| e.to_string())?;
            execute_grep(&input, &context_cwd(&ctx, &cwd)?, &options, signal.as_ref()).await
        })
    }));
    Arc::new(def)
}
pub async fn execute_grep(
    input: &GrepToolInput,
    cwd: &str,
    options: &GrepToolOptions,
    signal: Option<&Arc<AbortSignal>>,
) -> Result<Value, String> {
    cancellable(signal, async {
        let ensure = options
            .ensure_tool
            .clone()
            .unwrap_or_else(native_ensure_tool);
        let program = ensure("rg")
            .await?
            .ok_or("ripgrep (rg) is not available and could not be downloaded")?;
        let root = resolve_to_cwd(
            input
                .path
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("."),
            cwd,
        )?;
        let ops = options.operations.clone().unwrap_or_default();
        let is_directory = (ops.is_directory)(root.clone())
            .await
            .map_err(|_| format!("Path not found: {root}"))?;
        let context = input.context.filter(|value| *value > 0.0).unwrap_or(0.0);
        let limit = input.limit.unwrap_or(100.0).max(1.0);
        let state = Arc::new(Mutex::new(Matches::default()));
        let on_line: SearchLineCallback = {
            let state = state.clone();
            Arc::new(move |line| {
                let mut state = state.lock().unwrap();
                if trim_js_whitespace(&line).is_empty() || state.count as f64 >= limit {
                    return true;
                }
                if let Ok(event) = serde_json::from_str::<Value>(&line) {
                    if event["type"] == "match" {
                        state.count += 1;
                        if let (Some(path), Some(line_number)) = (
                            event["data"]["path"]["text"]
                                .as_str()
                                .filter(|s| !s.is_empty()),
                            event["data"]["line_number"].as_f64(),
                        ) {
                            state.items.push(Match {
                                path: path.into(),
                                line_number,
                                line_text: event["data"]["lines"]["text"]
                                    .as_str()
                                    .map(str::to_owned),
                            });
                        }
                        if state.count as f64 >= limit {
                            state.limit_reached = true;
                            return false;
                        }
                    }
                }
                true
            })
        };
        let runner = options.runner.clone().unwrap_or_else(native_runner);
        let exit = runner(
            SearchCommand {
                program,
                args: build_rg_args(input, &root),
            },
            on_line,
        )
        .await
        .map_err(|e| format!("Failed to run ripgrep: {e}"))?;
        check_abort(signal)?;
        let (count, limit_reached, matches) = {
            let state = state.lock().unwrap();
            (state.count, state.limit_reached, state.items.clone())
        };
        if !limit_reached && !matches!(exit.code, Some(0 | 1)) {
            let error = trim_js_whitespace(&exit.stderr);
            return Err(if error.is_empty() {
                format!(
                    "ripgrep exited with code {}",
                    exit.code.map_or_else(|| "null".into(), |v| v.to_string())
                )
            } else {
                error.into()
            });
        }
        if count == 0 {
            return Ok(text_result("No matches found"));
        }
        let mut cache = HashMap::<String, Vec<String>>::new();
        let mut output_lines = vec![];
        let mut lines_truncated = false;
        for matched in matches {
            let path = format_path(&matched.path, &root, is_directory);
            if let Some(line_text) = matched.line_text.as_deref().filter(|_| context == 0.0) {
                let sanitized = line_text.replace("\r\n", "\n").replace('\r', "");
                let sanitized = sanitized.strip_suffix('\n').unwrap_or(&sanitized);
                let (text, truncated) = truncate_line(sanitized);
                lines_truncated |= truncated;
                output_lines.push(format!(
                    "{path}:{}: {text}",
                    js_number_string(matched.line_number)
                ));
                continue;
            }
            if !cache.contains_key(&matched.path) {
                let lines = match (ops.read_file)(matched.path.clone()).await {
                    Ok(text) => text
                        .replace("\r\n", "\n")
                        .replace('\r', "\n")
                        .split('\n')
                        .map(str::to_owned)
                        .collect(),
                    Err(_) => vec![],
                };
                cache.insert(matched.path.clone(), lines);
            }
            let lines = &cache[&matched.path];
            if lines.is_empty() {
                output_lines.push(format!(
                    "{path}:{}: (unable to read file)",
                    js_number_string(matched.line_number)
                ));
                continue;
            }
            let start = if context > 0.0 {
                (matched.line_number - context).max(1.0)
            } else {
                matched.line_number
            };
            let end = if context > 0.0 {
                (matched.line_number + context).min(lines.len() as f64)
            } else {
                matched.line_number
            };
            let mut current = start;
            while current <= end {
                let text = if current >= 1.0 && current.fract() == 0.0 {
                    lines
                        .get((current - 1.0) as usize)
                        .map(String::as_str)
                        .unwrap_or("")
                } else {
                    ""
                };
                let (text, truncated) = truncate_line(&text.replace('\r', ""));
                lines_truncated |= truncated;
                output_lines.push(if current == matched.line_number {
                    format!("{path}:{}: {text}", js_number_string(current))
                } else {
                    format!("{path}-{}- {text}", js_number_string(current))
                });
                let next = current + 1.0;
                if next == current {
                    break;
                }
                current = next;
            }
        }
        let truncation = truncate_head(
            &output_lines.join("\n"),
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
                "{} matches limit reached. Use limit={} for more, or refine pattern",
                js_number_string(limit),
                js_number_string(limit * 2.0)
            ));
            details["matchLimitReached"] =
                serde_json::from_str(&js_number_string(limit)).unwrap_or(Value::Null);
        }
        if truncation.truncated {
            notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
            details["truncation"] = as_value(&truncation);
        }
        if lines_truncated {
            notices
                .push("Some lines truncated to 500 chars. Use read tool to see full lines".into());
            details["linesTruncated"] = json!(true);
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
