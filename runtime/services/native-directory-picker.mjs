// 在 Runtime 所在主机上弹出系统原生的文件夹选择框，供同机浏览器获得绝对路径。
// 浏览器的目录句柄拿不到路径，桌面壳之外只能由本机 Node 进程代为调用系统对话框。
import { execFile } from 'node:child_process'
import { access, constants } from 'node:fs/promises'
import { delimiter, join } from 'node:path'

/** @typedef {{ status: 'picked', path: string } | { status: 'cancelled' } | { status: 'unavailable' } | { status: 'busy' }} DirectoryPickResult */

// 用户可能长时间停留在对话框里；超时只防止子进程永久悬挂。
const PICK_TIMEOUT_MS = 30 * 60_000

/** @param {string} file @param {string[]} args @param {{ env?: NodeJS.ProcessEnv }} [options] */
function run(file, args, options = {}) {
  return new Promise((resolve) => {
    execFile(
      file,
      args,
      { timeout: PICK_TIMEOUT_MS, windowsHide: false, maxBuffer: 1024 * 1024, ...options },
      (error, stdout, stderr) =>
        resolve({
          code: error ? (typeof error.code === 'number' ? error.code : -1) : 0,
          stdout: String(stdout || ''),
          stderr: String(stderr || ''),
        }),
    )
  })
}

/** @param {string} name @param {NodeJS.ProcessEnv} env */
async function findExecutable(name, env) {
  for (const directory of String(env.PATH || '').split(delimiter)) {
    if (!directory) continue
    const candidate = join(directory, name)
    try {
      await access(candidate, constants.X_OK)
      return candidate
    } catch {
      // 继续查找下一个 PATH 目录。
    }
  }
  return ''
}

// macOS：参数经 argv 传入 AppleScript，路径与提示文字不拼进脚本源码。
const MAC_SCRIPT = [
  'on run argv',
  'set promptText to item 1 of argv',
  'set initialPath to item 2 of argv',
  'activate',
  'try',
  'if initialPath is "" then',
  'set picked to choose folder with prompt promptText',
  'else',
  'set picked to choose folder with prompt promptText default location (POSIX file initialPath)',
  'end if',
  'on error number -128',
  'return ""',
  'end try',
  'return POSIX path of picked',
  'end run',
]

// Windows：参数经环境变量传入，避免 PowerShell 命令行转义；输出强制 UTF-8 以保留中文路径。
// PowerShell 7（.NET Core）下 FolderBrowserDialog 是新式资源管理器样式，5.1 退回旧式树形对话框。
const WINDOWS_SCRIPT = [
  'Add-Type -AssemblyName System.Windows.Forms',
  '[Console]::OutputEncoding = [System.Text.Encoding]::UTF8',
  '$dialog = New-Object System.Windows.Forms.FolderBrowserDialog',
  '$dialog.Description = $env:PISPER_PICK_TITLE',
  "if ($dialog.PSObject.Properties['UseDescriptionForTitle']) { $dialog.UseDescriptionForTitle = $true }",
  'if ($env:PISPER_PICK_INITIAL) { $dialog.SelectedPath = $env:PISPER_PICK_INITIAL }',
  '$owner = New-Object System.Windows.Forms.Form -Property @{ TopMost = $true }',
  'if ($dialog.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $dialog.SelectedPath }',
].join('; ')

export class NativeDirectoryPicker {
  /**
   * @param {{ platform?: NodeJS.Platform, env?: NodeJS.ProcessEnv, runCommand?: typeof run }} [options]
   */
  constructor({ platform = process.platform, env = process.env, runCommand = run } = {}) {
    this.platform = platform
    this.env = env
    this.runCommand = runCommand
    this.pending = false
  }

  /** 解析当前主机可用的对话框命令；无图形会话或缺少工具时返回 null。 */
  async resolveLauncher() {
    if (['mobile-embedded', 'mobile-store'].includes(this.env.PISPER_RUNTIME_PROFILE)) return null
    if (this.platform === 'darwin') return { kind: 'mac', file: 'osascript' }
    if (this.platform === 'win32')
      return {
        kind: 'windows',
        file: (await findExecutable('pwsh.exe', this.env)) || 'powershell.exe',
      }
    if (this.platform !== 'linux') return null
    if (!this.env.DISPLAY && !this.env.WAYLAND_DISPLAY) return null
    for (const kind of /** @type {const} */ (['zenity', 'kdialog'])) {
      const file = await findExecutable(kind, this.env)
      if (file) return { kind, file }
    }
    return null
  }

  async available() {
    return Boolean(await this.resolveLauncher())
  }

  /**
   * @param {{ initialDirectory?: string, title?: string }} [options]
   * @returns {Promise<DirectoryPickResult>}
   */
  async pick({ initialDirectory = '', title = '' } = {}) {
    const launcher = await this.resolveLauncher()
    if (!launcher) return { status: 'unavailable' }
    // 同一时间只保留一个系统对话框，避免重复点击叠出多个窗口。
    if (this.pending) return { status: 'busy' }
    this.pending = true
    try {
      const result = await this.launch(
        launcher,
        String(initialDirectory || ''),
        String(title || ''),
      )
      // 工具存在但无法启动（如缺少图形会话权限）时视为不可用，交给调用方退回手动输入。
      if (result.code !== 0 && result.code !== 1) return { status: 'unavailable' }
      const path = result.stdout.replace(/\r?\n$/, '').trim()
      if (result.code === 1 || !path) return { status: 'cancelled' }
      return { status: 'picked', path: launcher.kind === 'mac' ? stripTrailingSlash(path) : path }
    } finally {
      this.pending = false
    }
  }

  /** @param {{ kind: string, file: string }} launcher @param {string} initial @param {string} title */
  launch(launcher, initial, title) {
    if (launcher.kind === 'mac')
      return this.runCommand(launcher.file, [
        ...MAC_SCRIPT.flatMap((line) => ['-e', line]),
        title,
        initial,
      ])
    if (launcher.kind === 'windows')
      return this.runCommand(
        launcher.file,
        ['-NoProfile', '-NonInteractive', '-STA', '-Command', WINDOWS_SCRIPT],
        { env: { ...this.env, PISPER_PICK_TITLE: title, PISPER_PICK_INITIAL: initial } },
      )
    if (launcher.kind === 'zenity')
      return this.runCommand(launcher.file, [
        '--file-selection',
        '--directory',
        ...(title ? [`--title=${title}`] : []),
        ...(initial ? [`--filename=${initial.replace(/\/?$/, '/')}`] : []),
      ])
    return this.runCommand(launcher.file, [
      '--getexistingdirectory',
      initial || this.env.HOME || '/',
      ...(title ? ['--title', title] : []),
    ])
  }
}

/** @param {string} path */
function stripTrailingSlash(path) {
  return path.length > 1 ? path.replace(/\/+$/, '') : path
}
