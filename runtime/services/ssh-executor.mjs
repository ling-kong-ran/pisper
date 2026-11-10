// SSH 机器的执行器：会话的文件与命令工具经系统自带的 ssh 客户端在对方机器上执行，对方不用装 Pisper。
// 接口和执行节点一样（operations() 就是引擎工具可替换的 operations）。用系统 ssh 而不是 JS 实现：
// ~/.ssh/config、密钥、ssh-agent、ProxyJump 全部照旧，不新增依赖。
// 对方是 Linux、macOS 等有 POSIX shell 的系统时走 sh 脚本；是 Windows（OpenSSH 服务，默认 shell 是 cmd 或 PowerShell）
// 时走固定的 PowerShell 引导命令，脚本和数据经 stdin 传过去（ssh-powershell.mjs）。对方是什么系统由连上后探测一次，
// 记在机器记录上，不信用户填的值。不依赖 SFTP，也不要 PTY。
// 登录默认用密钥（BatchMode）；用户选了密码时经 askpass 助手（runtime/workers/ssh-askpass.mjs）交给 ssh：
// 密码不进命令行也不进环境变量（助手拿着一次性令牌向 Runtime 要，见 ssh-askpass-channel.mjs），ssh 不读用户和
// 系统的配置文件（没有代理命令、跳板机之类会继承环境的子进程），而且只认用户确认过指纹的那把主机公钥
// （ssh-host-key.mjs）——第一次连接也不会把密码交给冒充的服务器。
import { spawn } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join, posix, win32 } from 'node:path'
import { isSea } from 'node:sea'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { ExecutorError } from './bots/executor.mjs'
import { openAskpassChannel } from './ssh-askpass-channel.mjs'
import {
  ASKPASS_CHANNEL,
  ASKPASS_FLAG,
  ASKPASS_OTHER_HOST,
  ASKPASS_REFUSED,
  ASKPASS_TOKEN,
} from './ssh-askpass-prompt.mjs'
import {
  HOST_KEY_ALIAS,
  knownHostsLine,
  normalizeHostKey,
  parseKnownHosts,
} from './ssh-host-key.mjs'
import { POWERSHELL_COMMAND, POWERSHELL_SCRIPTS, powershellPayload } from './ssh-powershell.mjs'

const CONNECT_TIMEOUT_SECONDS = 10
const OPERATION_TIMEOUT_MS = 60_000
const PROBE_TIMEOUT_MS = 25_000
const KILL_TIMEOUT_MS = 8_000
const VERSION_TIMEOUT_MS = 5_000
const HOST_KEY_TIMEOUT_MS = 20_000
// unix socket 路径上限 104 字节（macOS）：放密码通道的临时目录名和 socket 名一共 20 来个字符，上级目录不能太长。
const MAX_SCRATCH_PARENT_CHARS = 70
// 密码登录前用 ssh -G 看这台机器实际会怎么连；结果只记这么久（用户可能刚改了 ~/.ssh/config）。
const LOGIN_PLAN_TTL_MS = 30_000
const MAX_READ_BYTES = 16 * 1024 * 1024
const MAX_STDERR_CHARS = 4_000
const MAX_GLOB_ENTRIES = 200_000
const MAX_GLOB_RESULTS = 10_000
const LIST_CACHE_TTL_MS = 5_000
// ssh 退出后 stdout 还没结束时最多再等这么久：ControlPersist 分出的后台进程可能还握着管道。
const EXIT_DRAIN_MS = 1_000
// Unix socket 路径上限：macOS 104 字节（含结尾 NUL）。ssh 在目录后面接 /%C（40 位十六进制），
// 建立时还要再接 . 和 16 位随机串，一共 58 个字符；目录超过 45 个字符时 ssh 会直接以失败退出。
const MAX_MUX_DIRECTORY_CHARS = 45
// SSH_ASKPASS_REQUIRE 从 OpenSSH 8.4 起才有；Windows 上没有它就不会去调用 askpass。
const ASKPASS_REQUIRE_VERSION = [8, 4]

const EXIT_MISSING = 44
const EXIT_IS_DIRECTORY = 45
const EXIT_DENIED = 46
const EXIT_NO_CWD = 97
// ssh 客户端自己失败（连不上、认证失败）的退出码；远程命令的退出码由对方决定。
const EXIT_SSH_FAILED = 255
const PROBE_MARKER = 'PISPER_SSH_PROBE'
const SHELL_MARKER = 'PISPER_SSH_SHELL_'
const REQUIRED_COMMANDS = ['cat', 'find', 'ls', 'mkdir']

export const ASKPASS_HELPER = fileURLToPath(new URL('../workers/ssh-askpass.mjs', import.meta.url))

