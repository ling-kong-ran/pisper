import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  normalizeShellInstallerStatus,
  SHELL_INSTALLER_ACTIVE_POLL_MS,
  SHELL_INSTALLER_IDLE_POLL_MS,
  shellInstallerPollDelay,
  shellUpdateStatus,
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
  // macOS 下的是 Tauri 更新包；v0.6.40 的壳下的 dmg 也认。
  for (const fileName of [
    'Pisper_0.6.38_darwin_aarch64.app.tar.gz',
    'Pisper_0.6.38_darwin_aarch64.dmg',
  ]) {
    assert.equal(normalizeShellInstallerStatus(status({ fileName })).fileName, fileName)
  }
})

test('the desktop update status comes straight from the shell', () => {
  const downloading = shellUpdateStatus(
    status({ state: 'downloading', size: 200, transferred: 50 }),
    'restart',
  )
  assert.equal(downloading.state, 'downloading')
  assert.equal(downloading.percent, 25)
  assert.equal(downloading.total, 200)
  assert.equal(downloading.availableVersion, '0.6.38')
  assert.equal(downloading.installKind, 'restart')
  assert.equal(downloading.message, '')

  const failed = shellUpdateStatus(
    status({ state: 'error', message: 'rate limited', retryAt: 1_800_000_000 }),
    'installer',
  )
  assert.equal(failed.message, 'rate limited')
  assert.equal(failed.rateLimited, true)
  assert.equal(failed.retryAt, 1_800_000_000)
  assert.equal(failed.installKind, 'installer')

  // 已是最新时不再显示上一版的说明。
  assert.equal(shellUpdateStatus(status({ state: 'current' }), 'restart').notes, '')
  assert.equal(shellUpdateStatus(status({ state: 'current', size: 0 }), 'restart').percent, 0)
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
    'desktop_install_shell_update',
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
  // 只有手动检查才让壳立刻查一次；定时检查由壳自己负责，前端不再排桌面端的定时检查。
  assert.match(appUpdate, /if \(!refresh \|\| !controller\) return Promise\.resolve\(current\(\)\)/)
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
      install: reply('install'),
      open: reply('open'),
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
    install: true,
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
  assert.equal(published.at(-1).installKind, 'restart')
  assert.equal(await published.at(-1).install(), true)
  assert.ok(calls.some(([name]) => name === 'install'))

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

test('v0.6.40 shells without install fall back to opening the installer', async () => {
  const { bridge, calls } = fakeBridge({ status: status(), open: true })
  delete bridge.install
  const published = []
  startShellInstallerPoller(bridge, (value) => published.push(value), fakeScheduler())
  await settle()
  assert.equal(published.at(-1).installKind, 'installer')
  assert.equal(await published.at(-1).install(), true)
  assert.ok(calls.some(([name]) => name === 'open'))
  // 两个安装入口都没有的壳不加载。
  const { bridge: neither } = fakeBridge({ status: status() })
  delete neither.install
  delete neither.open
  assert.equal(usableShellInstallerBridge(neither), null)
})

test('the desktop app updates as a whole through the Tauri updater', async () => {
  const [glue, library, config, cargo, settings, appUpdate] = await Promise.all([
    readFile('src-tauri/src/desktop_shell/shell_installer.rs', 'utf8'),
    readFile('src-tauri/src/desktop_shell/mod.rs', 'utf8'),
    readFile('src-tauri/tauri.conf.json', 'utf8'),
    readFile('src-tauri/Cargo.toml', 'utf8'),
    readFile('src/features/config/UpdateSettings.tsx', 'utf8'),
    readFile('src/features/updates/useAppUpdate.ts', 'utf8'),
  ])
  const publicKey = (await readFile('src-tauri/updater.pubkey', 'utf8')).trim()
  const updater = JSON.parse(config).plugins.updater
  // 开发构建也要有插件配置，否则插件初始化失败；公钥必须和发布签名用的是同一把。
  assert.equal(updater.pubkey, publicKey)
  assert.deepEqual(updater.endpoints, [
    'https://github.com/ling-kong-ran/pisper/releases/latest/download/latest.json',
  ])
  assert.equal(updater.windows.installMode, 'passive')
  assert.match(cargo, /tauri-plugin-updater = "2\.\d+\.\d+"/)
  assert.match(library, /\.plugin\(tauri_plugin_updater::Builder::new\(\)\.build\(\)\)/)
  // 安装用已下载的那个版本，不再联网换成别的版本；先验签再收尾，收尾后才交给安装程序。
  assert.match(glue, /restore_update\(manifest\)/)
  assert.ok(glue.indexOf('update.verify(&bytes)') < glue.indexOf('update.install(bytes)'))
  assert.match(glue, /on_before_exit\(move \|\| \{[\s\S]*?prepare_for_update_exit\(&exit_app\)/)
  assert.match(
    library,
    /pub\(crate\) fn prepare_for_update_exit[\s\S]*?stop_background_work\(app\);\s*app\.cleanup_before_exit\(\);/,
  )
  // 桌面端不再有组件更新这条路。
  assert.doesNotMatch(appUpdate, /ComponentUpdates|installComponents|componentUpdateStatus/)
  assert.doesNotMatch(settings, /installComponents|ShellInstallerSettings/)
  assert.match(settings, /restartAndUpdate/)
})
