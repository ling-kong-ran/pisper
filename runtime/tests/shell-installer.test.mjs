import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  normalizeShellInstallerStatus,
  SHELL_INSTALLER_ACTIVE_POLL_MS,
  SHELL_INSTALLER_IDLE_POLL_MS,
  shellInstallerPollDelay,
} from '../../src/features/updates/shell-installer-state.ts'
import {
  startShellInstallerPoller,
  usableShellInstallerBridge,
} from '../../src/features/updates/shell-installer-poller.ts'

function status(overrides = {}) {
  return {
    supported: true,
    state: 'downloaded',
    currentVersion: '0.6.37',
    availableVersion: '0.6.38',
    releaseUrl: 'https://github.com/ling-kong-ran/pisper/releases/tag/v0.6.38',
    notes: '## What changed',
    fileName: 'Pisper_0.6.38_windows_x86_64-setup.exe',
    size: 80,
    transferred: 80,
    canDownload: true,
    autoDownload: true,
    message: '',
    retryAt: null,
    ...overrides,
  }
}

test('shell installer status from the bridge is validated field by field', () => {
  assert.deepEqual(normalizeShellInstallerStatus(status()), status())
  for (const value of [null, undefined, 'downloaded', [], {}, { state: 'installed' }]) {
    assert.equal(normalizeShellInstallerStatus(value), null)
  }
  const hostile = normalizeShellInstallerStatus(
    status({
      supported: 'yes',
      availableVersion: '0.6.38<script>',
      currentVersion: 7,
      releaseUrl: 'https://evil.example/pisper/releases',
      fileName: '../../Pisper_0.6.38_windows_x86_64-setup.exe',
      size: -1,
      transferred: Number.NaN,
      canDownload: 'true',
      message: 'x'.repeat(2_000),
      retryAt: 'soon',
    }),
  )
  assert.equal(hostile.supported, false)
  assert.equal(hostile.availableVersion, '')
  assert.equal(hostile.currentVersion, '')
  assert.equal(hostile.releaseUrl, 'https://github.com/ling-kong-ran/pisper/releases')
  assert.equal(hostile.fileName, '')
  assert.equal(hostile.size, 0)
  assert.equal(hostile.transferred, 0)
  assert.equal(hostile.canDownload, false)
  assert.equal(hostile.message.length, 500)
  assert.equal(hostile.retryAt, null)
  // 旧字段缺失时自动下载按壳的默认值（开启）显示。
  const { autoDownload: _ignored, ...withoutAuto } = status()
  assert.equal(normalizeShellInstallerStatus(withoutAuto).autoDownload, true)
  assert.equal(normalizeShellInstallerStatus(status({ autoDownload: false })).autoDownload, false)
  // 进度不能超过总大小。
  assert.equal(normalizeShellInstallerStatus(status({ transferred: 500 })).transferred, 80)
})

test('shell installer polling follows the shell state', () => {
  assert.equal(shellInstallerPollDelay('downloading'), SHELL_INSTALLER_ACTIVE_POLL_MS)
  assert.equal(shellInstallerPollDelay('checking'), SHELL_INSTALLER_ACTIVE_POLL_MS)
  for (const state of ['idle', 'current', 'available', 'downloaded', 'error']) {
    assert.equal(shellInstallerPollDelay(state), SHELL_INSTALLER_IDLE_POLL_MS)
  }
})

test('installer commands are main-window only and the frontend feature-detects older shells', async () => {
  const [bridge, permission, capability, config, library, glue, poller, appUpdate] =
    await Promise.all([
      readFile('src-tauri/src/desktop_shell/desktop-bridge.js', 'utf8'),
      readFile('src-tauri/permissions/shell-installer.toml', 'utf8'),
      readFile('src-tauri/capabilities/desktop-shell-installer.json', 'utf8'),
      readFile('src-tauri/tauri.conf.json', 'utf8'),
      readFile('src-tauri/src/desktop_shell/mod.rs', 'utf8'),
      readFile('src-tauri/src/desktop_shell/shell_installer.rs', 'utf8'),
      readFile('src/features/updates/shell-installer-poller.ts', 'utf8'),
      readFile('src/features/updates/useAppUpdate.ts', 'utf8'),
    ])
  const commands = [
    'desktop_shell_installer_status',
    'desktop_check_shell_installer',
    'desktop_download_shell_installer',
    'desktop_set_shell_installer_auto_download',
    'desktop_open_shell_installer',
    'desktop_reveal_shell_installer',
  ]
  for (const command of commands) {
    assert.match(bridge, new RegExp(`invoke\\('${command}'`))
    assert.match(permission, new RegExp(`"${command}"`))
    assert.match(library, new RegExp(`shell_installer::${command},`))
    // 每个命令先确认调用方是主窗口。
    const start = glue.search(new RegExp(`pub (?:async )?fn ${command}\\(`))
    assert.ok(start >= 0, command)
    assert.match(glue.slice(start, start + 400), /require_main\(&window\)\?/, command)
  }
  // 不能混进桌面宠物也有的通用桥接权限。
  const desktopPermissions = await readFile('src-tauri/permissions/desktop.toml', 'utf8')
  for (const command of commands) assert.doesNotMatch(desktopPermissions, new RegExp(command))
  assert.deepEqual(JSON.parse(capability).windows, ['main'])
  assert.deepEqual(JSON.parse(capability).permissions, ['allow-pisper-desktop-shell-installer'])
  assert.ok(JSON.parse(config).app.security.capabilities.includes('desktop-shell-installer'))
  // 后台任务在启动成功后才开始，不在启动路径上等待。
  assert.match(library, /shell_installer::start\(app\.handle\(\)\);/)
  // 前端可能跑在没有这组方法的旧壳上：逐个确认是函数才用。
  assert.match(poller, /typeof bridge\[name\] === 'function'/)
  assert.match(appUpdate, /useShellInstaller\(bridge\?\.shellInstaller\)/)
  // 只有手动检查才顺带触发壳的检查；定时检查由壳自己负责，不重复请求。
  assert.match(appUpdate, /if \(refresh\) void shellInstallerCheckRef\.current/)
})

