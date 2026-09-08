// 防火墙只管理本实例的规则；所有系统调用可注入，测试不会触碰宿主策略。
import { execFile } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { basename, dirname, join, resolve } from 'node:path'
import { isSea } from 'node:sea'

const MAC_FIREWALL = '/usr/libexec/ApplicationFirewall/socketfilterfw'
const POWERSHELL = 'C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe'
const shellQuote = (value) => `'${String(value).replaceAll("'", "'\\''")}'`
const psQuote = (value) => `'${String(value).replaceAll("'", "''")}'`
const encodedPowerShell = (script) => Buffer.from(script, 'utf16le').toString('base64')
const sidecarPath = (value) => /^pisper-sidecar(?:-[a-zA-Z0-9_.-]+)?$/.test(basename(value))

// 退出码由受控脚本定义，不依赖系统本地化的成功提示来判断是否已经放行。
export function executeFirewallCommand({ file, args, timeout = 15_000 }) {
  return new Promise((resolveResult) => {
    execFile(
      file,
      args,
      { timeout, maxBuffer: 128 * 1024, windowsHide: true },
      (error, stdout, stderr) => {
        resolveResult({
          code: error ? (typeof error.code === 'number' ? error.code : -1) : 0,
          stdout: String(stdout || ''),
          stderr: String(stderr || ''),
          missing: error?.code === 'ENOENT',
          timedOut: Boolean(error?.killed),
        })
      },
    )
  })
}

function commandFor(platform, script, elevated) {
  if (platform === 'win32') {
    const encoded = encodedPowerShell(script)
    const command = elevated
      ? `$ErrorActionPreference = 'Stop'; try { $p = Start-Process -FilePath ${psQuote(POWERSHELL)} -Verb RunAs -Wait -PassThru -ArgumentList @('-NoProfile','-NonInteractive','-EncodedCommand',${psQuote(encoded)}); exit $p.ExitCode } catch { if ($_.Exception.NativeErrorCode -eq 1223 -or $_.Exception.InnerException.NativeErrorCode -eq 1223) { exit 80 }; exit 77 }`
      : script
    return {
      file: POWERSHELL,
      args: ['-NoProfile', '-NonInteractive', '-EncodedCommand', encodedPowerShell(command)],
      timeout: elevated ? 120_000 : 15_000,
    }
  }
  if (elevated && platform === 'darwin') {
    return {
      file: '/usr/bin/osascript',
      args: [
        '-e',
        `do shell script ${JSON.stringify(script)} with administrator privileges with prompt "Pisper remote access firewall"`,
      ],
      timeout: 120_000,
    }
  }
  if (elevated) {
    return { file: '/usr/bin/pkexec', args: ['/bin/sh', '-c', script], timeout: 120_000 }
  }
  return { file: '/bin/sh', args: ['-c', script], timeout: 15_000 }
}

function windowsScript(target, operation, owner) {
  const name = psQuote(`Pisper-Remote-${owner}`)
  const program = psQuote(target.execPath)
  const port = target.port
  const inspect = `
$r = Get-NetFirewallRule -PolicyStore ActiveStore -Name ${name} -ErrorAction SilentlyContinue
if (!$r) { exit 12 }
$p = $r | Get-NetFirewallPortFilter
$a = $r | Get-NetFirewallApplicationFilter
$s = $r | Get-NetFirewallAddressFilter
if (@($r).Count -ne 1 -or $r.Enabled -ne 'True' -or $r.Direction -ne 'Inbound' -or $r.Action -ne 'Allow' -or $p.Protocol -ne 'TCP' -or "$($p.LocalPort)" -ne '${port}' -or $a.Program -ne ${program} -or "$($s.RemoteAddress)" -ne 'LocalSubnet' -or "$($r.Profile)" -ne 'Any') { exit 12 }
exit 0`
  return `$ErrorActionPreference = 'Stop'
try {
${
  operation === 'remove'
    ? `Get-NetFirewallRule -PolicyStore PersistentStore -Name ${name} -ErrorAction SilentlyContinue | Remove-NetFirewallRule
if (Get-NetFirewallRule -PolicyStore ActiveStore -Name ${name} -ErrorAction SilentlyContinue) { exit 12 }
exit 0`
    : `${
        operation === 'apply'
          ? `Get-NetFirewallRule -PolicyStore PersistentStore -Name ${name} -ErrorAction SilentlyContinue | Remove-NetFirewallRule
New-NetFirewallRule -PolicyStore PersistentStore -Name ${name} -DisplayName 'Pisper remote HTTPS' -Group 'Pisper remote access' -Direction Inbound -Action Allow -Enabled True -Profile Any -Protocol TCP -LocalPort ${port} -Program ${program} -RemoteAddress LocalSubnet | Out-Null`
          : ''
      }
${inspect}`
}
} catch { exit 77 }`
}

