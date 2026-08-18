// bash 命令级安全守卫：命令在真正执行前的第一道保守字符串分析。
// 注意：这不是安全边界——混淆命令（替换/别名/eval/编码载荷）可绕过字符串解析，
// 必须配合操作系统沙箱（Linux bwrap/Landlock、Windows AppContainer）才能真正强制。
// 每条规则有严重度：block（灾难性/不可逆，无条件拒绝）、warn（可恢复破坏，转人工审批）。
/**
 * Command-level safety guard for the Pisper bash tool.
 *
 * This is the FIRST defense layer: a conservative, cross-platform string
 * analysis that classifies commands before they are spawned.
 *
 * It is intentionally NOT a security boundary. Obfuscated commands (command
 * substitution, aliases, eval, encoded payloads) can evade string parsing, so
 * this must be paired with an OS sandbox for real enforcement (bwrap/Landlock
 * on Linux, AppContainer on Windows).
 *
 * Each rule has a severity:
 * - `block`: catastrophic / irreversible — refused unconditionally.
 * - `warn`: destructive but recoverable — routed to human approval.
 */

// Leading environment assignments and modifier commands, stripped repeatedly
// (e.g. `sudo env FOO=1 rm -rf /`).
const LEADING_RE = /^(?:(?:[A-Za-z_][A-Za-z0-9_]*=\S+|sudo|doas|command|nohup|env|time|xargs)\s+)+/

// Filesystem root, home directory, and system directories used as deletion /
// permission-change targets.
const ROOT_OR_HOME_RE =
  /\/(?:\s|$|\*)|~(?:\s|$|\/|\*)|(?:\$home\b|\$\{home\})|\/(?:bin|boot|dev|etc|home|lib|lib64|opt|proc|root|sbin|srv|sys|usr|var)\b/i

// Raw block devices (excludes /dev/null, /dev/zero, /dev/random, ...).
const BLOCK_DEVICE_RE = /\/dev\/(?:sd|hd|vd|xvd|nvme|mmcblk|mapper|disk|loop)/i

// Critical system paths (used as deletion / overwrite targets).
const SYSTEM_PATH_RE = /\/(?:etc|bin|sbin|boot|lib|lib64|usr|var|proc|sys|root)(?:\/|\b)/i

/**
 * Replace the contents of single/double-quoted literals with spaces so that
 * `echo "rm -rf /"` is not treated as a real `rm` invocation. `$`, backticks
 * and escapes are kept inside double quotes because those actually execute.
 */
export function maskQuotedLiterals(command) {
  let out = ''
  let i = 0
  const n = command.length

  while (i < n) {
    const ch = command[i]

    // Single-quoted literal: mask everything until the closing quote.
    if (ch === "'") {
      i += 1
      while (i < n && command[i] !== "'") {
        out += ' '
        i += 1
      }
      i += 1 // skip the closing quote
      continue
    }

    // Double-quoted literal: mask everything except expansions, which execute.
    if (ch === '"') {
      i += 1
      while (i < n && command[i] !== '"') {
        const c = command[i]
        if (c === '\\' && i + 1 < n) {
          out += c + command[i + 1]
          i += 2
          continue
        }
        if (c === '`') {
          const r = copyBacktick(command, i)
          out += r.text
          i = r.next
          continue
        }
        if (c === '$' && command[i + 1] === '(') {
          const r = copyDollarParen(command, i)
          out += r.text
          i = r.next
          continue
        }
        if (c === '$' && command[i + 1] === '{') {
          const r = copyBracedExpansion(command, i)
          out += r.text
          i = r.next
          continue
        }
        if (c === '$' && /[A-Za-z_]/.test(command[i + 1] || '')) {
          out += '$'
          i += 1
          while (i < n && /[A-Za-z0-9_]/.test(command[i])) {
            out += command[i]
            i += 1
          }
          continue
        }
        if (c === '$') {
          out += '$'
          i += 1
          continue
        }
        out += ' '
        i += 1
      }
      i += 1 // skip the closing quote
      continue
    }

    // Backtick command substitution outside quotes: keep verbatim.
    if (ch === '`') {
      const r = copyBacktick(command, i)
      out += r.text
      i = r.next
      continue
    }

    // $( ... ) command substitution outside quotes: keep verbatim.
    if (ch === '$' && command[i + 1] === '(') {
      const r = copyDollarParen(command, i)
      out += r.text
      i = r.next
      continue
    }

    out += ch
    i += 1
  }

  return out
}

