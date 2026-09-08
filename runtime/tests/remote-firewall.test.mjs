import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import test from 'node:test'
import { RemoteFirewallService, firewallCommand } from '../services/remote-firewall-service.mjs'
import { remoteRoutes } from '../http/routes/remote.mjs'
import { createPisperRuntime } from '../app-runtime.mjs'

const owner = 'abcdef123456'
const ok = { code: 0, stdout: '', stderr: '' }
const windows = { platform: 'win32', execPath: 'C:\\Pisper\\pisper-sidecar.exe', port: 5174 }
const mac = {
  platform: 'darwin',
  execPath: '/Applications/Pisper.app/Contents/MacOS/pisper-sidecar',
  port: 5174,
  previousAppState: 'absent',
}
const linux = {
  platform: 'linux',
  execPath: '/opt/pisper/pisper-sidecar',
  port: 5174,
  backend: 'ufw',
}

function fixture(t, options = {}) {
  const dataDir = mkdtempSync(join(tmpdir(), 'pisper-firewall-'))
  t.after(() => rmSync(dataDir, { recursive: true, force: true }))
  const calls = []
  const service = new RemoteFirewallService({
    dataDir,
    platform: 'win32',
    execPath: windows.execPath,
    ...options,
    execute: async (command) => {
      calls.push(command)
      return options.execute
        ? options.execute(command)
        : { ...ok, code: command.operation === 'inspect' ? 12 : 0 }
    },
  })
  return { dataDir, calls, service }
}

function psScript(command) {
  return Buffer.from(command.args.at(-1), 'base64').toString('utf16le')
}

test('Windows 规则同时限定实际端口、程序、本地 IPv4/IPv6 子网；只在明确操作时使用 UAC', () => {
  const plain = firewallCommand({ ...windows, port: 6001 }, 'apply', { owner })
  assert.match(
    psScript(plain),
    /-LocalPort 6001 -Program 'C:\\Pisper\\pisper-sidecar.exe' -RemoteAddress LocalSubnet/,
  )
  assert.match(psScript(plain), /-Protocol TCP/)
  assert.match(psScript(plain), /Get-NetFirewallApplicationFilter/)
  assert.match(psScript(plain), /PolicyStore ActiveStore/)
  assert.doesNotMatch(psScript(plain), /-Verb RunAs|Set-NetFirewallProfile|100\.64/)
  const elevated = firewallCommand(windows, 'apply', { owner, elevated: true })
  assert.match(psScript(elevated), /-Verb RunAs -Wait -PassThru/)
  assert.match(psScript(elevated), /exit \$p.ExitCode/)
  assert.match(psScript(elevated), /1223/)
})

test('状态轮询无系统操作；启动检查缺失规则不写规则、不提权', async (t) => {
  const { service, calls } = fixture(t)
  assert.equal(
    (await service.reconcile({ enabled: true, port: 6321, inspectOnly: true })).state,
    'needs_authorization',
  )
  assert.deepEqual(
    calls.map((call) => call.operation),
    ['inspect'],
  )
  assert.equal(calls[0].elevated, false)
  for (let index = 0; index < 20; index += 1)
    assert.equal(service.status().lanReachability, 'unverified')
  assert.equal(calls.length, 1)
})

test('明确本地开启只提权一次，成功检查后重复开启幂等', async (t) => {
  let configured = false
  const { service, calls } = fixture(t, {
    execute: (command) => {
      if (command.operation === 'apply') configured = true
      return { ...ok, code: command.operation === 'inspect' && !configured ? 12 : 0 }
    },
  })
  const first = await service.reconcile({ enabled: true, port: 6123, allowElevation: true })
  assert.equal(first.state, 'configured')
  assert.equal(first.busy, false)
  assert.equal(first.port, 6123)
  await service.reconcile({ enabled: true, port: 6123, allowElevation: true })
  assert.equal(calls.filter((call) => call.elevated).length, 1)
  assert.equal(service.targets.length, 1)
})