function macScript(target, operation) {
  const app = shellQuote(target.execPath)
  const fw = shellQuote(MAC_FIREWALL)
  const inspect = `state=$(${fw} --getappblocked ${app} 2>&1) || exit 77
case "$state" in
  *'is not part of the firewall'*) exit 12 ;;
  *'Block incoming connections'*|*' is blocked'*) exit 12 ;;
  *'Allow incoming connections'*|*' is permitted'*) exit 0 ;;
  *) exit 78 ;;
esac`
  const restore =
    target.previousAppState === 'blocked'
      ? `state=$(${fw} --getappblocked ${app} 2>&1)
case "$state" in *'Block incoming connections'*|*' is blocked'*) exit 0 ;; esac
${fw} --blockapp ${app} >/dev/null || exit 77
state=$(${fw} --getappblocked ${app}) || exit 77
case "$state" in *'Block incoming connections'*|*' is blocked'*) exit 0 ;; *) exit 78 ;; esac`
      : `state=$(${fw} --getappblocked ${app} 2>&1)
case "$state" in *'is not part of the firewall'*) exit 0 ;; esac
${fw} --remove ${app} >/dev/null || exit 77
state=$(${fw} --getappblocked ${app} 2>&1)
case "$state" in *'is not part of the firewall'*) exit 0 ;; *) exit 78 ;; esac`
  return `export LC_ALL=C
${
  operation === 'remove'
    ? restore
    : `global=$(${fw} --getglobalstate) || exit 77
case "$global" in *'State = 0'*) exit 10 ;; esac
block=$(${fw} --getblockall) || exit 77
case "$block" in *'blocking all'*|*'block all non-essential'*|*[Ee][Nn][Aa][Bb][Ll][Ee][Dd]*) exit 13 ;; esac
${operation === 'apply' ? `${fw} --add ${app} >/dev/null && ${fw} --unblockapp ${app} >/dev/null || exit 77` : ''}
${inspect}`
}`
}

function ufwScript(target, operation, owner) {
  const profile = `PisperRemote-${owner}`
  const path = `/etc/ufw/applications.d/pisper-remote-${owner}`
  const content = `# Managed by Pisper ${owner}\n[${profile}]\ntitle=Pisper remote HTTPS\ndescription=Pisper remote HTTPS\nports=${target.port}/tcp\n`
  const check = `printf '%s\\n' "$added" | /usr/bin/grep -E -- ${shellQuote(`^ufw allow '?${profile}'?( comment .*)?$`)} >/dev/null`
  return `export LC_ALL=C
set -e
state=$(/usr/sbin/ufw status) || exit 77
added=$(/usr/sbin/ufw show added) || exit 77
${
  operation === 'remove'
    ? `if [ -f ${shellQuote(path)} ]; then
  /usr/bin/grep -Fx -- ${shellQuote(`# Managed by Pisper ${owner}`)} ${shellQuote(path)} >/dev/null || exit 78
  if ${check}; then /usr/sbin/ufw --force delete allow ${shellQuote(profile)} >/dev/null || exit 77; fi
  added=$(/usr/sbin/ufw show added) || exit 77
  if ${check}; then exit 78; fi
  /bin/rm -- ${shellQuote(path)}
elif ${check}; then
  exit 78
fi
exit 0`
    : `case "$state" in *'Status: inactive'*) exit 10 ;; *'Status: active'*) ;; *) exit 78 ;; esac