// 远程命令总要经对方的 shell 解析一次：路径和命令一律单引号包起来，没有拼接的余地。
export function shellQuote(value) {
  const text = String(value)
  if (text.includes('\0'))
    throw new ExecutorError('invalid_path', 'Path cannot contain NUL characters.')
  return `'${text.replaceAll("'", `'\\''`)}'`
}

// 大脑在 Windows 时引擎工具用 path.resolve 拼路径，会把 /srv/app/a 变成 C:\srv\app\a。
// 对方是 POSIX 时这里还原回去；对方是 Windows 时路径本来就是 Windows 写法，不动。
export function toRemotePath(value, platform = process.platform, family = 'posix') {
  const text = String(value)
  if (family === 'windows') {
    if (text.includes('\0'))
      throw new ExecutorError('invalid_path', 'Path cannot contain NUL characters.')
    return text
  }
  if (platform !== 'win32') return text
  return text.replace(/^[A-Za-z]:(?=[\\/])/, '').replaceAll('\\', '/')
}

// 这台电脑上的 ssh：Windows 10/11 自带的在 System32\OpenSSH（可选功能，可能没装），没有时再找 PATH 里的。
// 只会是这两个之一，不接受用户填的路径。
export function sshExecutable(platform = process.platform, env = process.env, exists = existsSync) {
  if (platform !== 'win32') return 'ssh'
  const root = env.SystemRoot || env.windir || 'C:\\Windows'
  const bundled = win32.join(root, 'System32', 'OpenSSH', 'ssh.exe')
  return exists(bundled) ? bundled : 'ssh'
}

// ssh -V 的输出（在 stderr）：OpenSSH_9.6p1 …、OpenSSH_for_Windows_8.6p1 …。认不出时返回 null。
export function parseSshVersion(text) {
  const match = /OpenSSH(?:_for_Windows)?_(\d+)\.(\d+)/.exec(String(text))
  return match ? [Number(match[1]), Number(match[2])] : null
}

function versionAtLeast(version, [major, minor]) {
  return Boolean(version) && (version[0] > major || (version[0] === major && version[1] >= minor))
}

// 路径放进 -o 选项时，带空格的要用双引号包起来（ssh 按空白拆多个文件）；路径里不允许有双引号。
function optionPath(path) {
  const text = String(path)
  if (text.includes('"') || /[\r\n\0]/.test(text))
    throw new ExecutorError('invalid_path', 'The data folder path cannot be used with ssh.')
  return /\s/.test(text) ? `"${text}"` : text
}

// 选项写死在这里：不转发 agent 和端口（对方机器不该能用你的密钥）。
// 密钥登录：读这台电脑的 ssh 配置（别名、跳板机照旧），首次连接记下指纹，之后指纹变了就拒绝。
// 密码登录：
// - 关掉 BatchMode（否则 ssh 不问密码），只试密码和键盘交互、不试密钥（免得先把失败次数用掉），每种方式只问一次。
// - 只认 knownHostsFile 里那一把用户确认过的主机公钥（StrictHostKeyChecking=yes，不看别的 known_hosts，
//   也不让 ssh 往里加别的公钥）：对不上就在发密码之前断开。
// - 用 -F 指向一个空的配置文件：用户和系统的 ssh 配置都不读，ProxyCommand、ProxyJump、KnownHostsCommand、
//   LocalCommand、Match exec、SendEnv 一个都没有。-F 放在最后（后写的生效），其余选项放在最前（先写的生效）。
export function sshArguments(
  machine,
  {
    muxDirectory = '',
    extraArguments = [],
    password = false,
    configFile = '',
    knownHostsFile = '',
  } = {},
) {
  if (password && (!configFile || !knownHostsFile))
    throw new ExecutorError(
      'host_key_unconfirmed',
      'Password sign-in needs a confirmed host fingerprint.',
    )
  return [
    '-T',
    ...(password
      ? [
          '-o',
          'BatchMode=no',
          '-o',
          'PreferredAuthentications=password,keyboard-interactive',
          '-o',
          'PubkeyAuthentication=no',
          '-o',
          'NumberOfPasswordPrompts=1',
          '-o',
          'StrictHostKeyChecking=yes',
          '-o',
          `UserKnownHostsFile=${optionPath(knownHostsFile)}`,
          '-o',
          `GlobalKnownHostsFile=${optionPath(knownHostsFile)}`,
          '-o',
          `HostKeyAlias=${HOST_KEY_ALIAS}`,
          '-o',
          'CheckHostIP=no',
          '-o',
          'UpdateHostKeys=no',
        ]
      : ['-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=accept-new']),
    '-o',
    `ConnectTimeout=${CONNECT_TIMEOUT_SECONDS}`,
    '-o',
    'ServerAliveInterval=15',
    '-o',
    'ServerAliveCountMax=3',
    '-o',
    'ForwardAgent=no',
    '-o',
    'ForwardX11=no',
    '-o',
    'ClearAllForwardings=yes',
    '-o',
    'LogLevel=ERROR',
    ...(muxDirectory
      ? [
          '-o',
          'ControlMaster=auto',
          '-o',
          `ControlPath=${muxDirectory}/%C`,
          '-o',
          'ControlPersist=120',
        ]
      : []),
    ...(machine.port ? ['-p', String(machine.port)] : []),
    ...extraArguments,
    ...(password ? ['-F', configFile] : []),
    '--',
    machine.target,
  ]
}

// 取主机公钥：只握手、不登录。所有认证方式都关掉（不带密钥、不问密码），ssh 在验证完对方持有这把私钥后
// 把公钥写进一个临时的 known_hosts，随后因为没有可用的认证方式而退出。同样不读任何配置文件。
export function hostKeyArguments(machine, { configFile, knownHostsFile, extraArguments = [] }) {
  return [
    '-T',
    '-o',
    'BatchMode=yes',
    '-o',
    'PubkeyAuthentication=no',
    '-o',
    'PasswordAuthentication=no',
    '-o',
    'StrictHostKeyChecking=accept-new',
    '-o',
    `UserKnownHostsFile=${optionPath(knownHostsFile)}`,
    '-o',
    `GlobalKnownHostsFile=${optionPath(knownHostsFile)}`,
    '-o',
    `HostKeyAlias=${HOST_KEY_ALIAS}`,
    '-o',
    'HashKnownHosts=no',
    '-o',
    'CheckHostIP=no',
    '-o',
    'UpdateHostKeys=no',
    '-o',
    `ConnectTimeout=${CONNECT_TIMEOUT_SECONDS}`,
    '-o',
    'ForwardAgent=no',
    '-o',
    'ForwardX11=no',
    '-o',
    'ClearAllForwardings=yes',
    '-o',
    'LogLevel=ERROR',
    ...(machine.port ? ['-p', String(machine.port)] : []),
    ...extraArguments,
    '-F',
    configFile,
    '--',
    machine.target,
    'exit',
  ]
}

// 密码登录时给 ssh 的环境：SSH_ASKPASS 指向 Pisper 自己的可执行文件，再加上向 Runtime 要密码的通道地址和
// 一次性令牌。密码本身不在这里。打包版由入口脚本认出 PISPER_SSH_ASKPASS；开发版是 node，用 NODE_OPTIONS
// 先加载助手（替换而不是追加，只影响这次 ssh 及其子进程）。旧版 OpenSSH 不认 SSH_ASKPASS_REQUIRE，
// 要有 DISPLAY 且没有控制终端才调用助手：POSIX 上同时给 DISPLAY，并以新会话启动（spawnOptions 里的 detached）。
export function askpassEnvironment(
  channel,
  {
    env = process.env,
    platform = process.platform,
    packaged = isSea(),
    execPath = process.execPath,
    helper = ASKPASS_HELPER,
  } = {},
) {
  const next = {
    ...env,
    SSH_ASKPASS: execPath,
    SSH_ASKPASS_REQUIRE: 'force',
    [ASKPASS_FLAG]: '1',
    [ASKPASS_CHANNEL]: String(channel.address),
    [ASKPASS_TOKEN]: String(channel.token),
  }
  if (platform !== 'win32') next.DISPLAY = env.DISPLAY || ':0'
  if (packaged) delete next.NODE_OPTIONS
  else next.NODE_OPTIONS = `--import=${pathToFileURL(helper).href}`
  return next
}

// ssh -G 的输出（对这台机器实际生效的值）里，密码登录要看的几项：最终连的是谁（提示语里的 user@host 要和它对上）、
// 端口、中间有没有跳板或代理命令。
export function parseSshConfig(text) {
  const plan = { user: '', hostname: '', port: '', hostKeyAlias: '', proxy: false }
  for (const line of String(text).split(/\r?\n/)) {
    const at = line.indexOf(' ')
    if (at < 0) continue
    const key = line.slice(0, at).toLowerCase()
    const value = line.slice(at + 1).trim()
    if (!value) continue
    if (key === 'user') plan.user = value
    else if (key === 'hostname') plan.hostname = value
    else if (key === 'port') plan.port = value
    else if (key === 'hostkeyalias') plan.hostKeyAlias = value
    else if ((key === 'proxyjump' || key === 'proxycommand') && value.toLowerCase() !== 'none')
      plan.proxy = true
  }
  return plan
}

// 密码登录不读这台电脑的 ssh 配置。地址要是靠配置才连得上（别名、配置里写的用户名或端口、跳板机），
// 不读配置就会连到别处或连不上：事先比一下“读配置”和“不读配置”两种解析结果，不一样就说清楚，不悄悄失败。
export function dependsOnUserConfig(literal, configured) {
  if (!configured?.hostname) return false
  return (
    configured.proxy ||
    configured.hostname.toLowerCase() !== literal.hostname.toLowerCase() ||
    configured.user !== literal.user ||
    (configured.port || '22') !== (literal.port || '22')
  )
}

// ssh 客户端的错误只有英文，分类后由界面按语言给出下一步。
export function classifySshFailure(stderr) {
  const text = String(stderr)
  if (text.includes(ASKPASS_OTHER_HOST)) return 'askpass_other_host'
  if (text.includes(ASKPASS_REFUSED)) return 'unsupported_remote'
  if (/REMOTE HOST IDENTIFICATION HAS CHANGED|Host key verification failed/i.test(text))
    return 'host_key_changed'
  if (/Connection refused/i.test(text)) return 'connection_refused'
  if (/Permission denied|Too many authentication failures|publickey/i.test(text))
    return 'auth_failed'
  if (
    /timed out|No route to host|Network is unreachable|Could not resolve hostname|Name or service not known|nodename nor servname|Temporary failure in name resolution/i.test(
      text,
    )
  )
    return 'unreachable'
  return 'connection_failed'
}

const FAILURE_MESSAGES = {
  ssh_client_missing: 'ssh command not found on this computer.',
  host_key_changed:
    'Host key changed; connection refused for safety. Update known_hosts once you confirm the machine was reinstalled.',
  connection_refused: 'Connection refused: SSH may not be running, or the port is wrong.',
  auth_failed: 'Sign-in refused: Pisper signed in with a key; add your key to the machine.',
  unreachable: 'Machine unreachable: check the address, network and firewall.',
  connection_failed: 'SSH connection failed.',
  unsupported_remote:
    'The machine asks for more than a password (such as a one-time code); this is not supported. Use key sign-in.',
}

// 只看分类，不带 ssh 的原文：原文里有用户名、主机和对方的横幅，没有必要外传。
function sshFailure(code, { password = false } = {}) {
  // 密码机器的主机公钥和用户确认过的不一样：ssh 在发密码之前就断开了。要用户重新核对指纹，不能自动接受。
  if (code === 'host_key_changed' && password)
    return new ExecutorError(
      'host_key_changed',
      'This machine no longer presents the host key you confirmed, so the connection was refused before the password was sent. If the machine was reinstalled, confirm its new fingerprint in Settings.',
    )
  // 问密码的不是目标机（多半是跳板机）：密码没有交出去，说清楚怎么办。
  if (code === 'askpass_other_host')
    return new ExecutorError(
      'unsupported_remote',
      'Another host on the way (a jump host) asked for a password, or the prompt did not name this machine; the saved password was not sent. Use key sign-in for the jump host.',
    )
  if (code === 'auth_failed' && password)
    return new ExecutorError(
      'auth_failed',
      'Sign-in refused: wrong user name or password, or the machine does not accept password sign-in.',
    )
  return new ExecutorError(code, FAILURE_MESSAGES[code] || FAILURE_MESSAGES.connection_failed)
}

function collectStderr(child) {
  let stderr = ''
  child.stderr?.on('data', (chunk) => {
    if (stderr.length < MAX_STDERR_CHARS) stderr += chunk
  })
  return () => stderr.slice(0, MAX_STDERR_CHARS)
}

// ssh 退出后 stdout 可能还有没读完的数据，但不能等 close：连接复用时分出的后台进程可能一直握着管道。
// 结束时主动放开两条管道，不留下要等它退出才释放的句柄。
function whenFinished(child, done) {
  let exitCode = null
  let exited = false
  let stdoutEnded = false
  let finished = false
  let drainTimer
  const finish = () => {
    if (finished) return
    finished = true
    clearTimeout(drainTimer)
    child.stdout?.destroy()
    child.stderr?.destroy()
    done(exitCode)
  }
  child.stdout?.once('end', () => {
    stdoutEnded = true
    if (exited) finish()
  })
  child.once('exit', (code, signal) => {
    exited = true
    exitCode = code ?? (signal ? 128 : 1)
    if (stdoutEnded) finish()
    else drainTimer = setTimeout(finish, EXIT_DRAIN_MS)
  })
  child.once('close', () => {
    if (exited) finish()
  })
}

// 在 glob 里和执行节点同一套语义（packages/pisper/lib/executor-node.mjs）：npm 包不依赖 runtime，这里各留一份。
function globToRegExp(pattern) {
  let source = ''
  const text = String(pattern).replaceAll('\\', '/')
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index]
    if (char === '*') {
      if (text[index + 1] === '*') {
        const slash = text[index + 2] === '/'
        source += slash ? '(?:.*/)?' : '.*'
        index += slash ? 2 : 1
      } else source += '[^/]*'
    } else if (char === '?') source += '[^/]'
    else if (char === '{') {
      const end = text.indexOf('}', index)
      if (end < 0) source += '\\{'
      else {
        source += `(?:${text
          .slice(index + 1, end)
          .split(',')
          .map((part) => part.replace(/[.+^$()|[\]\\]/g, '\\$&').replaceAll('*', '[^/]*'))
          .join('|')})`
        index = end
      }
    } else source += char.replace(/[.+^$()|[\]\\]/g, '\\$&')
  }
  return new RegExp(`^${source}$`)
}

// 第一步探测：不知道对方的 shell 是什么，发一句三种 shell 都能执行的 echo，看变量怎么展开。
// cmd 只展开 %OS%，PowerShell 只展开 $PSHOME，POSIX shell（含 zsh、fish）只展开 $PSHOME 且它是空的。
export const SHELL_PROBE_COMMAND = `echo "${SHELL_MARKER}%OS%_$PSHOME"`

// 返回 { family: 'posix' | 'windows' | 'unsupported', loginShell } 或 null（没认出来，可能是 csh 之类，按 POSIX 再试）。
export function parseShellProbe(text) {
  const line = String(text)
    .split(/\r?\n/)
    .map((item) => item.replaceAll('"', '').trim())
    .find((item) => item.startsWith(SHELL_MARKER))
  if (!line) return null
  const rest = line.slice(SHELL_MARKER.length)
  if (rest.startsWith('Windows_NT_')) return { family: 'windows', loginShell: 'cmd' }
  if (!rest.startsWith('%OS%_')) return null
  const home = rest.slice('%OS%_'.length)
  if (!home) return { family: 'posix', loginShell: '' }
  // 登录 shell 是 Linux / macOS 上的 PowerShell：单引号脚本传不过去，明确说不支持。
  return /^[A-Za-z]:\\/.test(home)
    ? { family: 'windows', loginShell: 'powershell' }
    : { family: 'unsupported', loginShell: 'powershell' }
}

const POSIX_PROBE = [
  `printf '%s\\n' ${PROBE_MARKER}`,
  `printf 'os=%s\\n' "$(uname -s 2>/dev/null || echo unknown)"`,
  `printf 'home=%s\\n' "\${HOME:-}"`,
  `printf 'shell=%s\\n' "\${SHELL:-}"`,
  `for name in ${REQUIRED_COMMANDS.join(' ')}; do command -v "$name" >/dev/null 2>&1 || printf 'missing=%s\\n' "$name"; done`,
].join('; ')

// 第二步探测的输出：标记行之后是 key=value。没有标记时返回 null。
export function parseProbeOutput(text) {
  const lines = String(text).split(/\r?\n/)
  const start = lines.indexOf(PROBE_MARKER)
  if (start < 0) return null
  const result = { platform: 'unknown', version: '', home: '', shell: '', missing: [] }
  for (const line of lines.slice(start + 1)) {
    const at = line.indexOf('=')
    if (at < 0) continue
    const key = line.slice(0, at)
    const value = line.slice(at + 1).trim()
    if (key === 'missing') result.missing.push(value)
    else if (key === 'os') result.platform = value || 'unknown'
    else if (key === 'version' || key === 'home' || key === 'shell') result[key] = value
  }
  return result
}

function osKind(platform) {
  const text = String(platform)
  if (/^windows/i.test(text)) return 'windows'
  if (/^linux/i.test(text)) return 'linux'
  if (/^darwin/i.test(text)) return 'macos'
  return 'unix'
}

// 系统名和 shell 名是对方机器自己报上来的，会写进系统提示：只留认得的写法（一个词，或已知名字加版本号），
// 不让一台恶意的机器借这两个字段往提示词里塞句子。
const POSIX_SHELLS = new Set([
  'sh',
  'bash',
  'zsh',
  'fish',
  'dash',
  'ksh',
  'mksh',
  'ash',
  'csh',
  'tcsh',
  'busybox',
])
const OS_LABELS = { linux: 'Linux', macos: 'macOS', windows: 'Windows', unix: 'Unix' }
const versionIn = (text) => /\d+(?:\.\d+){0,3}/.exec(String(text ?? ''))?.[0] || ''

export function safeSystemName(os, raw) {
  const text = String(raw ?? '').trim()
  if (os === 'windows') {
    const version = versionIn(text)
    return version ? `Windows ${version}` : 'Windows'
  }
  // uname -s：Linux、Darwin、FreeBSD、CYGWIN_NT-10.0-19045 这样的一个词。
  return /^[A-Za-z][A-Za-z0-9_.-]{0,23}$/.test(text) ? text : OS_LABELS[os] || 'Unix'
}

export function safeShellName(os, raw) {
  const text = String(raw ?? '').trim()
  if (os === 'windows') {
    if (/^cmd/i.test(text)) return 'cmd.exe'
    const version = versionIn(text)
    return version ? `PowerShell ${version}` : 'PowerShell'
  }
  if (!text) return ''
  const name = posix.basename(text).toLowerCase()
  return POSIX_SHELLS.has(name) ? name : 'sh'
}

// 探测结果 → 记在机器记录上的 system。shell 是命令实际在哪个 shell 里跑：
// POSIX 是对方的登录 shell（$SHELL -lc），Windows 一律是 PowerShell（与登录 shell 是 cmd 还是 PowerShell 无关）。
export function systemFromProbe(probe, loginShell = '') {
  const os = osKind(probe.platform)
  const windows = os === 'windows'
  return {
    os,
    family: windows ? 'windows' : 'posix',
    name: safeSystemName(os, windows ? probe.version : probe.platform),
    shell: safeShellName(os, windows ? probe.shell : probe.shell || 'sh'),
    ...(windows ? { loginShell: loginShell === 'powershell' ? 'powershell' : 'cmd' } : {}),
  }
}

const SYSTEM_OS = new Set(['linux', 'macos', 'windows', 'unix'])

// 存盘的 system（只来自探测，读回时仍按不可信数据校验）。旧版本只支持 POSIX，
// 没有 system 但探测成功过的机器按 POSIX 处理，系统名取当时记下的 uname。
export function normalizeSystem(value, lastCheck = null) {
  if (value && typeof value === 'object' && SYSTEM_OS.has(value.os)) {
    const windows = value.os === 'windows'
    return {
      os: value.os,
      family: windows ? 'windows' : 'posix',
      name: safeSystemName(value.os, value.name),
      shell: safeShellName(value.os, value.shell),
      ...(windows ? { loginShell: value.loginShell === 'powershell' ? 'powershell' : 'cmd' } : {}),
    }
  }
  if (lastCheck?.ok === true && lastCheck.platform && osKind(lastCheck.platform) !== 'windows')
    return {
      os: osKind(lastCheck.platform),
      family: 'posix',
      name: safeSystemName(osKind(lastCheck.platform), lastCheck.platform),
      shell: '',
    }
  return null
}

// 每个操作在对方机器上怎么执行：返回发给对方 shell 的那一条命令和 stdin。纯函数，参数都已是对方机器上的路径。
// POSIX：sh -c 加单引号包好的脚本；Windows：固定的 PowerShell 引导命令，脚本和参数作为数据走 stdin。
export function remoteRequest(family, op, params = {}) {
  if (family === 'windows') {
    const script = POWERSHELL_SCRIPTS[op]
    if (!script) throw new Error(`Unknown remote operation: ${op}`)
    const { data, ...args } = params
    return {
      command: POWERSHELL_COMMAND,
      stdin: powershellPayload(script, args, data === undefined ? undefined : Buffer.from(data)),
    }
  }
  const script = posixScript(op, params)
  return {
    command: `sh -c ${shellQuote(script)}`,
    stdin: params.data === undefined ? null : Buffer.from(params.data),
  }
}

function posixScript(op, params) {
  const quoted = () => shellQuote(params.path)
  switch (op) {
    case 'probe':
      return POSIX_PROBE
    case 'resolveDirectory': {
      const text = String(params.path ?? '')
      const target =
        !text || text === '~'
          ? '"$HOME"'
          : text.startsWith('~/')
            ? `"$HOME"/${shellQuote(text.slice(2))}`
            : shellQuote(text)
      return `cd ${target} 2>/dev/null || exit ${EXIT_MISSING}; pwd`
    }
    case 'readFile':
      return `[ -e ${quoted()} ] || exit ${EXIT_MISSING}; [ -d ${quoted()} ] && exit ${EXIT_IS_DIRECTORY}; exec cat -- ${quoted()}`
    case 'access': {
      const checks = [
        `[ -e ${quoted()} ] || exit ${EXIT_MISSING}`,
        `[ -r ${quoted()} ] || exit ${EXIT_DENIED}`,
      ]
      if (params.write) checks.push(`[ -w ${quoted()} ] || exit ${EXIT_DENIED}`)
      return checks.join('; ')
    }
    case 'writeFile':
      return `cat > ${quoted()}`
    case 'mkdir':
      return `mkdir -p -- ${quoted()}`
    case 'kind':
      return `if [ -d ${quoted()} ]; then echo directory; elif [ -e ${quoted()} ]; then echo file; else echo none; fi`
    case 'readdir':
      return `[ -d ${quoted()} ] || exit ${EXIT_MISSING}; cd ${quoted()} || exit ${EXIT_DENIED}; LC_ALL=C exec ls -Ap1`
    case 'glob': {
      const prune = params.prune.includes('node_modules')
        ? '\\( -name .git -o -name node_modules \\)'
        : '-name .git'
      return `cd ${shellQuote(params.cwd)} || exit ${EXIT_MISSING}; find . ${prune} -prune -o -print 2>/dev/null | head -n ${Number(params.max)}`
    }
    case 'exec':
      // 没有 PTY：靠带随机串的标记知道命令何时结束和它的退出码；stdin 接 /dev/null。
      return (
        `printf '\\036${params.nonce}P%s\\036' "$$"; ` +
        `cd ${shellQuote(params.cwd)} 2>/dev/null || exit ${EXIT_NO_CWD}; ` +
        `"\${SHELL:-/bin/sh}" -lc ${shellQuote(params.command)} </dev/null 2>&1; rc=$?; ` +
        `printf '\\036${params.nonce}E%s\\036' "$rc"`
      )
    case 'kill': {
      // 结束对方的整个进程组：断开 ssh 不会结束没有终端的远程进程。Linux 读 /proc，macOS 等没有 /proc 时用 ps。
      const pid = Number(params.pid)
      return [
        `pg=$(cut -d' ' -f5 /proc/${pid}/stat 2>/dev/null)`,
        `[ -n "$pg" ] || pg=$(ps -o pgid= -p ${pid} 2>/dev/null | tr -d ' ')`,
        `if [ -n "$pg" ] && [ "$pg" != 0 ] && [ "$pg" != 1 ]; then kill -s TERM -- "-$pg" 2>/dev/null; else kill -s TERM -- ${pid} 2>/dev/null; fi`,
      ].join('; ')
    }
    default:
      throw new Error(`Unknown remote operation: ${op}`)
  }
}

// 每个 ssh 路径只问一次版本（只有 Windows 上的密码登录需要）。
const sshVersions = new Map()

// 连接复用的目录：socket 能让人直接用这条已登录的连接，目录必须只属于自己（0700，不是符号链接），否则不复用。
// 每次都检查：目录可能被清理掉，缺了目录 ssh 会直接以失败退出。
export function usableMuxDirectory(path) {
  try {
    mkdirSync(path, { recursive: true, mode: 0o700 })
    const info = lstatSync(path)
    return (
      info.isDirectory() &&
      !info.isSymbolicLink() &&
      (typeof process.getuid !== 'function' || info.uid === process.getuid()) &&
      (info.mode & 0o077) === 0
    )
  } catch {
    return false
  }
}

export class SshExecutor {
  constructor({
    machine,
    muxDirectory = '',
    extraArguments = [],
    platform = process.platform,
    spawnProcess = spawn,
    now = () => Date.now(),
    isAvailable = () => true,
    password = () => '',
    onSystem = null,
    sshPath = sshExecutable(platform),
    authBlocked = null,
    onAuthFailed = null,
    scratchDirectory = '',
  }) {
    this.machine = machine
    this.id = machine.id
    this.kind = 'ssh'
    this.name = machine.name
    this.capabilities = Object.freeze({ shell: true, browser: false })
    this.platform = platform
    this.spawnProcess = spawnProcess
    this.now = now
    this.extraArguments = extraArguments
    // 连接复用：Windows 版 OpenSSH 不支持；路径太长时 ssh 会直接报错，宁可每次新连。
    this.muxDirectory =
      platform !== 'win32' && muxDirectory && muxDirectory.length <= MAX_MUX_DIRECTORY_CHARS
        ? muxDirectory
        : ''
    this.listCache = new Map()
    this.isAvailable = isAvailable
    // 密码不放在 machine 上：machine 会被序列化、复制，密码只在要连的那一刻取一次。
    this.password = typeof password === 'function' ? password : () => ''
    this.onSystem = onSystem
    this.sshPath = sshPath
    // 没保存的机器（添加前测试）探测结果只记在这里；保存过的机器以机器记录为准。
    this.detectedSystem = null
    this.detecting = null
    // 密码被对方拒绝过：不再拿同一个密码去试（每个工具调用都会新开一次连接，几次就能把账户锁住或让
    // fail2ban 封掉这台电脑）。保存过的机器由登记表记着（所有会话共用，改密码或点测试后解除）；
    // 没保存的（添加前测试）只记在这个执行器上。
    this.authRefused = false
    this.authBlocked = typeof authBlocked === 'function' ? authBlocked : () => this.authRefused
    this.onAuthFailed =
      typeof onAuthFailed === 'function' ? onAuthFailed : () => (this.authRefused = true)
    this.loginPlan = null
    // 密码登录每次连接用的临时目录（空配置文件、只有一行的 known_hosts、密码通道的 socket）建在这下面；
    // 没给或太长（socket 路径有上限）时用系统的临时目录。目录由 mkdtemp 新建，只有自己能进。
    this.scratchDirectory = scratchDirectory
  }

  online() {
    return this.machine.lastCheck?.ok === true
  }

  // 每次都从机器记录读：编辑了地址（探测结果被清掉）后，驻留会话手里的执行器也会重新探测。
  get system() {
    return normalizeSystem(this.machine.system, this.machine.lastCheck) || this.detectedSystem
  }

  get family() {
    return this.system?.family || 'posix'
  }

  // 密码登录这一次要用的两样东西：用户确认过的主机公钥和密码。顺序有讲究：没确认过指纹、或密码被拒绝过，
  // 就连密码文件都不去读。密码只在这次调用里经手，不记在执行器上。
  async #credentials() {
    if (this.machine.auth !== 'password') return null
    const hostKey = normalizeHostKey(this.machine.hostKey)
    if (!hostKey)
      throw new ExecutorError(
        'host_key_unconfirmed',
        'Password sign-in needs a host fingerprint you have confirmed. Open this machine in Settings and confirm its fingerprint.',
      )
    if (this.authBlocked())
      throw new ExecutorError(
        'auth_failed',
        'The saved password was refused earlier, so Pisper stopped trying it. Update the password in Settings, or test the connection there.',
      )
    const password = await this.password()
    if (!password) throw new ExecutorError('auth_failed', 'No password is saved for this machine.')
    return { password, hostKey }
  }

  // 认证失败的那一次：记下来，之后不再用这个密码连。
  #failure(stderr, password) {
    const code = classifySshFailure(stderr)
    if (code === 'auth_failed' && password) this.onAuthFailed()
    return sshFailure(code, { password: Boolean(password) })
  }

  // 每次连接（或一次查询）用完就删的临时目录。
  #scratch() {
    const parent =
      this.scratchDirectory && this.scratchDirectory.length <= MAX_SCRATCH_PARENT_CHARS
        ? this.scratchDirectory
        : tmpdir()
    if (parent === this.scratchDirectory) mkdirSync(parent, { recursive: true, mode: 0o700 })
    const directory = mkdtempSync(join(parent, 'pw-'))
    const configFile = join(directory, 'config')
    writeFileSync(configFile, '', { mode: 0o600 })
    return {
      directory,
      configFile,
      knownHostsFile: join(directory, 'known_hosts'),
      remove: () => rmSync(directory, { recursive: true, force: true }),
    }
  }

  // 跑一次不连网的 ssh（-G、-V 这类查询），收它的 stdout。失败时给空串。
  #query(args) {
    return new Promise((resolve, reject) => {
      let child
      try {
        child = this.spawnProcess(this.sshPath, args, {
          stdio: ['pipe', 'pipe', 'pipe'],
          windowsHide: true,
        })
      } catch (error) {
        reject(error)
        return
      }
      let stdout = ''
      let settled = false
      const finish = (error, value) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        if (error) reject(error)
        else resolve(value)
      }
      const timer = setTimeout(() => {
        child.kill()
        finish(null, '')
      }, VERSION_TIMEOUT_MS)
      child.stdout?.on('data', (chunk) => (stdout += chunk))
      child.stderr?.on('data', () => {})
      child.on('error', (error) =>
        finish(error?.code === 'ENOENT' ? sshFailure('ssh_client_missing') : null, ''),
      )
      child.on('close', (code) => finish(null, code === 0 ? stdout : ''))
      child.stdin?.on('error', () => {})
      child.stdin?.end()
    })
  }

  // 密码登录前问一次这台电脑的 ssh：不读配置文件时这个地址实际连的是谁（助手只把密码给它）。
  // 再按“读配置”解析一遍对比：靠配置才连得上的地址（别名、跳板机）不能用密码登录，说清楚。
  // 两次查询都不带密码通道，查不出来就不连（宁可失败，不能把密码交给认不出的一方）。
  async #loginPlan() {
    const cached = this.loginPlan
    if (
      cached &&
      this.now() - cached.at < LOGIN_PLAN_TTL_MS &&
      cached.target === this.machine.target
    )
      return cached.plan
    const scratch = this.#scratch()
    let plan
    let configured
    try {
      const base = [
        ...(this.machine.port ? ['-p', String(this.machine.port)] : []),
        ...this.extraArguments,
      ]
      plan = parseSshConfig(
        await this.#query(['-G', ...base, '-F', scratch.configFile, '--', this.machine.target]),
      )
      configured = parseSshConfig(await this.#query(['-G', ...base, '--', this.machine.target]))
    } finally {
      scratch.remove()
    }
    if (!plan.user || !plan.hostname)
      throw new ExecutorError(
        'unsupported_remote',
        'Could not work out which host this machine is, so the saved password was not sent. Password sign-in needs OpenSSH 6.8 or later.',
      )
    if (dependsOnUserConfig(plan, configured))
      throw new ExecutorError(
        'unsupported_remote',
        'Password sign-in does not read the SSH configuration on this computer, and this address only works through it (an alias, a configured user name or port, or a jump host). Enter the real user@host and port, or use key sign-in for aliases and jump hosts.',
      )
    this.loginPlan = { at: this.now(), target: this.machine.target, plan }
    return plan
  }

  #arguments(scratch = null) {
    const muxDirectory =
      this.muxDirectory && usableMuxDirectory(this.muxDirectory) ? this.muxDirectory : ''
    return sshArguments(this.machine, {
      muxDirectory,
      extraArguments: this.extraArguments,
      password: Boolean(scratch),
      ...(scratch
        ? { configFile: scratch.configFile, knownHostsFile: scratch.knownHostsFile }
        : {}),
    })
  }

  // Windows 自带的 OpenSSH 太旧（Windows 10 是 7.7）时不会调用 askpass，会卡在等输入：先说清楚。
  async #checkClient(password) {
    if (!password || this.platform !== 'win32') return
    if (!sshVersions.has(this.sshPath))
      sshVersions.set(
        this.sshPath,
        new Promise((resolve) => {
          let output = ''
          let child
          try {
            child = this.spawnProcess(this.sshPath, ['-V'], {
              stdio: ['pipe', 'pipe', 'pipe'],
              windowsHide: true,
            })
          } catch {
            resolve(null)
            return
          }
          const timer = setTimeout(() => child.kill(), VERSION_TIMEOUT_MS)
          child.stdout?.on('data', (chunk) => (output += chunk))
          child.stderr?.on('data', (chunk) => (output += chunk))
          child.on('error', () => resolve(null))
          child.on('close', () => {
            clearTimeout(timer)
            resolve(parseSshVersion(output))
          })
          child.stdin?.end()
        }),
      )
    const version = await sshVersions.get(this.sshPath)
    if (version && !versionAtLeast(version, ASKPASS_REQUIRE_VERSION))
      throw new ExecutorError(
        'unsupported_remote',
        `Password sign-in needs OpenSSH ${ASKPASS_REQUIRE_VERSION.join('.')} or later on this computer (found ${version.join('.')}). Update OpenSSH or use key sign-in.`,
      )
  }

  // 准备好这一次连接，返回一个同步的“启动 ssh”函数：要等的事（开密码通道）都在这里做完，
  // 启动和挂监听之间不能再隔一个 await——找不到 ssh 时的 error 事件在下一拍就发出来，隔开就收不到了。
  async #connection(credentials, login) {
    // 驻留的会话还拿着这个执行器：机器移除后不能再连过去。
    if (!this.isAvailable()) throw new ExecutorError('machine_not_found', 'Machine was removed.')
    if (!credentials)
      return (remoteCommand) => {
        if (!this.isAvailable())
          throw new ExecutorError('machine_not_found', 'Machine was removed.')
        return this.spawnProcess(this.sshPath, [...this.#arguments(), remoteCommand], {
          stdio: ['pipe', 'pipe', 'pipe'],
          windowsHide: true,
        })
      }
    // 这一次连接专用：空配置文件、只有那一把公钥的 known_hosts、密码通道。ssh 退出就全部关掉、删掉。
    const scratch = this.#scratch()
    let channel = null
    const release = () => {
      channel?.close()
      scratch.remove()
    }
    try {
      writeFileSync(scratch.knownHostsFile, knownHostsLine(credentials.hostKey), { mode: 0o600 })
      // ssh 的密码提示里写的主机名是 HostKeyAlias；键盘交互方式下老版本不写来源，只有“Password:”——
      // 不读配置文件就没有跳板机，这句只可能来自目标机。
      channel = await openAskpassChannel({
        directory: scratch.directory,
        platform: this.platform,
        secret: credentials.password,
        expect: [`${login.user}@${HOST_KEY_ALIAS}`, `${login.user}@${login.hostname}`],
        bare: true,
      })
      // 密码已经交给通道（它答过一次就放掉）：这边不再留着。
      credentials.password = ''
    } catch (error) {
      release()
      throw error
    }
    return (remoteCommand) => {
      try {
        const child = this.spawnProcess(
          this.sshPath,
          [...this.#arguments(scratch), remoteCommand],
          {
            stdio: ['pipe', 'pipe', 'pipe'],
            windowsHide: true,
            env: askpassEnvironment(channel, { platform: this.platform }),
            detached: this.platform !== 'win32',
          },
        )
        child.once('exit', release)
        child.once('error', release)
        return child
      } catch (error) {
        release()
        throw error
      }
    }
  }

  // 取这台机器的主机公钥给用户核对：只握手，不带任何凭据，也不读配置文件（见 hostKeyArguments）。
  // 只有握手完整走完（对方证明了它持有这把私钥，然后因为没有可用的登录方式被拒绝）才算数。
  async hostKey() {
    const scratch = this.#scratch()
    try {
      const args = hostKeyArguments(this.machine, {
        configFile: scratch.configFile,
        knownHostsFile: scratch.knownHostsFile,
        extraArguments: this.extraArguments,
      })
      const outcome = await new Promise((resolve, reject) => {
        let child
        try {
          child = this.spawnProcess(this.sshPath, args, {
            stdio: ['pipe', 'pipe', 'pipe'],
            windowsHide: true,
          })
        } catch (error) {
          reject(error)
          return
        }
        const stderr = collectStderr(child)
        let settled = false
        const finish = (error, value) => {
          if (settled) return
          settled = true
          clearTimeout(timer)
          child.stdout?.destroy()
          child.stderr?.destroy()
          if (error) reject(error)
          else resolve(value)
        }
        const timer = setTimeout(() => {
          child.kill()
          finish(new ExecutorError('ssh_timeout', 'Machine did not respond in time.'))
        }, HOST_KEY_TIMEOUT_MS)
        child.on('error', (error) =>
          finish(error?.code === 'ENOENT' ? sshFailure('ssh_client_missing') : error),
        )
        child.stdout?.on('data', () => {})
        whenFinished(child, (code) => finish(null, { code, stderr: stderr() }))
        child.stdin?.on('error', () => {})
        child.stdin?.end()
      })
      const failure = outcome.code === 0 ? '' : classifySshFailure(outcome.stderr)
      // 退出码 0：对方不要求登录就放行了（少见），握手同样完整。认证被拒是预期的结果。
      if (failure && failure !== 'auth_failed') throw sshFailure(failure)
      let recorded = ''
      try {
        recorded = readFileSync(scratch.knownHostsFile, 'utf8')
      } catch {}
      const hostKey = parseKnownHosts(recorded)
      if (!hostKey)
        throw new ExecutorError(
          'connection_failed',
          'Could not read the host key of this machine. Fetching it needs OpenSSH 7.6 or later on this computer.',
        )
      return hostKey
    } finally {
      scratch.remove()
    }
  }

  // 跑一条远程命令，结果整段收回来。maxBytes 防止把巨大的文件读进内存。
  async #run(request, { timeoutMs = OPERATION_TIMEOUT_MS, maxBytes = 0 } = {}) {
    const credentials = await this.#credentials()
    const password = Boolean(credentials)
    await this.#checkClient(password)
    const login = credentials ? await this.#loginPlan() : null
    const connect = await this.#connection(credentials, login)
    return new Promise((resolve, reject) => {
      let child
      try {
        child = connect(request.command)
      } catch (error) {
        reject(error)
        return
      }
      const chunks = []
      let size = 0
      let overflow = false
      let settled = false
      const stderr = collectStderr(child)
      const timer = setTimeout(() => {
        finish(new ExecutorError('ssh_timeout', 'Machine did not respond in time.'))
        child.kill()
      }, timeoutMs)
      const finish = (error, exitCode) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        if (error) {
          child.stdout?.destroy()
          child.stderr?.destroy()
          reject(error)
          return
        }
        if (exitCode === EXIT_SSH_FAILED) {
          reject(this.#failure(stderr(), password))
          return
        }
        resolve({ code: exitCode, stdout: Buffer.concat(chunks), stderr: stderr(), overflow })
      }
      child.on('error', (error) =>
        finish(error?.code === 'ENOENT' ? sshFailure('ssh_client_missing') : error),
      )
      child.stdout.on('data', (chunk) => {
        size += chunk.length
        if (maxBytes && size > maxBytes) {
          overflow = true
          child.kill()
          return
        }
        chunks.push(chunk)
      })
      whenFinished(child, (exitCode) => finish(null, exitCode))
      child.stdin.on('error', () => {})
      child.stdin.end(request.stdin ?? undefined)
    })
  }

  // 对方是什么系统、命令在哪个 shell 里跑：第一次用到时探测一次并记下，之后不再问。
  async #ensureSystem() {
    if (this.system) return this.system
    if (!this.detecting)
      this.detecting = this.#detect().finally(() => {
        this.detecting = null
      })
    await this.detecting
    return this.system
  }

  async #call(op, params, options) {
    const { family } = await this.#ensureSystem()
    return this.#run(remoteRequest(family, op, params), options)
  }

  #path(value) {
    return toRemotePath(value, this.platform, this.family)
  }

  #fail(result, path, what) {
    const shown = this.#path(path)
    if (result.code === EXIT_MISSING)
      throw new ExecutorError('not_found', `ENOENT: no such file or directory, ${what} '${shown}'`)
    if (result.code === EXIT_IS_DIRECTORY)
      throw new ExecutorError(
        'is_directory',
        `EISDIR: illegal operation on a directory, ${what} '${shown}'`,
      )
    if (result.code === EXIT_DENIED)
      throw new ExecutorError('permission_denied', `EACCES: permission denied, ${what} '${shown}'`)
    const detail = result.stderr.trim().slice(0, 300)
    throw new ExecutorError('remote_failed', detail || `${what} '${shown}' failed.`)
  }

  // 真实连上去跑两步：先看登录 shell 是哪一类，再按那一类跑探测脚本，有 shell、命令齐全才算能用。
  async #detect() {
    const login = parseShellProbe(
      (await this.#run({ command: SHELL_PROBE_COMMAND }, { timeoutMs: PROBE_TIMEOUT_MS })).stdout,
    )
    if (login?.family === 'unsupported')
      throw new ExecutorError(
        'unsupported_remote',
        'The account logs in to PowerShell on a non-Windows system; set a POSIX login shell.',
      )
    const family = login?.family || 'posix'
    const result = await this.#run(remoteRequest(family, 'probe'), { timeoutMs: PROBE_TIMEOUT_MS })
    const probe = parseProbeOutput(result.stdout.toString('utf8'))
    if (!probe)
      throw new ExecutorError(
        'unsupported_remote',
        family === 'windows'
          ? 'Windows PowerShell could not run on the machine (it may be removed or restricted).'
          : 'No POSIX shell on the machine (restricted accounts are not supported).',
      )
    if (probe.missing.length)
      throw new ExecutorError(
        'unsupported_remote',
        `Missing commands on the machine: ${probe.missing.join(', ')}.`,
      )
    const system = systemFromProbe(probe, login?.loginShell)
    this.detectedSystem = system
    await this.onSystem?.(system)
    return { platform: probe.platform, home: probe.home, system }
  }

  // 测试连接、重新检查：每次都真实探测，结果覆盖记下的系统。
  async probe() {
    const { platform, home, system } = await this.#detect()
    return { platform, home, os: system.os, shell: system.shell }
  }

  // 目录解析成绝对路径：~ 和空值都从对方的家目录算起。目录不存在时明确报错，不等到第一个工具才失败。
  async resolveDirectory(input) {
    const text = String(input ?? '').trim()
    const { family } = await this.#ensureSystem()
    const path = text && text !== '~' && !text.startsWith('~/') ? this.#path(text) : text
    const result = await this.#run(remoteRequest(family, 'resolveDirectory', { path }))
    if (result.code === EXIT_MISSING)
      throw new ExecutorError(
        'machine_folder_missing',
        `Folder not found on the machine: ${text || '~'}`,
      )
    if (result.code !== 0) this.#fail(result, text, 'cd')
    return result.stdout.toString('utf8').trim()
  }

  async #readFile(path) {
    await this.#ensureSystem()
    const result = await this.#call(
      'readFile',
      { path: this.#path(path) },
      { maxBytes: MAX_READ_BYTES },
    )
    if (result.overflow) throw new ExecutorError('too_large', 'File is too large to read.')
    if (result.code !== 0) this.#fail(result, path, 'open')
    return result.stdout
  }

  async #access(path, mode) {
    await this.#ensureSystem()
    const result = await this.#call('access', { path: this.#path(path), write: mode === 'rw' })
    if (result.code !== 0) this.#fail(result, path, 'access')
  }

  async #writeFile(path, content) {
    await this.#ensureSystem()
    this.listCache.clear()
    const result = await this.#call('writeFile', {
      path: this.#path(path),
      data: Buffer.isBuffer(content) ? content : String(content),
    })
    if (result.code !== 0) this.#fail(result, path, 'open')
  }

  async #mkdir(path) {
    await this.#ensureSystem()
    this.listCache.clear()
    const result = await this.#call('mkdir', { path: this.#path(path) })
    if (result.code !== 0) this.#fail(result, path, 'mkdir')
  }

  // 列目录缓存的键：Windows 路径不分大小写、两种分隔符都算。
  #cacheKey(path) {
    return this.family === 'windows' ? win32.normalize(path).toLowerCase() : path
  }

  // 引擎的 ls 工具对每一项都查一次 stat：列目录时顺带记下类型，免得几百次连接。
  #cached(path) {
    const key = this.#cacheKey(path)
    const entry = this.listCache.get(key)
    if (!entry) return null
    if (this.now() - entry.at > LIST_CACHE_TTL_MS) {
      this.listCache.delete(key)
      return null
    }
    return entry
  }

  async #kind(path) {
    await this.#ensureSystem()
    const remote = this.#path(path)
    const cached = this.#cached(remote)
    if (cached) return cached.directory ? 'directory' : 'file'
    const result = await this.#call('kind', { path: remote })
    if (result.code !== 0) this.#fail(result, path, 'stat')
    return result.stdout.toString('utf8').trim()
  }

  async #readdir(path) {
    await this.#ensureSystem()
    const remote = this.#path(path)
    const result = await this.#call('readdir', { path: remote })
    if (result.code !== 0) this.#fail(result, path, 'scandir')
    const at = this.now()
    const join = this.family === 'windows' ? win32.join : posix.join
    return result.stdout
      .toString('utf8')
      .split(/\r?\n/)
      .filter(Boolean)
      .map((line) => {
        const directory = line.endsWith('/')
        const name = directory ? line.slice(0, -1) : line
        this.listCache.set(this.#cacheKey(join(remote, name)), { directory, at })
        return name
      })
  }

  async #glob(pattern, cwd, { ignore = [], limit = 1000 } = {}) {
    await this.#ensureSystem()
    const matcher = globToRegExp(pattern)
    const ignored = (Array.isArray(ignore) ? ignore : []).map(globToRegExp)
    const max = Math.min(Math.max(1, Number(limit) || 1000), MAX_GLOB_RESULTS)
    // 依赖目录动辄几十万个文件：除非明确在找它，否则不进去。
    const prune = String(pattern).includes('node_modules') ? ['.git'] : ['.git', 'node_modules']
    const result = await this.#call('glob', {
      cwd: this.#path(cwd),
      prune,
      max: MAX_GLOB_ENTRIES,
    })
    if (result.code === EXIT_MISSING) this.#fail(result, cwd, 'scandir')
    const found = []
    for (const line of result.stdout.toString('utf8').split(/\r?\n/)) {
      if (found.length >= max) break
      if (!line || line === '.') continue
      const relative = line.replace(/^\.\//, '')
      if (ignored.some((rule) => rule.test(relative) || rule.test(`${relative}/`))) continue
      if (matcher.test(relative)) found.push(relative)
    }
    return found
  }

  // 结束对方的命令：POSIX 结束进程组，Windows 用 taskkill /T 结束整棵进程树。
  #killRemote(pid) {
    if (!Number.isSafeInteger(pid) || pid <= 1) return Promise.resolve()
    return this.#call('kill', { pid }, { timeoutMs: KILL_TIMEOUT_MS }).catch(() => {})
  }

  // 没有 PTY：靠带随机串的标记知道命令何时结束和它的退出码。命令一结束就断开，
  // 留在后台的进程（npm run dev &）继续跑，也不会因为它还握着管道而把工具卡住，和本机行为一致。
  // 标记不给模型看；命令读 stdin 立刻读到结尾，sudo 这类要输入的命令会失败，不会一直挂着。
  async #exec(command, cwd, { onData, signal, timeout } = {}) {
    if (signal?.aborted) throw new Error('aborted')
    const { family } = await this.#ensureSystem()
    const credentials = await this.#credentials()
    const password = Boolean(credentials)
    await this.#checkClient(password)
    const login = credentials ? await this.#loginPlan() : null
    const nonce = randomBytes(8).toString('hex')
    const remoteCwd = this.#path(cwd)
    const request =
      family === 'windows'
        ? remoteRequest(family, 'exec', { cwd: remoteCwd, nonce, data: String(command) })
        : remoteRequest(family, 'exec', { cwd: remoteCwd, nonce, command: String(command) })
    if (signal?.aborted) throw new Error('aborted')
    const connect = await this.#connection(credentials, login)
    return new Promise((resolve, reject) => {
      const separator = Buffer.from('\u001e', 'utf8')
      const prefix = Buffer.from(`\u001e${nonce}`, 'utf8')
      let child
      try {
        child = connect(request.command)
      } catch (error) {
        reject(error)
        return
      }
      const stderr = collectStderr(child)
      // Windows 上命令的 stderr 走 ssh 的 stderr 通道（POSIX 那边已经 2>&1 合进 stdout）：一样交给模型看。
      if (family === 'windows') child.stderr?.on('data', (chunk) => onData?.(chunk))
      let pending = Buffer.alloc(0)
      let remotePid = 0
      let markerExit = null
      let settled = false
      let stopping = ''
      let timer
      const cleanup = () => {
        clearTimeout(timer)
        signal?.removeEventListener('abort', onAbort)
      }
      const finish = (error, value) => {
        if (settled) return
        settled = true
        cleanup()
        child.stdout?.destroy()
        child.stderr?.destroy()
        if (error) reject(error)
        else resolve(value)
      }
      const stop = (reason) => {
        if (stopping || settled) return
        stopping = reason
        const killed = remotePid ? this.#killRemote(remotePid) : Promise.resolve()
        void killed.finally(() => {
          child.kill()
          finish(new Error(reason === 'timeout' ? `timeout:${timeout}` : 'aborted'))
        })
      }
      const onAbort = () => stop('abort')
      // 标记可能被切在两个数据块之间：留够一个标记长度不发出去，下一块来了再看。
      const consume = (chunk) => {
        pending = pending.length ? Buffer.concat([pending, chunk]) : chunk
        for (;;) {
          const start = pending.indexOf(prefix)
          if (start < 0) break
          const end = pending.indexOf(separator, start + prefix.length)
          if (end < 0) break
          if (start > 0) onData?.(pending.subarray(0, start))
          const body = pending.subarray(start + prefix.length, end).toString('utf8')
          pending = pending.subarray(end + separator.length)
          if (body[0] === 'P') remotePid = Number(body.slice(1)) || 0
          else if (body[0] === 'E') {
            markerExit = Number(body.slice(1))
            child.kill()
            finish(null, { exitCode: Number.isInteger(markerExit) ? markerExit : null })
            return
          }
        }
        // 只扣下最后一个分隔符起的那一小段（可能是没收全的标记），其余立刻交出去：
        // Windows 上 stderr 另走一路，扣得太多会让两路输出错位。
        const last = pending.lastIndexOf(separator)
        const ready =
          last >= 0 && pending.length - last <= prefix.length + 24 ? last : pending.length
        if (ready > 0) {
          onData?.(pending.subarray(0, ready))
          pending = pending.subarray(ready)
        }
      }
      if (timeout) timer = setTimeout(() => stop('timeout'), Math.max(1, Number(timeout)) * 1000)
      signal?.addEventListener('abort', onAbort, { once: true })
      // 等密码通道的那一会儿里已经被取消了：监听不会再触发，这里补上。
      if (signal?.aborted) onAbort()
      child.on('error', (error) =>
        finish(error?.code === 'ENOENT' ? sshFailure('ssh_client_missing') : error),
      )
      child.stdout.on('data', consume)
      whenFinished(child, (code) => {
        if (settled || stopping) return
        if (pending.length) onData?.(pending)
        if (code === EXIT_NO_CWD)
          finish(new Error(`Working directory does not exist: ${remoteCwd}`))
        else if (code === EXIT_SSH_FAILED) finish(this.#failure(stderr(), password))
        // 没等到结束标记就断了：命令是否跑完不知道，不能当成成功。
        else
          finish(
            new ExecutorError(
              'ssh_disconnected',
              'Connection to the machine dropped; the command may not have finished.',
            ),
          )
      })
      child.stdin.on('error', () => {})
      child.stdin.end(request.stdin ?? undefined)
    })
  }

  operations() {
    const readFile = (path) => this.#readFile(path)
    const stat = async (path) => {
      const kind = await this.#kind(path)
      if (kind === 'none')
        throw new ExecutorError('not_found', `ENOENT: no such file or directory, stat '${path}'`)
      return { isDirectory: () => kind === 'directory' }
    }
    const exists = async (path) => (await this.#kind(path)) !== 'none'
    return {
      read: { readFile, access: (path) => this.#access(path, 'r') },
      edit: {
        readFile,
        writeFile: (path, content) => this.#writeFile(path, content),
        access: (path) => this.#access(path, 'rw'),
      },
      write: {
        writeFile: (path, content) => this.#writeFile(path, content),
        mkdir: (path) => this.#mkdir(path),
      },
      ls: { exists, stat, readdir: (path) => this.#readdir(path) },
      find: { exists, glob: (pattern, cwd, options) => this.#glob(pattern, cwd, options) },
      bash: { exec: (command, cwd, options) => this.#exec(command, cwd, options) },
    }
  }
}