test('取消和权限错误均不伪报规则成功，重试仍可恢复', async (t) => {
  let failure = 80
  const { service } = fixture(t, {
    execute: (command) => ({ ...ok, code: command.operation === 'inspect' ? 12 : failure }),
  })
  assert.equal(
    (await service.reconcile({ enabled: true, port: 5174, allowElevation: true })).state,
    'cancelled',
  )
  failure = 77
  const denied = await service.reconcile({ enabled: true, port: 5174, allowElevation: true })
  assert.equal(denied.state, 'error')
  assert.equal(denied.reason, 'permission_denied')
  failure = 0
  assert.equal(
    (await service.reconcile({ enabled: true, port: 5174, allowElevation: true })).state,
    'configured',
  )
})

test('禁用清理无交互；权限不足保留清理意图，明确重试后清理', async (t) => {
  const { service, calls } = fixture(t, {
    execute: (command) => ({
      ...ok,
      code:
        command.operation === 'inspect'
          ? 12
          : command.operation === 'remove' && !command.elevated
            ? 77
            : 0,
    }),
  })
  await service.reconcile({ enabled: true, port: 5174, allowElevation: true })
  const disabled = await service.reconcile({ enabled: false })
  assert.equal(disabled.state, 'cleanup_pending')
  assert.equal(calls.at(-1).elevated, false)
  assert.equal(service.targets.length, 1)
  assert.equal(
    (await service.reconcile({ enabled: false, allowElevation: true })).state,
    'disabled',
  )
  assert.equal(service.targets.length, 0)
})

test('重启时禁用状态仅报告历史规则，不运行删除命令，即使调用方有系统权限', async (t) => {
  const { service, dataDir } = fixture(t)
  await service.reconcile({ enabled: true, port: 5174, allowElevation: true })
  const calls = []
  const restarted = new RemoteFirewallService({
    dataDir,
    platform: 'win32',
    execPath: windows.execPath,
    execute: async (command) => {
      calls.push(command)
      return ok
    },
  })
  const status = await restarted.reconcile({
    enabled: false,
    inspectOnly: true,
    allowElevation: true,
  })
  assert.equal(status.state, 'cleanup_pending')
  assert.equal(calls.length, 0)
  assert.equal(restarted.targets.length, 1)
})

test('升级端口和执行路径时，启动不改规则；显式重试一次授权清理旧规则并应用新规则', async (t) => {
  const { service, dataDir } = fixture(t)
  await service.reconcile({ enabled: true, port: 5174, allowElevation: true })
  const calls = []
  const upgraded = new RemoteFirewallService({
    dataDir,
    platform: 'win32',
    execPath: 'C:\\Pisper v2\\pisper-sidecar.exe',
    execute: async (command) => {
      calls.push(command)
      return { ...ok, code: command.operation === 'inspect' ? 12 : 0 }
    },
  })
  assert.equal(
    (await upgraded.reconcile({ enabled: true, port: 6188, inspectOnly: true })).reason,
    'rule_changed',
  )
  assert.equal(calls.filter((call) => call.elevated).length, 0)
  assert.equal(
    (await upgraded.reconcile({ enabled: true, port: 6188, allowElevation: true })).state,
    'configured',
  )
  const elevated = calls.filter((call) => call.elevated)
  assert.equal(elevated.length, 1)
  assert.deepEqual(
    elevated[0].operations.map((item) => item.operation),
    ['remove', 'apply'],
  )
  assert.equal(elevated[0].operations[0].target.port, 5174)
  assert.equal(upgraded.targets[0].port, 6188)
  assert.equal(
    JSON.parse(readFileSync(join(dataDir, 'remote-firewall.json'), 'utf8')).targets.length,
    1,
  )
})

test('并发显式操作串行化，禁用在配置结束后清理', async (t) => {
  const { service, calls } = fixture(t)
  const enabled = service.reconcile({ enabled: true, port: 5174, allowElevation: true })
  const disabled = service.reconcile({ enabled: false })
  await Promise.all([enabled, disabled])
  assert.deepEqual(
    calls.map((call) => call.operation),
    ['inspect', 'apply', 'remove'],
  )
  assert.equal(service.status().state, 'disabled')
})

test('macOS 通用 Node 或未打包 sidecar 一律不执行系统命令', async (t) => {
  for (const [execPath, packaged] of [
    ['/usr/local/bin/node', true],
    [mac.execPath, false],
  ]) {
    const { service, calls } = fixture(t, { platform: 'darwin', execPath, packaged })
    assert.equal(
      (await service.reconcile({ enabled: true, port: 5174, allowElevation: true })).reason,
      'packaged_app_required',
    )
    assert.equal(calls.length, 0)
  }
})

