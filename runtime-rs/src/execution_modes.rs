//! Release execution-mode and permission boundaries, shared by HTTP and actual tool preflight.
use regex::Regex;
use serde_json::Value;
use std::{
    path::{Component, Path, PathBuf},
    sync::{Mutex, OnceLock},
};

pub(crate) const DEFAULT_EXECUTION_MODE: &str = "approval-required";
pub(crate) fn normalize(mode: &str) -> Option<&'static str> {
    match mode {
        "approval-required" => Some("approval-required"),
        "workspace" | "workspace-write" => Some("workspace-write"),
        "full-access" => Some("full-access"),
        _ => None,
    }
}
pub(crate) fn permission_mode(mode: &str) -> &'static str {
    match mode {
        "approval-required" => "ask",
        "workspace-write" => "auto",
        _ => "ignore",
    }
}
pub(crate) fn tool_risk(name: &str) -> &'static str {
    match name {
        "read" | "ls" | "grep" | "find" | "decision" | "memory_search" | "mcp_list"
        | "discover_tools" | "call_tool" | "get_goal" | "update_goal" | "get_plan"
        | "update_plan" | "get_task_list" | "update_task_list" | "list_agents" | "send_message"
        | "wait_agent" | "send_team_message" | "list_team_members" => "low",
        "memory_remember" | "web_search" | "browser_automation" | "spawn_agent"
        | "followup_task" | "interrupt_agent" | "update_team_task" | "run_team_workflow" => {
            "medium"
        }
        _ => "high",
    }
}
pub(crate) fn visible(name: &str, mode: &str, external_risk: Option<&str>) -> bool {
    if mode != "approval-required" {
        return true;
    }
    matches!(
        name,
        "edit"
            | "write"
            | "bash"
            | "skill_create"
            | "plugin_create"
            | "generate_visual"
            | "computer-use"
            | "mobile_device"
            | "update_plan"
            | "spawn_agent"
            | "followup_task"
            | "interrupt_agent"
            | "update_team_task"
            | "run_team_workflow"
            | "discover_tools"
            | "call_tool"
            | "get_goal"
            | "update_goal"
            | "get_plan"
            | "get_task_list"
            | "list_agents"
            | "send_message"
            | "wait_agent"
    ) || matches!(
        name,
        "find_roots"
            | "observe_ui"
            | "search_ui"
            | "expand_ui"
            | "inspect_ui"
            | "act_ui"
            | "read_text"
            | "wait_for"
            | "launch_browser"
            | "navigate_browser"
            | "evaluate_browser"
    ) || matches!(
        external_risk.unwrap_or_else(|| tool_risk(name)),
        "low" | "低风险"
    )
}

#[derive(Debug, Clone)]
pub(crate) struct Requirement {
    pub risk: String,
    pub reason: String,
    pub block: bool,
    pub skip_remember: bool,
}
impl Requirement {
    fn ask(risk: &str, reason: impl Into<String>) -> Self {
        Self {
            risk: risk.into(),
            reason: reason.into(),
            block: false,
            skip_remember: false,
        }
    }
    fn block(reason: impl Into<String>) -> Self {
        Self {
            block: true,
            ..Self::ask("high", reason)
        }
    }
}

