// 普通会话选了远程机器时追加的系统提示：路径和命令都在那台机器上，并说清那台机器是什么系统、
// bash 工具的命令实际交给哪个 shell，免得模型在 Windows 上写 bash、在 macOS 上用 GNU 专有参数。
// 每个会话固定不变（不随轮次变化），纯函数，无 I/O。

const SHELL_HINTS = {
  // SSH 到 Windows：命令一律在 Windows PowerShell 里跑。
  powershell:
    'The bash tool runs your command in Windows PowerShell, not bash: write PowerShell (Get-ChildItem, Select-String, Get-Content, $env:NAME, `;` between commands; there is no `&&` in Windows PowerShell 5.1). Paths use Windows form such as C:\\Users\\name\\project.',
  // 让对方连过来的执行节点在 Windows 上用 cmd.exe。
  cmd: 'The bash tool runs your command in cmd.exe, not bash: write cmd syntax (dir, type, findstr, set NAME=value, %NAME%, `&&` between commands), or call `powershell -NoProfile -Command "..."` when you need PowerShell. Paths use Windows form such as C:\\Users\\name\\project.',
  macos:
    "The machine is macOS with BSD tools: `sed -i` needs a suffix argument (sed -i '' ...), and GNU-only flags such as `readlink -f`, `grep -P` or `date -d` may be missing.",
}

/**
 * @param {{ name: string, system?: { os?: string, family?: string, name?: string, shell?: string } | null }} machine
 * @param {string} cwd
 */
export function machineSystemPrompt(machine, cwd) {
  const system = machine.system || null
  const lines = [
    `You are working on the remote machine "${machine.name}", not on the user's own computer.`,
    `File and shell tools run on that machine, in ${cwd}. All paths are paths on that machine.`,
  ]
  if (system?.family === 'windows') {
    const shell = /^cmd/i.test(system.shell || '') ? 'cmd' : 'powershell'
    lines.push(`Operating system: ${system.name || 'Windows'}.`, SHELL_HINTS[shell])
  } else if (system) {
    const shell = system.shell || 'sh'
    lines.push(
      `Operating system: ${system.name || system.os}. Commands run in its login shell (${shell}).`,
    )
    if (system.os === 'macos') lines.push(SHELL_HINTS.macos)
  }
  lines.push(
    'Commands run without a terminal and cannot read input; interactive prompts fail.',
    'Tools that are not listed are not available here.',
  )
  return lines.join('\n')
}
