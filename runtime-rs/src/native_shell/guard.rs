//! Release command-guard rules. This unconditional spawn guard is independent
//! of execution-mode permission decisions. Its string analysis is not an OS sandbox.
use regex::Regex;
use std::sync::{Mutex, OnceLock};
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
                    output.push(ch);
                    break;
                }
            }
            output.push(ch);
        }
    }
}
pub(super) fn mask_literals(command: &str) -> String {
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
                } else if ch == '$'
                    && index + 1 < chars.len()
                    && (chars[index + 1].is_ascii_alphabetic() || chars[index + 1] == '_')
                {
                    output.push('$');
                    index += 1;
                    while index < chars.len()
                        && (chars[index].is_ascii_alphanumeric() || chars[index] == '_')
                    {
                        output.push(chars[index]);
                        index += 1;
                    }
                } else if ch == '$' {
                    output.push('$');
                    index += 1;
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
pub(super) struct CommandGuard {
    pub block: bool,
    pub rule: &'static str,
    pub reason: &'static str,
}
pub(super) fn guard_command(command: &str, windows: bool) -> Option<CommandGuard> {
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
                    *part,
                    "sudo" | "doas" | "command" | "nohup" | "env" | "time" | "xargs"
                ) || matches_regex(r"^[A-Za-z_][A-Za-z0-9_]*=\S+$", part)
            }) {
                parts.remove(0);
            }
            (
                parts.first().copied().unwrap_or_default().to_lowercase(),
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
            windows && any(&|p, _, _| p == "format"),
        ),
        (
            "diskpart",
            true,
            "磁盘管理（diskpart）",
            windows && any(&|p, _, _| p == "diskpart"),
        ),
        (
            "del-tree",
            true,
            "del/erase /s 递归删除",
            windows
                && any(&|p, a, _| {
                    matches!(p, "del" | "erase") && a.iter().any(|a| a.eq_ignore_ascii_case("/s"))
                }),
        ),
        (
            "rd-tree",
            true,
            "rd/rmdir /s 递归删除",
            windows
                && any(&|p, a, _| {
                    matches!(p, "rd" | "rmdir") && a.iter().any(|a| a.eq_ignore_ascii_case("/s"))
                }),
        ),
        (
            "ps-remove-recurse",
            true,
            "PowerShell Remove-Item -Recurse 递归删除",
            windows
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

pub(super) fn format_guard_error(decision: &CommandGuard, command: &str) -> String {
    let (message, hint) = match decision.rule {
        "rm-root" => (
            "recursive rm targeting the filesystem root or home directory",
            "use a scoped, non-recursive path and ask the user to confirm",
        ),
        "rm-recursive" => (
            "recursive rm (recursive deletion)",
            "ask the user to confirm before deleting directories recursively",
        ),
        "dd-device" => ("dd writing to a raw block device", ""),
        "write-device" => ("shell redirection writing to a raw block device", ""),
        "disk-tool" => (
            "disk-partitioning / filesystem-creation tool",
            "these rewrite disks or partitions and are never safe to run unattended",
        ),
        "chmod-root" => (
            "chmod on the filesystem root or home directory",
            "never make system/home trees world-writable",
        ),
        "chown-root" => (
            "recursive chown on the filesystem root or home directory",
            "",
        ),
        "fork-bomb" => ("fork bomb", ""),
        "git-reset-hard" => (
            "git reset --hard (discards uncommitted work)",
            "use `git reset --soft/--mixed`, stash, or ask the user to confirm",
        ),
        "git-clean-force" => ("git clean with --force (deletes untracked files)", ""),
        "git-checkout-force" => (
            "git checkout/restore with --force (discards local changes)",
            "",
        ),
        "git-push-force" => (
            "git push with --force/--mirror (rewrites remote history)",
            "prefer `git push --force-with-lease`",
        ),
        "git-branch-delete-force" => ("git branch -D (force-deletes a branch)", ""),
        "git-history-rewrite" => (
            "git history rewrite or permanent object pruning",
            "this permanently destroys commits and is irreversible",
        ),
        "curl-pipe-shell" => (
            "remote content piped to a shell interpreter (remote code execution)",
            "download the script, inspect it, then run it explicitly",
        ),
        "rm-system-file" => ("rm targeting a system file or directory", ""),
        "redirect-system-file" => ("shell redirection overwriting a system file", ""),
        "find-delete-root" => ("find -delete / -exec rm on root or home", ""),
        "find-delete" => ("find -delete / -exec rm (recursive deletion)", ""),
        "shutdown" => (
            "shutdown / reboot / poweroff",
            "confirm with the user before powering off or rebooting",
        ),
        "mv-dev-null" => ("mv to /dev/null (destroys the source file)", ""),
        "format-drive" => ("format (formats a disk volume)", ""),
        "diskpart" => ("diskpart (disk management)", ""),
        "del-tree" => ("del/erase with /s (recursive delete)", ""),
        "rd-tree" => ("rd/rmdir with /s (recursive remove)", ""),
        "ps-remove-recurse" => (
            "PowerShell Remove-Item with -Recurse (recursive delete)",
            "",
        ),
        _ => (decision.reason, ""),
    };
    let hint = if hint.is_empty() {
        String::new()
    } else {
        format!("\nHint: {hint}.")
    };
    format!("Blocked dangerous command [{}]: {message}.\n  $ {command}\nRefusing to run this automatically. Ask the user for explicit confirmation.{hint}", decision.rule)
}