fn lexical(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}
pub(crate) fn canonical(path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let path = lexical(&path);
    let mut target = path.clone();
    let mut suffix = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&target) {
            return suffix.iter().rev().fold(real, |mut base, part| {
                base.push(part);
                base
            });
        }
        let Some(name) = target.file_name().map(|name| name.to_os_string()) else {
            return path;
        };
        if !target.pop() {
            return path;
        }
        suffix.push(name);
    }
}
pub(crate) fn absolute(cwd: &str, input: &str) -> PathBuf {
    let path = Path::new(input);
    canonical(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(cwd).join(path)
    })
}
fn within(root: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        let root = root
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase();
        let target = target
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase();
        target == root || target.starts_with(&(root.trim_end_matches('/').to_string() + "/"))
    }
    #[cfg(not(windows))]
    {
        target.starts_with(root)
    }
}
pub(crate) fn outside_workspace(cwd: &str, input: &str) -> bool {
    !input.trim().is_empty() && !within(&canonical(Path::new(cwd)), &absolute(cwd, input))
}
pub(crate) fn ownership(
    cwd: &str,
    name: &str,
    args: &Value,
    owned_files: &[String],
) -> Option<Requirement> {
    if !matches!(name, "edit" | "write") || owned_files.iter().all(|scope| scope.trim().is_empty())
    {
        return None;
    }
    let path = args["path"]
        .as_str()
        .or_else(|| args["file_path"].as_str())
        .unwrap_or_default();
    if path.trim().is_empty() {
        return Some(Requirement::block(format!(
            "{name} 没有目标路径，无法按 Team 文件所有权执行。"
        )));
    }
    let target = absolute(cwd, path);
    if owned_files
        .iter()
        .filter(|scope| !scope.trim().is_empty())
        .any(|scope| within(&absolute(cwd, scope), &target))
    {
        return None;
    }
    Some(Requirement::block(format!(
        "{name} 只能修改当前 Team 任务声明的文件：{}。收到的是 {path}。",
        owned_files.join(", ")
    )))
}
pub(crate) fn requirement(
    mode: &str,
    execution: &str,
    cwd: &str,
    name: &str,
    args: &Value,
    external_risk: Option<&str>,
) -> Option<Requirement> {
    if execution == "full-access"
        || matches!(
            name,
            "get_goal"
                | "update_goal"
                | "get_plan"
                | "update_plan"
                | "get_task_list"
                | "update_task_list"
                | "mobile_device"
        )
    {
        return None;
    }
    if name == "skill_create" && args["scope"] == "global" {
        return Some(Requirement::block(
            "skill_create 只有在完全访问模式下才能创建全局技能。",
        ));
    }
    if name == "plugin_create" {
        return Some(Requirement::ask(
            "high",
            "plugin_create 将创建并安装全局插件，需要确认后执行。",
        ));
    }
    if matches!(name, "read" | "ls" | "grep" | "find" | "edit" | "write")
        && outside_workspace(
            cwd,
            args["path"]
                .as_str()
                .or_else(|| args["file_path"].as_str())
                .unwrap_or_default(),
        )
    {
        return Some(Requirement::block(format!(
            "{name} 不能在当前执行模式下访问当前工作目录之外的文件。"
        )));
    }
    if matches!(name, "read" | "ls" | "grep" | "find") {
        return None;
    }
    if name == "bash" {
        if let Some(guard) = guard_command(args["command"].as_str().unwrap_or_default()) {
            let reason = format!("命令守卫拦截：{}。", guard.reason);
            return Some(if guard.block {
                Requirement::block(reason)
            } else {
                Requirement::ask("high", format!("{reason}请确认后执行。"))
            });
        }
    }
    if mode == "auto" {
        return (name == "browser_automation"
            && matches!(args["action"].as_str(), Some("click" | "type")))
        .then(|| {
            Requirement::ask(
                "high",
                "浏览器交互可能提交表单或改变远端状态，需要确认后执行。",
            )
        });
    }
    if matches!(name, "edit" | "write" | "skill_create" | "plugin_create") {
        return Some(Requirement::ask(
            "high",
            format!("{name} 将修改当前工作区中的文件，需要确认后执行。"),
        ));
    }
    if name == "bash" {
        return Some(Requirement::ask(
            "high",
            "Shell 命令将以当前操作系统用户权限运行，批准后可访问工作区之外的文件和网络。",
        ));
    }
    let risk = external_risk.unwrap_or_else(|| tool_risk(name));
    (mode == "ask" && matches!(risk, "medium" | "high" | "中风险" | "高风险"))
        .then(|| Requirement::ask(risk, format!("{name} 属于{risk}工具，需要确认后执行。")))
}

