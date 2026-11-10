// 对方是 Windows 时 SSH 执行器发过去的东西。Windows 的 OpenSSH 服务默认用 cmd.exe（也可能配成 PowerShell）
// 解析远程命令，两者的引号规则和 POSIX 完全不同。所以远程命令永远是同一条固定的
// `powershell … -EncodedCommand <引导脚本>`：base64 里没有任何一种 shell 会解释的字符，cmd 和 PowerShell 读到的一样。
// 真正要跑的脚本、参数（JSON）和数据都从 stdin 送过去，不进命令行：没有拼接，没有 8191 字符的长度上限，
// 文件内容按原始字节往返（UTF-8、二进制、CRLF 都不变）。脚本是写死的常量，路径等参数只作为 JSON 数据读入。
// 纯常量和纯函数，无 I/O。

// 引导脚本：读完 stdin，第一行是 base64 的脚本，第二行是 base64 的 JSON 参数，其后是原始数据。
// 文件不存在 / 没有权限的异常换成和 POSIX 那边相同的退出码，其余异常只回一行英文原因。
const BOOTSTRAP = `$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$u = New-Object Text.UTF8Encoding $false
$m = New-Object IO.MemoryStream
[Console]::OpenStandardInput().CopyTo($m)
$b = $m.ToArray()
$x = [Array]::IndexOf($b, [byte]10)
$y = -1
if ($x -ge 0) { $y = [Array]::IndexOf($b, [byte]10, $x + 1) }
if ($y -lt 0) { [Console]::Error.WriteLine('Bad request.'); exit 2 }
$s = $u.GetString([Convert]::FromBase64String($u.GetString($b, 0, $x)))
$a = $u.GetString([Convert]::FromBase64String($u.GetString($b, $x + 1, $y - $x - 1))) | ConvertFrom-Json
$d = New-Object byte[] ($b.Length - $y - 1)
[Array]::Copy($b, $y + 1, $d, 0, $d.Length)
$o = [Console]::OpenStandardOutput()
function W([string]$t) { $z = $u.GetBytes($t); $o.Write($z, 0, $z.Length); $o.Flush() }
try { . ([ScriptBlock]::Create($s)) } catch {
  $e = $_.Exception
  while ($e.InnerException) { $e = $e.InnerException }
  if ($e -is [UnauthorizedAccessException]) { exit 46 }
  if ($e -is [IO.FileNotFoundException] -or $e -is [IO.DirectoryNotFoundException]) { exit 44 }
  [Console]::Error.WriteLine($e.Message)
  exit 1
}
exit 0
`

// 模型的命令在单独的 PowerShell 子进程里跑：命令里的 exit 只结束它自己，引导脚本还能写出结束标记。
// 命令文本从 stdin 读（读完 stdin 就到头了，要输入的命令立刻读到结尾，不会挂着）。
// 输出尽量是 UTF-8：没有控制台时设置会失败，忽略。-ExecutionPolicy Bypass 只作用于这个进程，
// 让 npm.ps1 这类脚本垫片能用。
const RUNNER = `$ProgressPreference = 'SilentlyContinue'
$u = New-Object Text.UTF8Encoding $false
try { [Console]::OutputEncoding = $u } catch {}
$OutputEncoding = $u
$m = New-Object IO.MemoryStream
[Console]::OpenStandardInput().CopyTo($m)
$s = $u.GetString($m.ToArray())
$global:LASTEXITCODE = 0
try { . ([ScriptBlock]::Create($s)) } catch { [Console]::Error.WriteLine($_.ToString()); exit 1 }
exit $global:LASTEXITCODE
`

// -EncodedCommand 要的是 UTF-16LE 的 base64。
export function encodePowerShell(script) {
  return Buffer.from(String(script), 'utf16le').toString('base64')
}

// 发给对方 shell 的那一条命令：每次都一样。
export const POWERSHELL_COMMAND = `powershell -NoProfile -NonInteractive -EncodedCommand ${encodePowerShell(BOOTSTRAP)}`

const RUNNER_ARGUMENTS = `-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ${encodePowerShell(RUNNER)}`