function fakeScheduler() {
  const timers = new Map()
  let next = 1
  return {
    timers,
    setTimeout(callback, delay) {
      const id = next++
      timers.set(id, { callback, delay })
      return id
    },
    clearTimeout(id) {
      timers.delete(id)
    },
    async fire() {
      const entries = [...timers]
      timers.clear()
      for (const [, { callback }] of entries) callback()
      await new Promise((resolve) => setImmediate(resolve))
    },
  }
}

function fakeBridge(responses) {
  const calls = []
  const reply =
    (name) =>
    async (...args) => {
      calls.push([name, ...args])
      const value = responses[name]
      if (value instanceof Error) throw value
      return typeof value === 'function' ? value(...args) : value
    }
  return {
    calls,
    bridge: {
      status: reply('status'),
      check: reply('check'),
      download: reply('download'),
      setAutoDownload: reply('setAutoDownload'),
      open: reply('open'),
      reveal: reply('reveal'),
    },
  }
}

const settle = () => new Promise((resolve) => setImmediate(resolve))

test('older shells without the full installer bridge load nothing', () => {
  assert.equal(usableShellInstallerBridge(undefined), null)
  assert.equal(usableShellInstallerBridge({ status: () => {} }), null)
  const published = []
  const stop = startShellInstallerPoller({ status: async () => status() }, (value) =>
    published.push(value),
  )
  stop()
  assert.deepEqual(published, [])
})

test('the poller mirrors the shell, speeds up while downloading and stops cleanly', async () => {
  let current = status({ state: 'downloading', size: 100, transferred: 10 })
  const { bridge, calls } = fakeBridge({
    status: () => current,
    setAutoDownload: (enabled) => status({ autoDownload: enabled }),
    open: true,
  })
  const scheduler = fakeScheduler()
  const published = []
  const stop = startShellInstallerPoller(bridge, (value) => published.push(value), scheduler)
  await settle()
  assert.equal(published.at(-1).status.state, 'downloading')
  assert.equal([...scheduler.timers.values()][0].delay, SHELL_INSTALLER_ACTIVE_POLL_MS)

  current = status()
  await scheduler.fire()
  assert.equal(published.at(-1).status.state, 'downloaded')
  assert.equal([...scheduler.timers.values()][0].delay, SHELL_INSTALLER_IDLE_POLL_MS)

  await published.at(-1).setAutoDownload(false)
  assert.equal(published.at(-1).status.autoDownload, false)
  assert.deepEqual(
    calls.find(([name]) => name === 'setAutoDownload'),
    ['setAutoDownload', false],
  )
  assert.equal(await published.at(-1).open(), true)

  stop()
  assert.equal(scheduler.timers.size, 0)
  const count = calls.length
  await settle()
  assert.equal(calls.length, count)
})

test('unsupported platforms publish nothing and failed actions reach the caller', async () => {
  const unsupported = fakeBridge({ status: status({ supported: false, state: 'unsupported' }) })
  const scheduler = fakeScheduler()
  const published = []
  startShellInstallerPoller(unsupported.bridge, (value) => published.push(value), scheduler)
  await settle()
  assert.deepEqual(published, [null])
  assert.equal(scheduler.timers.size, 0)

  const failing = fakeBridge({
    status: status({ state: 'available' }),
    download: new Error('busy'),
  })
  const controllers = []
  startShellInstallerPoller(failing.bridge, (value) => controllers.push(value), fakeScheduler())
  await settle()
  await assert.rejects(controllers.at(-1).download(), /busy/)
  // 失败后保留上一份状态。
  assert.equal(controllers.at(-1).status.state, 'available')
})