test('macOS 只授权专用 sidecar，不修改全局开关；原有阻止策略在禁用时恢复', async (t) => {
  const { service, calls } = fixture(t, {
    platform: 'darwin',
    execPath: mac.execPath,
    packaged: true,
    execute: (command) =>
      command.operation === 'baseline'
        ? { ...ok, stdout: 'Block incoming connections' }
        : { ...ok, code: command.operation === 'inspect' ? 12 : 0 },
  })
  await service.reconcile({ enabled: true, port: 5174, allowElevation: true })
  const apply = calls.find((call) => call.operation === 'apply')
  assert.equal(apply.file, '/usr/bin/osascript')
  assert.match(apply.args.join(' '), /with administrator privileges/)
  assert.match(apply.script, /--unblockapp/)
  assert.doesNotMatch(apply.script, /--setglobalstate|--setblockall|--setallowsigned/)
  await service.reconcile({ enabled: false })
  assert.match(calls.at(-1).script, /--blockapp/)
  assert.doesNotMatch(calls.at(-1).script, /--remove /)
})

test('macOS 原有放行策略不取得所有权，也不在禁用时删除', async (t) => {
  const { service, calls } = fixture(t, {
    platform: 'darwin',
    execPath: mac.execPath,
    packaged: true,
    execute: (command) =>
      command.operation === 'baseline'
        ? { ...ok, stdout: 'Incoming connection to the application is permitted' }
        : ok,
  })
  assert.equal(
    (await service.reconcile({ enabled: true, port: 5174, allowElevation: true })).state,
    'configured',
  )
  await service.reconcile({ enabled: false })
  assert.equal(
    calls.some((call) => call.operation === 'remove' || call.operation === 'apply'),
    false,
  )
})

test('macOS block-all、取消和超时真实报告，监听成功不影响结论', async (t) => {
  for (const [result, expected] of [
    [{ code: 13 }, 'block_all'],
    [{ code: 1, stderr: 'User canceled. (-128)' }, 'cancelled'],
    [{ code: -1, timedOut: true }, 'timeout'],
  ]) {
    const { service } = fixture(t, {
      platform: 'darwin',
      execPath: mac.execPath,
      packaged: true,
      execute: (command) =>
        command.operation === 'baseline'
          ? { ...ok, stdout: 'The application is not part of the firewall' }
          : command.operation === 'inspect'
            ? { ...ok, code: expected === 'block_all' ? 13 : 12 }
            : { ...ok, ...result },
    })
    const status = await service.reconcile({ enabled: true, port: 5174, allowElevation: true })
    assert.equal(status.reason, expected)
    assert.equal(status.lanReachability, 'unverified')
    assert.notEqual(status.state, 'configured')
  }
})

test('Linux UFW 使用自有应用配置限定端口；firewalld 使用独立前缀并覆盖当前区域与持久化策略', () => {
  const ufw = firewallCommand(linux, 'apply', { owner, elevated: true })
  assert.equal(ufw.file, '/usr/bin/pkexec')
  assert.match(ufw.script, /ports=5174\/tcp/)
  assert.match(ufw.script, /PisperRemote-abcdef123456/)
  assert.doesNotMatch(ufw.script, /ufw (enable|disable|reset)|allow all/)
  const firewalld = firewallCommand(
    { ...linux, backend: 'firewalld', zones: ['public', 'home'] },
    'apply',
    { owner },
  )
  assert.match(firewalld.script, /port="5174" protocol="tcp"/)
  assert.match(firewalld.script, /log prefix="pisper-abcdef123456" limit value="1\/m"/)
  assert.match(firewalld.script, /--permanent --zone='home'/)
  assert.match(firewalld.script, /--zone='public'/)
  assert.doesNotMatch(firewalld.script, /--reload|--set-default-zone|--panic/)
})