function copyBacktick(command, i) {
  let text = command[i] // the backtick
  i += 1
  while (i < command.length && command[i] !== '`') {
    const ch = command[i]
    if (ch === '\\' && i + 1 < command.length) {
      text += ch + command[i + 1]
      i += 2
      continue
    }
    text += ch
    i += 1
  }
  if (i < command.length) {
    text += command[i]
    i += 1
  }
  return { text, next: i }
}

function copyDollarParen(command, i) {
  let text = '$('
  i += 2
  let depth = 1
  while (i < command.length && depth > 0) {
    const ch = command[i]
    if (ch === '\\' && i + 1 < command.length) {
      text += ch + command[i + 1]
      i += 2
      continue
    }
    if (ch === '(') {
      depth += 1
      text += ch
      i += 1
      continue
    }
    if (ch === ')') {
      depth -= 1
      if (depth > 0) text += ch
      i += 1
      continue
    }
    text += ch
    i += 1
  }
  return { text, next: i }
}

function copyBracedExpansion(command, i) {
  let text = '${'
  i += 2
  let depth = 1
  while (i < command.length && depth > 0) {
    const ch = command[i]
    if (ch === '{') {
      depth += 1
      text += ch
      i += 1
      continue
    }
    if (ch === '}') {
      depth -= 1
      if (depth > 0) text += ch
      i += 1
      continue
    }
    text += ch
    i += 1
  }
  return { text, next: i }
}

/** Split a masked command into simple command segments on shell separators. */
export function splitSegments(command) {
  return command
    .split(/[;&|\n]+/)
    .map((s) => s.trim())
    .filter(Boolean)
}

/**
 * Parse one segment into its program and argument tokens, ignoring leading
 * environment assignments and modifier commands (`sudo`, `env`, `time`, ...).
 */
export function parseSegment(segment) {
  let s = segment.trim()
  s = s.replace(LEADING_RE, '')
  const parts = s.split(/\s+/).filter(Boolean)
  return { program: parts[0] || '', args: parts.slice(1) }
}

function programIs(program, name) {
  return program.toLowerCase() === name.toLowerCase()
}

function hasFlag(args, flag) {
  return args.some((a) => a === flag || (flag.startsWith('--') && a.startsWith(`${flag}=`)))
}

/** Match a combined short flag such as `-r` inside `-rf` / `-fr` / `-R`. */
function hasShortFlag(args, letter) {
  return args.some((a) => /^-[a-zA-Z]+$/.test(a) && a.toLowerCase().includes(letter))
}

function hasRecursiveFlag(args) {
  return args.some((a) => a === '--recursive' || /^-[a-zA-Z]*[rR][a-zA-Z]*$/.test(a))
}