${
  operation === 'apply'
    ? `if [ -f ${shellQuote(path)} ]; then
  /usr/bin/grep -Fx -- ${shellQuote(`# Managed by Pisper ${owner}`)} ${shellQuote(path)} >/dev/null || exit 78
fi
printf '%s' ${shellQuote(content)} > ${shellQuote(path)}
/usr/sbin/ufw app update ${shellQuote(profile)} >/dev/null || exit 77
/usr/sbin/ufw allow ${shellQuote(profile)} >/dev/null || exit 77
state=$(/usr/sbin/ufw status) || exit 77
added=$(/usr/sbin/ufw show added) || exit 77`
    : ''
}
[ -f ${shellQuote(path)} ] || exit 12
/usr/bin/grep -Fx -- 'ports=${target.port}/tcp' ${shellQuote(path)} >/dev/null || exit 12
${check} || exit 12
printf '%s\\n' "$state" | /usr/bin/grep -E -- ${shellQuote(`^${profile}( +\\(v6\\))? +ALLOW IN +`)} >/dev/null || exit 12
exit 0`
}`
}

function firewalldScript(target, operation, owner) {
  // 日志前缀同时作为规则所有权标识，避免清理用户原有的同端口放行规则。
  const rule = shellQuote(
    `rule port port="${target.port}" protocol="tcp" log prefix="pisper-${owner}" limit value="1/m" accept`,
  )
  const operations = target.zones.flatMap((zone) =>
    [false, true].map((permanent) => {
      const base = `/usr/bin/firewall-cmd ${permanent ? '--permanent ' : ''}--zone=${shellQuote(zone)}`
      const query = `${base} --query-rich-rule=${rule} >/dev/null 2>&1`
      if (operation === 'remove')
        return `${query}
code=$?
case "$code" in
  0) ${base} --remove-rich-rule=${rule} >/dev/null || exit 77 ;;
  1) ;;
  *) exit 77 ;;
esac
${query}
code=$?
case "$code" in 1) ;; 0) exit 78 ;; *) exit 77 ;; esac`
      return `${operation === 'apply' ? `${base} --add-rich-rule=${rule} >/dev/null || exit 77\n` : ''}${query} || exit 12`
    }),
  )
  return `export LC_ALL=C