test('Linux 自动检测正在使用的区域，缺少管理器时不触发授权', async (t) => {
  const { service, calls } = fixture(t, {
    platform: 'linux',
    execPath: linux.execPath,
    exists: (path) => path === '/usr/bin/firewall-cmd',
    execute: (command) =>
      command.operation === 'detect'
        ? { ...ok, stdout: 'public (default)\n  interfaces: eth0\nhome\n  interfaces: wlan0\n' }
        : { ...ok, code: 12 },
  })
  await service.reconcile({ enabled: true, port: 6000, inspectOnly: true })
  assert.deepEqual(calls.at(-1).target.zones, ['public', 'home'])
  assert.equal(
    calls.some((call) => call.elevated),
    false,
  )
  const unsupported = fixture(t, { platform: 'linux', exists: () => false })
  assert.equal(
    (await unsupported.service.reconcile({ enabled: true, port: 6000, allowElevation: true }))
      .reason,
    'unsupported_firewall',
  )
  assert.equal(unsupported.calls.length, 0)
})

test(
  'macOS 实际查询文案通过替身命令验证，不执行宿主工具',
  { skip: process.platform === 'win32' },
  () => {
    for (const [appState, blockAll, expected] of [
      [
        'Incoming connection to /Applications/Pisper.app/Contents/MacOS/pisper-sidecar is permitted.',
        false,
        0,
      ],
      [
        'Incoming connection to /Applications/Pisper.app/Contents/MacOS/pisper-sidecar is blocked.',
        false,
        12,
      ],
      ['The application is not part of the firewall', false, 12],
      ['Incoming connection to the application is permitted', true, 13],
    ]) {
      const command = firewallCommand(mac, 'inspect', { owner })
      const script = command.script.replaceAll(
        "'/usr/libexec/ApplicationFirewall/socketfilterfw'",
        'fake_fw',
      )
      assert.equal(script.includes('/usr/libexec'), false)
      const replacement = `fake_fw() {
      case "$1" in
        --getglobalstate) printf '%s\\n' 'Firewall is enabled. (State = 1)' ;;
        --getblockall) printf '%s\\n' '${blockAll ? 'Firewall is blocking all non-essential incoming connections.' : 'Firewall has block all state set to disabled.'}' ;;
        --getappblocked) printf '%s\\n' '${appState}' ;;
        *) return 99 ;;
      esac
    }`
      const result = spawnSync('/bin/sh', ['-c', `${replacement}\n${script}`], { encoding: 'utf8' })
      assert.equal(result.status, expected, result.stderr)
    }
  },
)

test('端口、区域、执行路径均先校验；含引号的合法路径只能成为参数', () => {
  for (const port of [0, -1, 65536, '5174; touch /tmp/unsafe'])
    assert.throws(() => firewallCommand({ ...windows, port }, 'apply', { owner }))
  assert.throws(() =>
    firewallCommand({ ...linux, backend: 'firewalld', zones: ['home;evil'] }, 'apply', { owner }),
  )
  assert.throws(() => firewallCommand({ ...windows, execPath: 'a\ncommand' }, 'apply', { owner }))
  const quoted = firewallCommand(
    { ...windows, execPath: "C:\\Pisper's App\\pisper-sidecar.exe" },
    'apply',
    { owner },
  )
  assert.match(quoted.script, /Pisper''s App/)
})

test(
  '所有 POSIX 命令只做 shell 语法检查，不执行宿主防火墙命令',
  { skip: process.platform === 'win32' },
  () => {
    for (const target of [
      mac,
      { ...mac, previousAppState: 'blocked' },
      linux,
      { ...linux, backend: 'firewalld', zones: ['home'] },
    ]) {
      for (const operation of ['inspect', 'apply', 'remove']) {
        const command = firewallCommand(target, operation, { owner })
        const parsed = spawnSync('/bin/sh', ['-n'], { input: command.script, encoding: 'utf8' })
        assert.equal(parsed.status, 0, parsed.stderr)
      }
    }
  },
)

async function route(
  path,
  {
    method = 'POST',
    remote = false,
    address = '127.0.0.1',
    host = '127.0.0.1:5173',
    origin,
    contentType = 'application/json',
    input = {},
    control = {},
  } = {},
) {
  let output
  const found = remoteRoutes.find((item) => item.path === path && item.method === method)
  assert.ok(found)
  await found.handler({
    services: { remoteControl: control },
    req: {
      pisperRemote: remote,
      socket: { remoteAddress: address },
      headers: { host, origin, 'content-type': contentType },
    },
    body: async () => input,
    json: (status, body) => {
      output = { status, body }
    },
  })
  return output
}

