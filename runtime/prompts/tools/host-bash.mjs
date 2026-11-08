// 本机 bash 工具的说明：精简说明（lean-coding-tools 的 bash）之后追加一句，说明命令在哪个 shell、以什么身份运行。
const HOST_NOTE =
  'Commands run as the current operating-system user and can access host files and networks.'
const POWERSHELL_NOTE =
  'Bash was not found on Windows, so commands run with the system Windows PowerShell (powershell.exe).'

/** @param {string} leanDescription @param {{ powershell?: boolean }} [options] */
export function hostBashDescription(leanDescription, { powershell = false } = {}) {
  const base = powershell
    ? String(leanDescription).replace('Run a bash command', 'Run a command')
    : String(leanDescription)
  return `${base}\n${powershell ? POWERSHELL_NOTE : HOST_NOTE}`
}