/usr/bin/firewall-cmd --state >/dev/null 2>&1 || exit 77
${operations.join('\n')}
exit 0`
}

export function firewallCommand(target, operation, { owner, elevated = false } = {}) {
  if (!/^[a-f0-9]{12}$/.test(owner)) throw new Error('Invalid firewall owner')
  if (!Number.isInteger(target.port) || target.port < 1 || target.port > 65535)
    throw new Error('Invalid remote HTTPS port')
  if (typeof target.execPath !== 'string' || !target.execPath || /[\0\r\n]/.test(target.execPath))
    throw new Error('Invalid runtime executable')
  if (!['inspect', 'apply', 'remove'].includes(operation))
    throw new Error('Invalid firewall operation')
  let script
  if (target.platform === 'win32') script = windowsScript(target, operation, owner)
  else if (target.platform === 'darwin' && sidecarPath(target.execPath))
    script = macScript(target, operation)
  else if (target.platform === 'linux' && target.backend === 'ufw')
    script = ufwScript(target, operation, owner)
  else if (
    target.platform === 'linux' &&
    target.backend === 'firewalld' &&
    target.zones?.length &&
    target.zones.every((zone) => /^[\w-]+$/.test(zone))
  )
    script = firewalldScript(target, operation, owner)
  else throw new Error('Unsupported firewall target')
  return { ...commandFor(target.platform, script, elevated), script, operation, elevated, target }
}

function resultCode(result) {
  if (result.timedOut) return 'timeout'
  if (result.missing) return 'unsupported'
  if (result.code === 0) return 'configured'
  if (result.code === 10) return 'not_needed'
  if (result.code === 12) return 'needs_authorization'
  if (result.code === 13) return 'block_all'
  // osascript 会把 shell 的退出码包装到 stderr；系统取消仍保留 -128。
  if (/\(-128\)/.test(result.stderr) || [80, 126].includes(result.code)) return 'cancelled'
  if (/\(10\)/.test(result.stderr)) return 'not_needed'
  if (/\(13\)/.test(result.stderr)) return 'block_all'
  if (result.code === 77 || result.code === 127 || /\(77\)/.test(result.stderr))
    return 'permission_denied'
  return 'verification_failed'
}

export class RemoteFirewallService {
  constructor({
    dataDir,
    platform = process.platform,
    execPath = process.execPath,
    packaged = isSea(),
    execute = executeFirewallCommand,
    exists = existsSync,
  } = {}) {
    this.filePath = join(dataDir, 'remote-firewall.json')
    this.owner = createHash('sha256').update(resolve(dataDir)).digest('hex').slice(0, 12)
    this.platform = platform
    this.execPath = execPath
    this.packaged = packaged
    this.execute = execute
    this.exists = exists
    this.targets = []
    this.queue = Promise.resolve()
    this.current = {
      state: 'disabled',
      reason: null,
      port: null,
      scope: platform === 'darwin' ? 'application' : platform === 'win32' ? 'program_port' : 'port',
      checkedAt: null,
      busy: false,
      lanReachability: 'unverified',
    }
    try {
      const saved = JSON.parse(readFileSync(this.filePath, 'utf8'))
      // 存储中的历史目标也走命令生成校验，不能把损坏状态插入提权脚本。
      this.targets = (saved.targets || []).filter((target) => {
        try {
          firewallCommand(target, 'inspect', { owner: this.owner })
          return target.platform === platform
        } catch {
          return false
        }
      })
    } catch {
      // 首次启用时没有历史规则。
    }
  }

  status() {
    return { ...this.current }
  }

  save() {
    mkdirSync(dirname(this.filePath), { recursive: true })
    const temporary = `${this.filePath}.${process.pid}.tmp`
    writeFileSync(temporary, `${JSON.stringify({ version: 1, targets: this.targets })}\n`, {
      mode: 0o600,
    })
    renameSync(temporary, this.filePath)
  }

  report(state, reason = null) {
    this.current = { ...this.current, state, reason, checkedAt: new Date().toISOString() }
    return this.status()
  }

  async run(target, operation, elevated = false) {
    return this.execute(firewallCommand(target, operation, { owner: this.owner, elevated }))
  }

  async runMany(operations, elevated) {
    if (operations.length === 1)
      return this.run(operations[0].target, operations[0].operation, elevated)
    const scripts = operations.map(
      ({ target, operation }) => firewallCommand(target, operation, { owner: this.owner }).script,
    )
    // 迁移旧规则和创建新规则共用一次系统授权；子进程退出码保留验证失败原因。
    const script =
      this.platform === 'win32'
        ? scripts
            .map(
              (item) =>
                `& ${psQuote(POWERSHELL)} -NoProfile -NonInteractive -EncodedCommand ${psQuote(encodedPowerShell(item))}; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }`,
            )
            .join('\n')
        : scripts.map((item) => `(\n${item}\n) || exit $?`).join('\n')
    return this.execute({
      ...commandFor(this.platform, script, elevated),
      script,
      operation: 'batch',
      operations,
      elevated,
    })
  }

  async target(port) {
    const target = { platform: this.platform, port, execPath: this.execPath }
    if (this.platform === 'darwin') {
      if (!this.packaged || !sidecarPath(this.execPath)) return { reason: 'packaged_app_required' }
      const previous = this.targets.find((item) => item.execPath === this.execPath)
      if (previous) target.previousAppState = previous.previousAppState
      else {
        const result = await this.execute({
          file: MAC_FIREWALL,
          args: ['--getappblocked', this.execPath],
          timeout: 10_000,
          operation: 'baseline',
        })
        if (result.stdout.includes('is not part of the firewall'))
          target.previousAppState = 'absent'
        else if (/Allow incoming connections| is permitted/.test(result.stdout))
          target.previousAppState = 'allowed'
        else if (/Block incoming connections| is blocked/.test(result.stdout))
          target.previousAppState = 'blocked'
        else return { reason: 'inspection_failed' }
      }
    } else if (this.platform === 'linux') {
      if (this.exists('/usr/bin/firewall-cmd')) {
        const result = await this.execute({
          file: '/usr/bin/firewall-cmd',
          args: ['--get-active-zones'],
          timeout: 10_000,
          operation: 'detect',
        })
        const zones = result.stdout
          .split('\n')
          .filter((line) => /^[\w-]+(?: \(.*\))?$/.test(line))
          .map((line) => line.split(' ')[0])
        if (result.code === 0 && zones.length) return { ...target, backend: 'firewalld', zones }
      }
      if (this.exists('/usr/sbin/ufw')) return { ...target, backend: 'ufw' }
      return { reason: 'unsupported_firewall' }
    } else if (this.platform !== 'win32') return { reason: 'unsupported_platform' }
    return target
  }

  reconcile(options) {
    const task = this.queue.then(async () => {
      this.current.busy = true
      try {
        return await this.reconcileNow(options)
      } catch {
        return this.report(options.enabled ? 'error' : 'cleanup_pending', 'operation_failed')
      } finally {
        this.current.busy = false
      }
    })
    this.queue = task.catch(() => {})
    return task.then(() => this.status())
  }

  async cleanup(elevated) {
    if (!this.targets.length) return null
    const result = await this.runMany(
      this.targets.map((target) => ({ target, operation: 'remove' })),
      elevated,
    )
    if (resultCode(result) !== 'configured') return resultCode(result)
    this.targets = []
    this.save()
    return null
  }

  async reconcileNow({ enabled, port, allowElevation = false, inspectOnly = false }) {
    this.current.port = enabled ? port : null
    if (!enabled) {
      if (inspectOnly)
        return this.report(
          this.targets.length ? 'cleanup_pending' : 'disabled',
          this.targets.length ? 'needs_authorization' : null,
        )
      const failure = await this.cleanup(allowElevation)
      return this.report(failure ? 'cleanup_pending' : 'disabled', failure)
    }
    const target = await this.target(port)
    if (target.reason) return this.report('unsupported', target.reason)
    const sameTarget = (other) =>
      JSON.stringify(this.platform === 'darwin' ? { ...other, port: 0 } : other) ===
      JSON.stringify(this.platform === 'darwin' ? { ...target, port: 0 } : target)
    const stale = this.targets.some((item) => !sameTarget(item))
    const inspected = resultCode(await this.run(target, 'inspect'))
    if (!stale && ['configured', 'not_needed'].includes(inspected)) return this.report(inspected)
    if (inspected === 'block_all') return this.report('error', 'block_all')
    if (!allowElevation || inspectOnly)
      return this.report('needs_authorization', stale ? 'rule_changed' : inspected)
    const operations = stale
      ? this.targets.map((item) => ({ target: item, operation: 'remove' }))
      : []
    const owned = !(target.platform === 'darwin' && target.previousAppState === 'allowed')
    // 写入意图先于提权操作，取消或部分成功后仍可准确清理自己的残留规则。
    if (!this.targets.some(sameTarget) && owned) {
      this.targets.push(target)
      this.save()
    }
    operations.push({ target, operation: 'apply' })
    const applied = resultCode(await this.runMany(operations, true))
    if (['configured', 'not_needed'].includes(applied)) {
      this.targets = owned && applied === 'configured' ? [target] : []
      this.save()
      return this.report(applied)
    }
    return this.report(applied === 'cancelled' ? 'cancelled' : 'error', applied)
  }
}