test('远端即使伪装 loopback 或提供提权标志，也不能配置防火墙', async () => {
  const control = {
    retryFirewall: () => assert.fail('远端不能调用提权'),
    setEnabled: () => assert.fail('拒绝前不能改变开关'),
  }
  for (const request of [
    { remote: true },
    { address: '192.168.1.5' },
    { host: 'attacker.test:5173' },
    { origin: 'https://attacker.test' },
  ]) {
    assert.equal((await route('/api/remote/firewall/retry', { ...request, control })).status, 403)
    assert.equal(
      (
        await route('/api/remote/enabled', {
          ...request,
          method: 'PUT',
          input: { enabled: true, configureFirewall: true },
          control,
        })
      ).status,
      403,
    )
    assert.equal(
      (await route('/api/remote/firewall', { ...request, method: 'GET', control })).status,
      403,
    )
  }
})

test('本地明确开启和重试获得配置权限；普通远端开关永不获得权限，公共状态不变', async () => {
  const calls = []
  const state = { enabled: true, listening: true, error: null }
  const control = {
    status: () => state,
    setEnabled: (_enabled, options) => calls.push(options),
    retryFirewall: () => ({ state: 'configured' }),
    firewallStatus: () => ({ state: 'needs_authorization' }),
  }
  const local = await route('/api/remote/enabled', {
    method: 'PUT',
    input: { enabled: true, configureFirewall: true },
    control,
  })
  assert.deepEqual(local.body, { apiVersion: 1, ...state })
  assert.deepEqual(calls[0], { configureFirewall: true })
  await route('/api/remote/enabled', {
    method: 'PUT',
    remote: true,
    input: { enabled: true },
    control,
  })
  assert.deepEqual(calls[1], { configureFirewall: false })
  assert.equal(
    (await route('/api/remote/firewall/retry', { control, address: '::ffff:127.0.0.1' })).status,
    200,
  )
  assert.equal(
    (await route('/api/remote/firewall/retry', { control, contentType: 'text/plain' })).status,
    415,
  )
  assert.equal(
    (await route('/api/remote/firewall', { method: 'GET', control })).body.state,
    'needs_authorization',
  )
})

test('runtime 启动传入只读选项，明确开启使用实际随机 HTTPS 端口；关闭停止监听后清理', async (t) => {
  const { dataDir } = fixture(t)
  writeFileSync(join(dataDir, 'package.json'), '{"version":"0.0.0-test"}')
  const calls = []
  const firewall = {
    status: () => ({ state: 'disabled' }),
    reconcile: async (options) => {
      calls.push(options)
      return firewall.status()
    },
  }
  class Runtime {
    async init() {}
    async dispose() {}
  }
  const app = await createPisperRuntime({
    root: dataDir,
    dataDir,
    runtimeCwd: dataDir,
    production: true,
    port: 0,
    runtimeCapabilities: { profile: 'desktop', features: { remoteAccess: true } },
    remote: { enabled: true, port: 0, host: '127.0.0.1', firewallService: firewall },
    runtimeModuleLoader: async () => ({ AgentRuntimeService: Runtime }),
    apiHandlerModuleLoader: async () => ({ createApiHandler: () => async () => false }),
  })
  t.after(() => app.close())
  assert.equal(calls[0].inspectOnly, true)
  assert.equal(calls[0].allowElevation, undefined)
  assert.equal(calls[0].port, app.remoteControl.status().port)
  assert.ok(calls[0].port > 0)
  await app.remoteControl.setEnabled(true, { configureFirewall: true })
  assert.equal(calls.at(-1).allowElevation, true)
  await app.remoteControl.setEnabled(false, { configureFirewall: true })
  assert.equal(calls.at(-1).enabled, false)
  assert.equal(calls.at(-1).allowElevation, undefined)
  assert.equal(app.remoteControl.status().listening, false)
  await app.remoteControl.retryFirewall()
  assert.equal(calls.at(-1).enabled, false)
  assert.equal(calls.at(-1).allowElevation, true)
})