function hasRootOrHomeTarget(segment) {
  return ROOT_OR_HOME_RE.test(segment)
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

const RULES = [
  {
    id: 'rm-root',
    severity: 'block',
    message: 'recursive rm targeting the filesystem root or home directory',
    reason: '递归删除根目录或家目录',
    hint: 'use a scoped, non-recursive path and ask the user to confirm',
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        return programIs(program, 'rm') && hasRecursiveFlag(args) && hasRootOrHomeTarget(seg)
      })
    },
  },
  {
    id: 'rm-recursive',
    severity: 'warn',
    message: 'recursive rm (recursive deletion)',
    reason: '递归删除（rm -r）',
    hint: 'ask the user to confirm before deleting directories recursively',
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        return programIs(program, 'rm') && hasRecursiveFlag(args)
      })
    },
  },
  {
    id: 'dd-device',
    severity: 'block',
    message: 'dd writing to a raw block device',
    reason: '使用 dd 写入原始块设备',
    test(segments) {
      return segments.some((seg) => {
        const { program } = parseSegment(seg)
        return programIs(program, 'dd') && BLOCK_DEVICE_RE.test(seg)
      })
    },
  },
  {
    id: 'write-device',
    severity: 'block',
    message: 'shell redirection writing to a raw block device',
    reason: '重定向写入原始块设备',
    test(segments, masked) {
      return />\s*\/dev\/(?:sd|hd|vd|xvd|nvme|mmcblk|mapper|disk|loop)/i.test(masked)
    },
  },
  {
    id: 'disk-tool',
    severity: 'block',
    message: 'disk-partitioning / filesystem-creation tool',
    reason: '磁盘分区或格式化工具',
    hint: 'these rewrite disks or partitions and are never safe to run unattended',
    test(segments) {
      const TOOLS = /^(mkfs(\.\w+)?|fdisk|parted|mkswap|wipefs|sfdisk|sgdisk|cgdisk|gdisk)$/i
      return segments.some((seg) => {
        const { program } = parseSegment(seg)
        return TOOLS.test(program)
      })
    },
  },
  {
    id: 'chmod-root',
    severity: 'block',
    message: 'chmod on the filesystem root or home directory',
    reason: '修改根目录或家目录权限',
    hint: 'never make system/home trees world-writable',
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        if (!programIs(program, 'chmod')) return false
        if (!hasRootOrHomeTarget(seg)) return false
        return (
          hasRecursiveFlag(args) ||
          args.some((a) => /^7{3,4}$/.test(a) || a === 'a+rwx' || a === '777')
        )
      })
    },
  },
  {
    id: 'chown-root',
    severity: 'block',
    message: 'recursive chown on the filesystem root or home directory',
    reason: '递归修改根目录或家目录属主',
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        return programIs(program, 'chown') && hasRecursiveFlag(args) && hasRootOrHomeTarget(seg)
      })
    },
  },
  {
    id: 'fork-bomb',
    severity: 'block',
    message: 'fork bomb',
    reason: 'fork 炸弹',
    test(segments, masked) {
      return /:\(\)\s*\{\s*[^}]*:\s*\|/.test(masked) || /\(\)\s*\{\s*[^}]*\|[^}]*&/.test(masked)
    },
  },
  {
    id: 'git-reset-hard',
    severity: 'warn',
    message: 'git reset --hard (discards uncommitted work)',
    reason: 'git reset --hard 丢弃未提交改动',
    hint: 'use `git reset --soft/--mixed`, stash, or ask the user to confirm',
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        return programIs(program, 'git') && args[0] === 'reset' && hasFlag(args, '--hard')
      })
    },
  },
  {
    id: 'git-clean-force',
    severity: 'warn',
    message: 'git clean with --force (deletes untracked files)',
    reason: 'git clean -f 删除未跟踪文件',
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        return (
          programIs(program, 'git') &&
          args[0] === 'clean' &&
          (hasShortFlag(args, 'f') || hasFlag(args, '--force'))
        )
      })
    },
  },
  {
    id: 'git-checkout-force',
    severity: 'warn',
    message: 'git checkout/restore with --force (discards local changes)',
    reason: 'git checkout/restore -f 丢弃本地改动',
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        if (!programIs(program, 'git')) return false
        if (args[0] !== 'checkout' && args[0] !== 'restore') return false
        return hasShortFlag(args, 'f') || hasFlag(args, '--force')
      })
    },
  },
  {
    id: 'git-push-force',
    severity: 'block',
    message: 'git push with --force/--mirror (rewrites remote history)',
    reason: 'git push --force 覆盖远端历史',
    hint: 'prefer `git push --force-with-lease`',
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        if (!programIs(program, 'git') || args[0] !== 'push') return false
        return (
          hasShortFlag(args, 'f') ||
          hasFlag(args, '--force') ||
          hasFlag(args, '--mirror') ||
          args.some((a) => /^\+[^:]+/.test(a))
        )
      })
    },
  },
  {
    id: 'git-branch-delete-force',
    severity: 'warn',
    message: 'git branch -D (force-deletes a branch)',
    reason: 'git branch -D 强制删除分支',
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        if (!programIs(program, 'git') || args[0] !== 'branch') return false
        return args.includes('-D') || (hasFlag(args, '--delete') && hasFlag(args, '--force'))
      })
    },
  },
  {
    id: 'git-history-rewrite',
    severity: 'block',
    message: 'git history rewrite or permanent object pruning',
    reason: '重写或永久清除 git 历史',
    hint: 'this permanently destroys commits and is irreversible',
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        if (!programIs(program, 'git')) return false
        const sub = args[0]
        if (sub === 'filter-branch' || sub === 'filter-repo') return true
        if (sub === 'reflog' && args.includes('expire') && args.includes('--all')) return true
        if (sub === 'gc' && args.some((a) => a === '--prune=now')) return true
        return false
      })
    },
  },
  {
    id: 'curl-pipe-shell',
    severity: 'block',
    message: 'remote content piped to a shell interpreter (remote code execution)',
    reason: '将远程内容直接管道给 shell 执行（远程代码执行）',
    hint: 'download the script, inspect it, then run it explicitly',
    test(segments, masked) {
      return (
        /(?:curl|wget)\b[^;&|\n]*\|\s*(?:sudo\s+)?(?:sh|bash|zsh|dash|ksh|python\d*|perl|ruby|node|npm|deno)\b/i.test(
          masked,
        ) ||
        /(?:sh|bash|zsh|source|\.)\s+<\s*\(\s*(?:curl|wget)\b/i.test(masked) ||
        /(?:eval|sh|bash|zsh)\b[^;&|\n]*(?:\$\s*\(\s*(?:curl|wget)\b|`\s*(?:curl|wget)\b)/i.test(
          masked,
        )
      )
    },
  },
  {
    id: 'rm-system-file',
    severity: 'block',
    message: 'rm targeting a system file or directory',
    reason: '删除系统文件或目录',
    test(segments) {
      return segments.some((seg) => {
        const { program } = parseSegment(seg)
        return programIs(program, 'rm') && SYSTEM_PATH_RE.test(seg)
      })
    },
  },
  {
    id: 'redirect-system-file',
    severity: 'block',
    message: 'shell redirection overwriting a system file',
    reason: '重定向覆盖系统文件',
    test(segments, masked) {
      return />+\s*\/(?:etc|bin|sbin|boot|lib|lib64|usr|var|proc|sys|root)(?:\/|\b)/i.test(masked)
    },
  },
  {
    id: 'find-delete-root',
    severity: 'block',
    message: 'find -delete / -exec rm on root or home',
    reason: '使用 find 递归删除根目录或家目录',
    test(segments) {
      return segments.some((seg) => {
        const { program } = parseSegment(seg)
        if (!programIs(program, 'find')) return false
        if (!/(?:-delete|(?:-exec|-execdir)\s+rm\b)/.test(seg)) return false
        return hasRootOrHomeTarget(seg)
      })
    },
  },
  {
    id: 'find-delete',
    severity: 'warn',
    message: 'find -delete / -exec rm (recursive deletion)',
    reason: '使用 find 递归删除文件',
    test(segments) {
      return segments.some((seg) => {
        const { program } = parseSegment(seg)
        if (!programIs(program, 'find')) return false
        return /(?:-delete|(?:-exec|-execdir)\s+rm\b)/.test(seg)
      })
    },
  },
  {
    id: 'shutdown',
    severity: 'warn',
    message: 'shutdown / reboot / poweroff',
    reason: '关机或重启系统',
    hint: 'confirm with the user before powering off or rebooting',
    test(segments) {
      const REBOOT =
        /^(?:shutdown|reboot|poweroff|halt|init|telinit|restart-computer|stop-computer)$/i
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        if (REBOOT.test(program)) return true
        if (programIs(program, 'systemctl')) {
          return args.some((a) => /^(?:reboot|poweroff|halt|suspend|hibernate)$/i.test(a))
        }
        return false
      })
    },
  },
  {
    id: 'mv-dev-null',
    severity: 'block',
    message: 'mv to /dev/null (destroys the source file)',
    reason: '将文件移动到 /dev/null（销毁文件）',
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        if (!programIs(program, 'mv')) return false
        return args[args.length - 1] === '/dev/null'
      })
    },
  },
  {
    id: 'format-drive',
    severity: 'block',
    message: 'format (formats a disk volume)',
    reason: '格式化磁盘卷',
    platforms: ['win32'],
    test(segments) {
      return segments.some((seg) => {
        const { program } = parseSegment(seg)
        return programIs(program, 'format')
      })
    },
  },
  {
    id: 'diskpart',
    severity: 'block',
    message: 'diskpart (disk management)',
    reason: '磁盘管理（diskpart）',
    platforms: ['win32'],
    test(segments) {
      return segments.some((seg) => {
        const { program } = parseSegment(seg)
        return programIs(program, 'diskpart')
      })
    },
  },
  {
    id: 'del-tree',
    severity: 'block',
    message: 'del/erase with /s (recursive delete)',
    reason: 'del/erase /s 递归删除',
    platforms: ['win32'],
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        if (!programIs(program, 'del') && !programIs(program, 'erase')) return false
        return args.some((a) => a.toLowerCase() === '/s')
      })
    },
  },
  {
    id: 'rd-tree',
    severity: 'block',
    message: 'rd/rmdir with /s (recursive remove)',
    reason: 'rd/rmdir /s 递归删除',
    platforms: ['win32'],
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        if (!programIs(program, 'rd') && !programIs(program, 'rmdir')) return false
        return args.some((a) => a.toLowerCase() === '/s')
      })
    },
  },
  {
    id: 'ps-remove-recurse',
    severity: 'block',
    message: 'PowerShell Remove-Item with -Recurse (recursive delete)',
    reason: 'PowerShell Remove-Item -Recurse 递归删除',
    platforms: ['win32'],
    test(segments) {
      return segments.some((seg) => {
        const { program, args } = parseSegment(seg)
        return programIs(program, 'Remove-Item') && args.some((a) => /^-r(ecurse)?$/i.test(a))
      })
    },
  },
]

/**
 * Analyse a shell command and classify it.
 * Returns `{ blocked: false }` when allowed, otherwise
 * `{ blocked: true, severity: 'block' | 'warn', ruleId, message, reason, hint }`.
 */
export function guardCommand(command, { platform = process.platform } = {}) {
  if (typeof command !== 'string' || command.trim() === '') return { blocked: false }
  const masked = maskQuotedLiterals(command)
  const segments = splitSegments(masked)
  for (const rule of RULES) {
    if (rule.platforms && !rule.platforms.includes(platform)) continue
    if (rule.test(segments, masked)) {
      return {
        blocked: true,
        severity: rule.severity,
        ruleId: rule.id,
        message: rule.message,
        reason: rule.reason,
        hint: rule.hint || '',
      }
    }
  }
  return { blocked: false }
}

export function formatGuardError(decision, command) {
  const hint = decision.hint ? `\nHint: ${decision.hint}.` : ''
  return (
    `Blocked dangerous command [${decision.ruleId}]: ${decision.message}.` +
    `\n  $ ${command}` +
    `\nRefusing to run this automatically. Ask the user for explicit confirmation.` +
    hint
  )
}