// 每个操作一段脚本；$a 是参数，$d 是原始数据，W 写 UTF-8 文本到 stdout，$o 是原始 stdout。
export const POWERSHELL_SCRIPTS = Object.freeze({
  probe: `W ("PISPER_SSH_PROBE\`nos=Windows\`nversion=" + [Environment]::OSVersion.Version + "\`nhome=" + $env:USERPROFILE + "\`nshell=PowerShell " + $PSVersionTable.PSVersion + "\`n")`,
  resolveDirectory: `$p = [string]$a.path
$h = $env:USERPROFILE
if (-not $p -or $p -eq '~') { $p = $h } elseif ($p -like '~/*' -or $p -like '~\\*') { $p = Join-Path $h $p.Substring(2) }
if (-not [IO.Directory]::Exists($p)) { exit 44 }
W ([IO.Path]::GetFullPath($p))`,
  readFile: `$p = [string]$a.path
if ([IO.Directory]::Exists($p)) { exit 45 }
if (-not [IO.File]::Exists($p)) { exit 44 }
$f = [IO.File]::OpenRead($p)
try { $f.CopyTo($o) } finally { $f.Dispose() }
$o.Flush()`,
  access: `$p = [string]$a.path
$dir = [IO.Directory]::Exists($p)
if (-not $dir -and -not [IO.File]::Exists($p)) { exit 44 }
if (-not $dir) {
  $mode = [IO.FileAccess]::Read
  if ($a.write) { $mode = [IO.FileAccess]::ReadWrite }
  try { ([IO.File]::Open($p, [IO.FileMode]::Open, $mode, [IO.FileShare]::ReadWrite)).Dispose() } catch { exit 46 }
}`,
  writeFile: `[IO.File]::WriteAllBytes([string]$a.path, $d)`,
  mkdir: `[void][IO.Directory]::CreateDirectory([string]$a.path)`,
  kind: `$p = [string]$a.path
if ([IO.Directory]::Exists($p)) { W 'directory' } elseif ([IO.File]::Exists($p)) { W 'file' } else { W 'none' }`,
  readdir: `$p = [string]$a.path
if (-not [IO.Directory]::Exists($p)) { exit 44 }
$names = New-Object Collections.Generic.List[string]
foreach ($e in (New-Object IO.DirectoryInfo $p).GetFileSystemInfos()) {
  if ($e.Attributes -band [IO.FileAttributes]::Directory) { $names.Add($e.Name + '/') } else { $names.Add($e.Name) }
}
$names.Sort([StringComparer]::Ordinal)
W (($names -join "\`n") + "\`n")`,
  // 和 POSIX 的 find 一样输出相对路径（用 / 分隔），跳过 .git 等目录，不跟进联接点（防止循环）。
  glob: `$root = [string]$a.cwd
if (-not [IO.Directory]::Exists($root)) { exit 44 }
$skip = @($a.prune)
$max = [int]$a.max
$out = New-Object Text.StringBuilder
$count = 0
$stack = New-Object Collections.Stack
$stack.Push(@((New-Object IO.DirectoryInfo $root), ''))
while ($stack.Count -gt 0 -and $count -lt $max) {
  $item = $stack.Pop()
  $entries = @()
  try { $entries = $item[0].GetFileSystemInfos() } catch { $entries = @() }
  foreach ($e in $entries) {
    if ($count -ge $max) { break }
    $isDir = ($e.Attributes -band [IO.FileAttributes]::Directory) -ne 0
    if ($isDir -and $skip -contains $e.Name) { continue }
    $rel = $e.Name
    if ($item[1]) { $rel = $item[1] + '/' + $e.Name }
    [void]$out.Append($rel).Append("\`n")
    $count++
    if ($isDir -and -not ($e.Attributes -band [IO.FileAttributes]::ReparsePoint)) { $stack.Push(@($e, $rel)) }
  }
}
W $out.ToString()`,
  // 先报引导进程的 PID（停止时 taskkill /T 按它结束整棵进程树），再跑命令，最后写结束标记和退出码。
  // 子进程继承这里的 stdout / stderr，输出原样流回去。
  exec: `$cwd = [string]$a.cwd
if (-not [IO.Directory]::Exists($cwd)) { exit 97 }
W ([string][char]30 + $a.nonce + 'P' + $PID + [char]30)
$psi = New-Object Diagnostics.ProcessStartInfo
$psi.FileName = [Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
$psi.Arguments = '${RUNNER_ARGUMENTS}'
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.WorkingDirectory = $cwd
$p = [Diagnostics.Process]::Start($psi)
$p.StandardInput.BaseStream.Write($d, 0, $d.Length)
$p.StandardInput.Close()
$p.WaitForExit()
W ([string][char]30 + $a.nonce + 'E' + $p.ExitCode + [char]30)`,
  kill: `& (Join-Path $env:SystemRoot 'System32\\taskkill.exe') /PID ([int]$a.pid) /T /F | Out-Null`,
})

// stdin 的内容：脚本和参数各一行 base64，然后是原始数据。
export function powershellPayload(script, args = {}, data = Buffer.alloc(0)) {
  const head = `${Buffer.from(script, 'utf8').toString('base64')}\n${Buffer.from(JSON.stringify(args), 'utf8').toString('base64')}\n`
  return Buffer.concat([
    Buffer.from(head, 'ascii'),
    Buffer.isBuffer(data) ? data : Buffer.from(data),
  ])
}

// 拆开 stdin 内容（测试和排查用）：和引导脚本的读法一致。
export function decodePowerShellPayload(payload) {
  const first = payload.indexOf(10)
  const second = payload.indexOf(10, first + 1)
  return {
    script: Buffer.from(payload.subarray(0, first).toString('ascii'), 'base64').toString('utf8'),
    args: JSON.parse(
      Buffer.from(payload.subarray(first + 1, second).toString('ascii'), 'base64').toString('utf8'),
    ),
    data: payload.subarray(second + 1),
  }
}
