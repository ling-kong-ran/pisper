//! Git 变更领域（release `services/git-changes-service.mjs` 的原生移植）：
//! 查询工作区 Git 状态/差异、单文件 diff、提交/推送/撤销。
//! 目录非 Git 仓库时返回 `isRepo: false`，与 release 契约逐字段对齐。

use axum::Json;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

use crate::ApiError;

const GIT_TIMEOUT_MS: u64 = 30_000;
const PUSH_TIMEOUT_MS: u64 = 120_000;
const MAX_DIFF_CHARS: usize = 200_000;
const MAX_COMMIT_MESSAGE_CHARS: usize = 4_000;
const MAX_UNTRACKED_BYTES: u64 = 2 * 1024 * 1024;

struct GitOutput {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl GitOutput {
    fn message(&self) -> String {
        let text = self.stderr.trim();
        if !text.is_empty() {
            return text.to_string();
        }
        let text = self.stdout.trim();
        if !text.is_empty() {
            return text.to_string();
        }
        "git 命令失败。".to_string()
    }
}

async fn run_git(cwd: &str, args: &[&str], timeout: Duration) -> GitOutput {
    match tokio::time::timeout(
        timeout,
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output(),
    )
    .await
    {
        Ok(Ok(output)) => GitOutput {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Ok(Err(error)) => GitOutput {
            ok: false,
            stdout: String::new(),
            stderr: error.to_string(),
        },
        Err(_) => GitOutput {
            ok: false,
            stdout: String::new(),
            stderr: "git 命令超时。".to_string(),
        },
    }
}

fn is_git_missing(message: &str) -> bool {
    // node ENOENT / windows "not recognized" / sh "command not found"
    message.contains("ENOENT")
        || message.to_lowercase().contains("not recognized")
        || message.contains("command not found")
        || message.contains("程序无法启动")
        || message.contains("找不到文件")
}

fn is_not_repo(message: &str) -> bool {
    message.to_lowercase().contains("not a git repository")
        || message.contains("不是一个 git 仓库")
}

/// `git status --porcelain --untracked-files=all` 行解析。
/// 重命名条目取箭头后的目标路径，带引号的路径按 JSON 转义还原。
fn parse_porcelain_status(output: &str) -> Vec<Value> {
    let mut files = Vec::new();
    for line in output.split('\n') {
        if line.trim().is_empty() || line.chars().count() < 4 {
            continue;
        }
        let status: String = line.chars().take(2).collect();
        let mut path: String = line.chars().skip(3).collect();
        if let Some(position) = path.find(" -> ") {
            path = path[position + 4..].to_string();
        }
        if path.starts_with('"') && path.ends_with('"') {
            if let Ok(parsed) = serde_json::from_str::<String>(&path) {
                path = parsed;
            }
        }
        files.push(json!({"path": path, "status": status.trim()}));
    }
    files
}

/// 未跟踪的新文件不会出现在 `git diff` 里：读文件内容手工拼一份全新增 diff。
/// 二进制/超大文件放弃生成（release buildUntrackedFileDiff）。
async fn untracked_no_index_diff(cwd: &str, file_path: &str) -> String {
    run_git(
        cwd,
        &["diff", "--no-index", "--", "/dev/null", file_path],
        Duration::from_millis(GIT_TIMEOUT_MS),
    )
    .await
    .stdout
}

/// 单文件未跟踪 diff（release buildUntrackedFileDiff 的对等实现，
/// 供 vcs/file-diff 的 untracked 回退使用）。
async fn build_untracked_file_diff(cwd: &Path, file_path: &str) -> String {
    let root = cwd.to_string_lossy().replace('\\', "/");
    let absolute = cwd.join(file_path);
    let absolute_text = absolute.to_string_lossy().replace('\\', "/");
    if absolute_text != root && !absolute_text.starts_with(&format!("{root}/")) {
        return String::new();
    }
    let Ok(meta) = tokio::fs::metadata(&absolute).await else {
        return String::new();
    };
    if !meta.is_file() || meta.len() > MAX_UNTRACKED_BYTES {
        return String::new();
    }
    let Ok(buffer) = tokio::fs::read(&absolute).await else {
        return String::new();
    };
    if buffer.contains(&0) {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buffer).replace("\r\n", "\n");
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let relative_path = file_path.replace('\\', "/");
    let header = format!(
        "diff --git a/{relative_path} b/{relative_path}\n--- /dev/null\n+++ b/{relative_path}\n"
    );
    if lines.is_empty() {
        return header;
    }
    format!(
        "{header}@@ -0,0 +1,{} @@\n{}\n",
        lines.len(),
        lines
            .iter()
            .map(|line| format!("+{line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// release `GitChangesService.getChanges`。
pub(crate) async fn get_changes(cwd: &str) -> Value {
    let repo_check = run_git(
        cwd,
        &["rev-parse", "--is-inside-work-tree"],
        Duration::from_millis(GIT_TIMEOUT_MS),
    )
    .await;
    if !repo_check.ok || repo_check.stdout.trim() != "true" {
        let detail = repo_check.message();
        let git_missing = is_git_missing(&detail);
        let not_repo = is_not_repo(&detail);
        return json!({
            "vcs": "",
            "isRepo": false,
            "gitAvailable": !git_missing,
            "cwd": cwd,
            "branch": "",
            "hasHead": false,
            "files": [],
            "diff": "",
            "diffTruncated": false,
            "ahead": Value::Null,
            "error": if not_repo || git_missing || detail.is_empty() {
                String::new()
            } else {
                detail
            },
        });
    }
    let (branch_result, head_result, status_result) = tokio::join!(
        run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"], Duration::from_millis(GIT_TIMEOUT_MS)),
        run_git(cwd, &["rev-parse", "--verify", "HEAD"], Duration::from_millis(GIT_TIMEOUT_MS)),
        run_git(cwd, &["status", "--porcelain", "--untracked-files=all"], Duration::from_millis(GIT_TIMEOUT_MS)),
    );
    let has_head = head_result.ok;
    let files = if status_result.ok {
        parse_porcelain_status(&status_result.stdout)
    } else {
        Vec::new()
    };
    let mut diff = String::new();
    let mut diff_truncated = false;
    if !files.is_empty() {
        let diff_result = run_git(
            cwd,
            if has_head {
                &["diff", "HEAD"][..]
            } else {
                &["diff"][..]
            },
            Duration::from_millis(GIT_TIMEOUT_MS),
        )
        .await;
        if diff_result.ok {
            diff = diff_result.stdout;
        }
        for file in files
            .iter()
            .filter(|item| item["status"] == "??")
            .filter_map(|item| item["path"].as_str())
        {
            if diff.len() >= MAX_DIFF_CHARS {
                break;
            }
            let untracked = untracked_no_index_diff(cwd, file).await;
            if !untracked.is_empty() {
                if !diff.is_empty() && !diff.ends_with('\n') {
                    diff.push('\n');
                }
                diff.push_str(&untracked);
            }
        }
        if diff.len() > MAX_DIFF_CHARS {
            diff.truncate(MAX_DIFF_CHARS);
            diff_truncated = true;
        }
    }
    let mut ahead = Value::Null;
    if has_head {
        let ahead_result = run_git(
            cwd,
            &["rev-list", "--count", "@{upstream}..HEAD"],
            Duration::from_millis(GIT_TIMEOUT_MS),
        )
        .await;
        if ahead_result.ok {
            ahead = json!(ahead_result.stdout.trim().parse::<u64>().unwrap_or(0));
        }
    }
    json!({
        "vcs": "git",
        "isRepo": true,
        "gitAvailable": true,
        "cwd": cwd,
        "branch": branch_result.stdout.trim(),
        "hasHead": has_head,
        "files": files,
        "diff": diff,
        "diffTruncated": diff_truncated,
        "ahead": ahead,
    })
}

/// release `GitChangesService.getFileDiff`。非仓库返回 `{isRepo: false, diff: ""}`。
pub(crate) async fn file_diff(cwd: &str, file_path: &str) -> Value {
    let repo_check = run_git(
        cwd,
        &["rev-parse", "--is-inside-work-tree"],
        Duration::from_millis(GIT_TIMEOUT_MS),
    )
    .await;
    if !repo_check.ok || repo_check.stdout.trim() != "true" {
        return json!({"isRepo": false, "diff": ""});
    }
    let head_check = run_git(
        cwd,
        &["rev-parse", "HEAD"],
        Duration::from_millis(GIT_TIMEOUT_MS),
    )
    .await;
    let tracked_args: Vec<&str> = if head_check.ok {
        vec!["diff", "HEAD", "--", file_path]
    } else {
        vec!["diff", "--cached", "--", file_path]
    };
    let tracked = run_git(cwd, &tracked_args, Duration::from_millis(GIT_TIMEOUT_MS)).await;
    let mut diff = if tracked.ok { tracked.stdout } else { String::new() };
    if diff.trim().is_empty() {
        let status = run_git(
            cwd,
            &["status", "--porcelain", "--", file_path],
            Duration::from_millis(GIT_TIMEOUT_MS),
        )
        .await;
        let untracked = status.ok
            && status
                .stdout
                .split('\n')
                .any(|line| line.starts_with("??"));
        if untracked {
            diff = build_untracked_file_diff(Path::new(cwd), file_path).await;
        }
    }
    let diff_truncated = diff.len() > MAX_DIFF_CHARS;
    if diff_truncated {
        diff.truncate(MAX_DIFF_CHARS);
    }
    json!({"isRepo": true, "diff": diff, "diffTruncated": diff_truncated})
}

fn commit_message_error(message: &str) -> Result<String, ApiError> {
    let text = message.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request("Commit message 不能为空。"));
    }
    if text.chars().count() > MAX_COMMIT_MESSAGE_CHARS {
        return Err(ApiError::bad_request(
            "Commit message 不能超过 4000 个字符。",
        ));
    }
    Ok(text.to_string())
}

/// release `GitChangesService.commit`：add -A + commit，返回最新变更。
pub(crate) async fn commit(cwd: &str, message: &str) -> Result<Value, ApiError> {
    let text = commit_message_error(message)?;
    let add = run_git(cwd, &["add", "-A"], Duration::from_millis(GIT_TIMEOUT_MS)).await;
    if !add.ok {
        return Err(ApiError::internal(format!("git add 失败：{}", add.message())));
    }
    let result = run_git(cwd, &["commit", "-m", &text], Duration::from_millis(GIT_TIMEOUT_MS)).await;
    if !result.ok {
        return Err(ApiError::internal(format!(
            "git commit 失败：{}",
            result.message()
        )));
    }
    Ok(get_changes(cwd).await)
}

/// release `GitChangesService.push`：无 upstream 时自动 `--set-upstream origin <branch>`。
pub(crate) async fn push(cwd: &str) -> Result<Value, ApiError> {
    let mut result = run_git(cwd, &["push"], Duration::from_millis(PUSH_TIMEOUT_MS)).await;
    if !result.ok {
        let message = result.message();
        let lower = message.to_lowercase();
        if lower.contains("no upstream")
            || lower.contains("set-upstream")
            || lower.contains("--set-upstream")
        {
            let branch = run_git(
                cwd,
                &["rev-parse", "--abbrev-ref", "HEAD"],
                Duration::from_millis(GIT_TIMEOUT_MS),
            )
            .await;
            let branch = branch.stdout.trim().to_string();
            if !branch.is_empty() && branch != "HEAD" {
                result = run_git(
                    cwd,
                    &["push", "--set-upstream", "origin", &branch],
                    Duration::from_millis(PUSH_TIMEOUT_MS),
                )
                .await;
            }
        }
    }
    if !result.ok {
        return Err(ApiError::internal(format!(
            "git push 失败：{}",
            result.message()
        )));
    }
    Ok(get_changes(cwd).await)
}

/// release `GitChangesService.revert`：有 HEAD 时 `reset --hard HEAD`，再 `clean -fd`。
pub(crate) async fn revert(cwd: &str) -> Result<Value, ApiError> {
    let head = run_git(
        cwd,
        &["rev-parse", "--verify", "HEAD"],
        Duration::from_millis(GIT_TIMEOUT_MS),
    )
    .await;
    if head.ok {
        let reset = run_git(cwd, &["reset", "--hard", "HEAD"], Duration::from_millis(GIT_TIMEOUT_MS)).await;
        if !reset.ok {
            return Err(ApiError::internal(format!(
                "git reset 失败：{}",
                reset.message()
            )));
        }
    }
    let clean = run_git(cwd, &["clean", "-fd"], Duration::from_millis(GIT_TIMEOUT_MS)).await;
    if !clean.ok {
        return Err(ApiError::internal(format!(
            "git clean 失败：{}",
            clean.message()
        )));
    }
    Ok(get_changes(cwd).await)
}