fn matches_regex(pattern: &str, value: &str) -> bool {
    static PATTERNS: OnceLock<Mutex<std::collections::HashMap<String, Regex>>> = OnceLock::new();
    let mut patterns = PATTERNS
        .get_or_init(Default::default)
        .lock()
        .expect("command patterns");
    patterns
        .entry(pattern.to_string())
        .or_insert_with(|| Regex::new(pattern).expect("release command pattern"))
        .is_match(value)
}
fn copy_expansion(chars: &[char], index: &mut usize, output: &mut String, delimiter: char) {
    let opening = chars[*index];
    output.push(opening);
    *index += 1;
    let mut depth = 1;
    while *index < chars.len() {
        let ch = chars[*index];
        *index += 1;
        if ch == '\\' && *index < chars.len() {
            output.push(ch);
            output.push(chars[*index]);
            *index += 1;
            continue;
        }
        if delimiter == '`' {
            output.push(ch);
            if ch == '`' {
                break;
            }
        } else {
            if ch == opening {
                depth += 1;
            }
            if ch == delimiter {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            output.push(ch);
        }
    }
}
pub(crate) fn mask_literals(command: &str) -> String {
    let chars: Vec<_> = command.chars().collect();
    let mut output = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '\'' {
            index += 1;
            while index < chars.len() && chars[index] != '\'' {
                output.push(' ');
                index += 1;
            }
            index += usize::from(index < chars.len());
        } else if ch == '"' {
            index += 1;
            while index < chars.len() && chars[index] != '"' {
                let ch = chars[index];
                if ch == '\\' && index + 1 < chars.len() {
                    output.push(ch);
                    output.push(chars[index + 1]);
                    index += 2;
                } else if ch == '`' {
                    copy_expansion(&chars, &mut index, &mut output, '`');
                } else if ch == '$'
                    && index + 1 < chars.len()
                    && matches!(chars[index + 1], '(' | '{')
                {
                    output.push('$');
                    index += 1;
                    let delimiter = if chars[index] == '(' { ')' } else { '}' };
                    copy_expansion(&chars, &mut index, &mut output, delimiter);
                } else if ch == '$' {
                    output.push('$');
                    index += 1;
                    while index < chars.len()
                        && (chars[index].is_ascii_alphanumeric() || chars[index] == '_')
                    {
                        output.push(chars[index]);
                        index += 1;
                    }
                } else {
                    output.push(' ');
                    index += 1;
                }
            }
            index += usize::from(index < chars.len());
        } else if ch == '`' {
            copy_expansion(&chars, &mut index, &mut output, '`');
        } else if ch == '$' && index + 1 < chars.len() && chars[index + 1] == '(' {
            output.push('$');
            index += 1;
            copy_expansion(&chars, &mut index, &mut output, ')');
        } else {
            output.push(ch);
            index += 1;
        }
    }
    output
}
#[derive(Debug)]
pub(crate) struct CommandGuard {
    pub block: bool,
    pub rule: &'static str,
    pub reason: &'static str,
}
pub(crate) fn guard_command(command: &str) -> Option<CommandGuard> {
    let masked = mask_literals(command);
    let root_pattern = r"(?i)/(?:\s|$|\*)|~(?:\s|$|/|\*)|(?:\$home\b|\$\{home\})|/(?:bin|boot|dev|etc|home|lib|lib64|opt|proc|root|sbin|srv|sys|usr|var)\b";
    let system_pattern = r"(?i)/(?:etc|bin|sbin|boot|lib|lib64|usr|var|proc|sys|root)(?:/|\b)";
    let segments: Vec<_> = masked
        .split([';', '&', '|', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let parsed: Vec<_> = segments
        .iter()
        .map(|segment| {
            let mut parts: Vec<_> = segment.split_whitespace().collect();
            while parts.first().is_some_and(|part| {
                matches!(
                    part.to_ascii_lowercase().as_str(),
                    "sudo" | "doas" | "command" | "nohup" | "env" | "time" | "xargs"
                ) || matches_regex(r"^[A-Za-z_][A-Za-z0-9_]*=\S+$", part)
            }) {
                parts.remove(0);
            }
            (
                parts
                    .first()
                    .copied()
                    .unwrap_or_default()
                    .to_ascii_lowercase(),
                parts.into_iter().skip(1).collect::<Vec<_>>(),
                *segment,
            )
        })
        .collect();
    let flag = |args: &[&str], value: &str| {
        args.iter().any(|arg| {
            *arg == value || (value.starts_with("--") && arg.starts_with(&format!("{value}=")))
        })
    };
    let short = |args: &[&str], value: char| {
        args.iter().any(|arg| {
            matches_regex(r"^-[a-zA-Z]+$", arg) && arg.to_ascii_lowercase().contains(value)
        })
    };
    let recursive = |args: &[&str]| flag(args, "--recursive") || short(args, 'r');
    let any = |f: &dyn Fn(&str, &[&str], &str) -> bool| {
        parsed
            .iter()
            .any(|(program, args, segment)| f(program, args, segment))
    };
    let rules: Vec<(&'static str, bool, &'static str, bool)> = vec![
        (
            "rm-root",
            true,
            "递归删除根目录或家目录",
            any(&|p, a, s| p == "rm" && recursive(a) && matches_regex(root_pattern, s)),
        ),
        (
            "rm-recursive",
            false,
            "递归删除（rm -r）",
            any(&|p, a, _| p == "rm" && recursive(a)),
        ),
        (
            "dd-device",
            true,
            "使用 dd 写入原始块设备",
            any(&|p, _, s| {
                p == "dd"
                    && matches_regex(r"(?i)/dev/(?:sd|hd|vd|xvd|nvme|mmcblk|mapper|disk|loop)", s)
            }),
        ),
        (
            "write-device",
            true,
            "重定向写入原始块设备",
            matches_regex(
                r"(?i)>\s*/dev/(?:sd|hd|vd|xvd|nvme|mmcblk|mapper|disk|loop)",
                &masked,
            ),
        ),
        (
            "disk-tool",
            true,
            "磁盘分区或格式化工具",
            any(&|p, _, _| {
                matches_regex(
                    r"(?i)^(mkfs(\.\w+)?|fdisk|parted|mkswap|wipefs|sfdisk|sgdisk|cgdisk|gdisk)$",
                    p,
                )
            }),
        ),
        (
            "chmod-root",
            true,
            "修改根目录或家目录权限",
            any(&|p, a, s| {
                p == "chmod"
                    && matches_regex(root_pattern, s)
                    && (recursive(a)
                        || a.iter().any(|a| {
                            matches_regex(r"^7{3,4}$", a) || matches!(*a, "a+rwx" | "777")
                        }))
            }),
        ),
        (
            "chown-root",
            true,
            "递归修改根目录或家目录属主",
            any(&|p, a, s| p == "chown" && recursive(a) && matches_regex(root_pattern, s)),
        ),
        (
            "fork-bomb",
            true,
            "fork 炸弹",
            matches_regex(
                r":\(\)\s*\{\s*[^}]*:\s*\||\(\)\s*\{\s*[^}]*\|[^}]*&",
                &masked,
            ),
        ),
        (
            "git-reset-hard",
            false,
            "git reset --hard 丢弃未提交改动",
            any(&|p, a, _| p == "git" && a.first() == Some(&"reset") && flag(a, "--hard")),
        ),
        (
            "git-clean-force",
            false,
            "git clean -f 删除未跟踪文件",
            any(&|p, a, _| {
                p == "git" && a.first() == Some(&"clean") && (short(a, 'f') || flag(a, "--force"))
            }),
        ),
        (
            "git-checkout-force",
            false,
            "git checkout/restore -f 丢弃本地改动",
            any(&|p, a, _| {
                p == "git"
                    && matches!(a.first(), Some(&"checkout" | &"restore"))
                    && (short(a, 'f') || flag(a, "--force"))
            }),
        ),
        (
            "git-push-force",
            true,
            "git push --force 覆盖远端历史",
            any(&|p, a, _| {
                p == "git"
                    && a.first() == Some(&"push")
                    && (short(a, 'f')
                        || flag(a, "--force")
                        || flag(a, "--mirror")
                        || a.iter().any(|a| matches_regex(r"^\+[^:]+", a)))
            }),
        ),
        (
            "git-branch-delete-force",
            false,
            "git branch -D 强制删除分支",
            any(&|p, a, _| {
                p == "git"
                    && a.first() == Some(&"branch")
                    && (a.contains(&"-D") || (flag(a, "--delete") && flag(a, "--force")))
            }),
        ),
        (
            "git-history-rewrite",
            true,
            "重写或永久清除 git 历史",
            any(&|p, a, _| {
                p == "git"
                    && (matches!(a.first(), Some(&"filter-branch" | &"filter-repo"))
                        || (a.first() == Some(&"reflog")
                            && a.contains(&"expire")
                            && a.contains(&"--all"))
                        || (a.first() == Some(&"gc") && a.contains(&"--prune=now")))
            }),
        ),
        (
            "curl-pipe-shell",
            true,
            "将远程内容直接管道给 shell 执行（远程代码执行）",
            matches_regex(
                r"(?i)(?:curl|wget)\b[^;&|\n]*\|\s*(?:sudo\s+)?(?:sh|bash|zsh|dash|ksh|python\d*|perl|ruby|node|npm|deno)\b|(?:sh|bash|zsh|source|\.)\s+<\s*\(\s*(?:curl|wget)\b|(?:eval|sh|bash|zsh)\b[^;&|\n]*(?:\$\s*\(\s*(?:curl|wget)\b|`\s*(?:curl|wget)\b)",
                &masked,
            ),
        ),
        (
            "rm-system-file",
            true,
            "删除系统文件或目录",
            any(&|p, _, s| p == "rm" && matches_regex(system_pattern, s)),
        ),
        (
            "redirect-system-file",
            true,
            "重定向覆盖系统文件",
            matches_regex(
                r"(?i)>+\s*/(?:etc|bin|sbin|boot|lib|lib64|usr|var|proc|sys|root)(?:/|\b)",
                &masked,
            ),
        ),
        (
            "find-delete-root",
            true,
            "使用 find 递归删除根目录或家目录",
            any(&|p, _, s| {
                p == "find"
                    && matches_regex(r"(?:-delete|(?:-exec|-execdir)\s+rm\b)", s)
                    && matches_regex(root_pattern, s)
            }),
        ),
        (
            "find-delete",
            false,
            "使用 find 递归删除文件",
            any(&|p, _, s| {
                p == "find" && matches_regex(r"(?:-delete|(?:-exec|-execdir)\s+rm\b)", s)
            }),
        ),
        (
            "shutdown",
            false,
            "关机或重启系统",
            any(&|p, a, _| {
                matches_regex(
                    r"(?i)^(?:shutdown|reboot|poweroff|halt|init|telinit|restart-computer|stop-computer)$",
                    p,
                ) || (p == "systemctl"
                    && a.iter().any(|a| {
                        matches_regex(r"(?i)^(?:reboot|poweroff|halt|suspend|hibernate)$", a)
                    }))
            }),
        ),
        (
            "mv-dev-null",
            true,
            "将文件移动到 /dev/null（销毁文件）",
            any(&|p, a, _| p == "mv" && a.last() == Some(&"/dev/null")),
        ),
        (
            "format-drive",
            true,
            "格式化磁盘卷",
            cfg!(windows) && any(&|p, _, _| p == "format"),
        ),
        (
            "diskpart",
            true,
            "磁盘管理（diskpart）",
            cfg!(windows) && any(&|p, _, _| p == "diskpart"),
        ),
        (
            "del-tree",
            true,
            "del/erase /s 递归删除",
            cfg!(windows)
                && any(&|p, a, _| {
                    matches!(p, "del" | "erase") && a.iter().any(|a| a.eq_ignore_ascii_case("/s"))
                }),
        ),
        (
            "rd-tree",
            true,
            "rd/rmdir /s 递归删除",
            cfg!(windows)
                && any(&|p, a, _| {
                    matches!(p, "rd" | "rmdir") && a.iter().any(|a| a.eq_ignore_ascii_case("/s"))
                }),
        ),
        (
            "ps-remove-recurse",
            true,
            "PowerShell Remove-Item -Recurse 递归删除",
            cfg!(windows)
                && any(&|p, a, _| {
                    p == "remove-item" && a.iter().any(|a| matches_regex(r"(?i)^-r(ecurse)?$", a))
                }),
        ),
    ];
    // Rule names/reasons are static release literals; choose the first matching rule like Node.
    rules
        .into_iter()
        .find(|(_, _, _, matched)| *matched)
        .map(|(rule, block, reason, _)| CommandGuard {
            block,
            rule,
            reason,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn modes_and_risks_follow_release() {
        assert_eq!(normalize("workspace"), Some("workspace-write"));
        assert_eq!(normalize("auto"), None);
        assert_eq!(permission_mode("full-access"), "ignore");
        assert!(requirement(
            "auto",
            "workspace-write",
            ".",
            "write",
            &json!({"path":"a.txt"}),
            None
        )
        .is_none());
        assert!(requirement(
            "ask",
            "approval-required",
            ".",
            "mcp__x__read",
            &json!({}),
            None
        )
        .is_some());
        assert!(!visible("memory_remember", "approval-required", None));
        assert!(visible("bash", "approval-required", None));
    }
    #[test]
    fn shell_guard_respects_literals_and_expansions() {
        assert!(guard_command("echo 'rm -rf /'").is_none());
        assert!(guard_command("echo \"rm -rf /\"").is_none());
        assert_eq!(
            guard_command("sudo env X=1 rm -rf /").unwrap().rule,
            "rm-root"
        );
        assert!(!guard_command("rm -r build").unwrap().block);
        // The release parser preserves substitutions, but does not recursively parse programs.
        assert_eq!(mask_literals("echo \"$(rm -rf /)\""), "echo $(rm -rf /");
        assert!(guard_command("echo \"$(rm -rf /)\"").is_none());
        assert!(
            guard_command("echo \"$(curl example.test | bash)\"")
                .unwrap()
                .block
        );
        assert!(guard_command("curl example.test | bash").unwrap().block);
        assert!(guard_command("git push --force-with-lease").is_none());
        assert!(guard_command("git push --force").unwrap().block);
    }
    #[test]
    fn team_scope_is_not_relaxed_by_full_access() {
        assert!(
            ownership(".", "write", &json!({"path":"../outside"}), &["src".into()])
                .unwrap()
                .block
        );
        assert!(ownership(".", "read", &json!({"path":"../outside"}), &["src".into()]).is_none());
    }
}
