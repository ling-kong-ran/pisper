import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { createServer } from 'node:http'
import { createServer as createNetServer } from 'node:net'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { setTimeout as delay } from 'node:timers/promises'
import { chromium } from 'playwright-core'
import { PNG } from 'pngjs'
import { checkWorkflowParity } from './smoke-rust-workflows.mjs'
import { checkNotificationParity } from './smoke-rust-notifications.mjs'
import { checkFileChangeParity } from './smoke-rust-file-changes.mjs'
import { checkGameAssetParity, checkGameAssetUiParity } from './smoke-rust-game-assets.mjs'
import { checkCustomUiParity } from './smoke-rust-custom-ui.mjs'
import { checkSessionProjectionReads } from './smoke-rust-session-reads.mjs'
import { checkWebSearchParity } from './smoke-rust-web-search.mjs'
import { checkPluginsParity } from './smoke-rust-plugins.mjs'
import { checkImageAgentParity } from './smoke-rust-image-agent.mjs'
import { checkVisualParity } from './smoke-rust-visual.mjs'
import { checkChannelsParity } from './smoke-rust-channels.mjs'
import { checkBrowserParity } from './smoke-rust-browser.mjs'
import {
  checkTerminalBridgeUiParity,
  checkTerminalEntrypointNegatives,
  checkNativeDesktopTerminalIpcParity,
} from './smoke-rust-terminal.mjs'

// 只访问自己启动的后端和模型夹具；旧产物的全局 config 路径不满足隔离要求。
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const args = process.argv.slice(2)
const legacyProbe = args.includes('--legacy-probe')
const skipUi = args.includes('--skip-ui')
const desktopIndex = args.indexOf('--desktop')
assert.ok(
  desktopIndex < 0 || args[desktopIndex + 1],
  '--desktop requires the actual installed Pisper executable',
)
const desktopExecutable = desktopIndex < 0 ? undefined : resolve(args[desktopIndex + 1])
const desktopCdpAuto = args.includes('--desktop-cdp-auto')
assert.ok(!desktopCdpAuto || desktopExecutable, '--desktop-cdp-auto requires --desktop')
const appRootIndex = args.indexOf('--app-root')
assert.ok(
  appRootIndex < 0 || args[appRootIndex + 1],
  '--app-root requires a staged resource directory',
)
const applicationRoot = appRootIndex < 0 ? root : resolve(args[appRootIndex + 1])
const terminalEvidenceIndex = args.indexOf('--native-terminal-evidence')
assert.ok(
  terminalEvidenceIndex < 0 || args[terminalEvidenceIndex + 1],
  '--native-terminal-evidence requires a fresh native test record',
)
const terminalEvidencePath =
  terminalEvidenceIndex < 0 ? undefined : resolve(args[terminalEvidenceIndex + 1])
const executable = resolve(
  args.find(
    (arg, index) =>
      !arg.startsWith('--') &&
      (desktopIndex < 0 || index !== desktopIndex + 1) &&
      (appRootIndex < 0 || index !== appRootIndex + 1) &&
      (terminalEvidenceIndex < 0 || index !== terminalEvidenceIndex + 1),
  ) || join(root, 'release/rust-build/pisper-server.exe'),
)
const output = join(
  root,
  'release/local-rust-build/usability',
  new Date().toISOString().replaceAll(':', '-'),
)
const sandboxHome = join(output, 'home')
const agent = join(output, 'agent')
const workspace = join(output, 'workspace')
const providerId = 'rust-usability-fixture'
const modelId = 'rust-fixture-model'
const imageProviderId = 'rust-image-fixture'
const imageModelId = 'gpt-image-fixture'
const sentinel = 'PISPER_RUST_REAL_READ_TOOL_OK'
const mcpSentinel = 'PISPER_RUST_REAL_MCP_TOOL_OK'
const mcpId = 'rustfixture'
const mcpTool = `mcp__${mcpId}__echo`
const report = {
  executable,
  applicationRoot,
  desktopExecutable,
  output,
  fixture: 'loopback-only OpenAI-compatible model; synthetic credentials; isolated agent/workspace',
  legacyProbe,
  started: new Date().toISOString(),
  checks: [],
  failures: [],
  pageErrors: [],
  consoleErrors: [],
  failedApi: [],
  deniedBrowserRequests: [],
  unsupported: [],
  expectedUnsupportedApi: [],
  fixtureRequests: [],
  imageFixtureRequests: [],
  mcpRequests: [],
  serverLog: '',
}
let child
let browser
let desktopPage
let base
let sessionId
let cookie
let ready
let fixture
let runtimeCapabilities
let notificationRestart
let fileChangeRestart
let gameAssetRestart
let customUiRestart
let pluginsRestart
let channelsRestart
let browserRestart
let ownedDesktopProcesses = []
const heldModelRequests = new Map()
const heldImageRequests = new Map()

function disabledFeature(path) {
  const feature =
    path.startsWith('/api/workflows') || path.startsWith('/api/workflow-runs')
      ? 'workflows'
      : path.startsWith('/api/schedules')
        ? 'schedules'
        : /^\/api\/sessions\/[^/]+\/(file-changes|change-summary)(?:\/|$)/.test(path)
          ? 'fileChanges'
          : null
  return feature && runtimeCapabilities?.features?.[feature] === false ? feature : null
}

async function assertUnsupported(path) {
  const response = await fetch(base + path, {
    headers: { Origin: base, Cookie: cookie },
    signal: AbortSignal.timeout(10000),
  })
  assert.equal(response.status, 501, `${path} must honestly reject its unimplemented feature`)
  const data = await response.json()
  assert.equal(data.code, 'unsupported')
  assert.equal(typeof data.error, 'string')
  const evidence = {
    feature: disabledFeature(path),
    path,
    status: 'unsupported',
    message: data.error,
  }
  report.unsupported.push(evidence)
  console.log(`UNSUPPORTED ${evidence.feature}: ${data.error}`)
  return evidence
}

async function check(name, fn) {
  try {
    const evidence = await fn()
    report.checks.push({ name, ok: true, ...(evidence === undefined ? {} : { evidence }) })
    console.log(`PASS ${name}`)
    return evidence
  } catch (error) {
    report.checks.push({ name, ok: false })
    report.failures.push({ name, error: error.stack || String(error) })
    console.log(`FAIL ${name}: ${error.message}`)
    return undefined
  } finally {
    await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
  }
}

async function json(path, method = 'GET', body, timeout = 20000) {
  assert.ok(path.startsWith('/api/'), 'Harness requests must target local API')
  const response = await fetch(base + path, {
    method,
    headers: { Origin: base, Cookie: cookie, 'Content-Type': 'application/json' },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(timeout),
  })
  const raw = await response.text()
  assert.ok(response.ok, `${method} ${path}: HTTP ${response.status} ${raw.slice(0, 200)}`)
  assert.match(
    response.headers.get('content-type') || '',
    /application\/json/,
    `${path} must return JSON`,
  )
  return JSON.parse(raw)
}

function request(path, options = {}) {
  assert.ok(path.startsWith('/api/'), 'Fixture must target its local backend')
  return fetch(base + path, {
    ...options,
    headers: { Origin: base, Cookie: cookie, ...options.headers },
    redirect: 'error',
    signal: options.signal || AbortSignal.timeout(20000),
  })
}

function assertConfig(config) {
  assert.ok(Array.isArray(config.providers), '/api/config must contain providers[]')
  for (const provider of config.providers) {
    assert.equal(typeof provider.id, 'string')
    assert.equal(typeof provider.name, 'string')
    assert.ok(Array.isArray(provider.models), `${provider.id}.models must be an array`)
    assert.equal(typeof provider.configured, 'boolean')
    assert.equal(typeof provider.enabled, 'boolean')
    for (const model of provider.models) {
      assert.equal(typeof model.id, 'string')
      assert.equal(typeof model.name, 'string')
      assert.equal(typeof model.kind, 'string')
    }
  }
  assert.equal(typeof config.thinkingLevel, 'string')
  assert.equal(typeof config.toolMode, 'string')
  assert.equal(
    JSON.stringify(config).includes('synthetic-rust-test-key'),
    false,
    'Config must never echo credentials',
  )
}

function assertMessages(data) {
  assert.ok(Array.isArray(data.messages), 'History/live response must contain messages[]')
  assert.equal(typeof data.pageInfo?.start, 'number')
  assert.equal(typeof data.pageInfo?.hasMore, 'boolean')
  assert.ok(data.pageInfo?.nextCursor === null || typeof data.pageInfo?.nextCursor === 'string')
  for (const message of data.messages) {
    assert.equal(typeof message.id, 'string')
    assert.equal(typeof message.role, 'string')
    assert.equal(typeof message.text, 'string')
  }
}

async function start() {
  const env = Object.fromEntries(
    Object.entries(process.env).filter(([key]) =>
      /^(?:PATH|PATHEXT|SYSTEMROOT|WINDIR|COMSPEC|TEMP|TMP|PROCESSOR_ARCHITECTURE|NUMBER_OF_PROCESSORS)$/i.test(
        key,
      ),
    ),
  )
  Object.assign(env, {
    HOME: sandboxHome,
    USERPROFILE: sandboxHome,
    APPDATA: join(sandboxHome, 'AppData/Roaming'),
    LOCALAPPDATA: join(sandboxHome, 'AppData/Local'),
    CODEX_HOME: join(sandboxHome, '.codex'),
    CLAUDE_CONFIG_DIR: join(sandboxHome, '.claude'),
    XDG_CONFIG_HOME: join(sandboxHome, '.config'),
    PISPER_AGENT_DIR: agent,
    PI_CODING_AGENT_DIR: agent,
    PISPER_RS_DATA_DIR: agent,
    PISPER_WORKSPACE_DIR: workspace,
    PISPER_FRONTEND_ROOT: join(root, 'dist'),
    PISPER_APP_ROOT: applicationRoot,
    PISPER_DESKTOP_TOKEN: 'rust_usability_sandbox_only',
    PISPER_EXIT_ON_STDIN_CLOSE: '1',
    PI_SKIP_VERSION_CHECK: '1',
    PI_TELEMETRY: '0',
    RUST_LOG: 'pisper_server=info',
    ...(legacyProbe
      ? {
          PISPER_RS_PROVIDER: providerId,
          PISPER_RS_MODEL: modelId,
          PISPER_RS_API_KEY: 'synthetic-rust-test-key',
        }
      : {}),
  })
  if (desktopExecutable) {
    const health = await startDesktop(env)
    await assertBootPublicSession()
    return health
  }
  ready = undefined
  let captured = ''
  child = spawn(executable, [], {
    cwd: workspace,
    env,
    windowsHide: true,
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const append = (chunk) => {
    const text = String(chunk)
    captured += text
    report.serverLog += text
    const line = captured.split(/\r?\n/).find((value) => value.startsWith('PISPER_SIDECAR_READY '))
    if (line && !ready) ready = JSON.parse(line.slice('PISPER_SIDECAR_READY '.length))
  }
  child.stdout.on('data', append)
  child.stderr.on('data', append)
  let spawnError
  child.on('error', (error) => {
    spawnError = error
  })
  const deadline = Date.now() + 30000
  while (!ready && Date.now() < deadline) {
    if (spawnError) throw spawnError
    assert.equal(child.exitCode, null, `Rust server exited early: ${report.serverLog.slice(-2000)}`)
    await delay(100)
  }
  assert.ok(ready?.url, `Rust sidecar did not announce readiness: ${report.serverLog.slice(-2000)}`)
  base = ready.url
  assert.equal(new URL(base).hostname, '127.0.0.1')
  cookie = '__pisper_desktop=rust_usability_sandbox_only'
  report.backend = base
  const health = await json('/api/health')
  assert.equal(health.engine, 'pi-rs')
  await assertBootPublicSession()
  return health
}

async function assertBootPublicSession() {
  if (legacyProbe) return
  const catalog = await json('/api/sessions')
  assert.ok(
    Array.isArray(catalog.sessions) && catalog.sessions.length > 0,
    'Boot must publish its actual persisted session before the frontend starts',
  )
  const id = catalog.sessions[0].id
  assert.equal(typeof id, 'string')
  const snapshot = await json(`/api/sessions/${id}/live`, 'GET', undefined, 5000)
  assert.equal(snapshot.id, id, 'Boot session must be addressable through its real live snapshot')
  assertMessages(snapshot)
  report.bootstrapSessions ||= []
  report.bootstrapSessions.push({ backend: base, id, cwd: snapshot.cwd })
}

async function freeLoopbackPort() {
  const server = createNetServer()
  await new Promise((done, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', done)
  })
  const port = server.address().port
  await new Promise((done) => server.close(done))
  return port
}

function powershellOutput(script) {
  return new Promise((done, reject) => {
    const helper = spawn('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], {
      windowsHide: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    let result = ''
    helper.stdout.on('data', (chunk) => {
      result += chunk
    })
    helper.on('error', reject)
    helper.once('exit', (code) =>
      code === 0
        ? done(result.trim() || '[]')
        : reject(new Error('Owned process inspection failed')),
    )
  })
}

async function ownedProcessTree(pid) {
  return JSON.parse(
    await powershellOutput(`
$ownedQueue = [System.Collections.Generic.Queue[int]]::new()
$ownedQueue.Enqueue(${pid})
$ownedRows = [System.Collections.Generic.List[object]]::new()
while ($ownedQueue.Count -gt 0) {
  $ownedPid = $ownedQueue.Dequeue()
  $ownedProcess = Get-CimInstance Win32_Process -Filter ('ProcessId = ' + $ownedPid)
  if ($null -eq $ownedProcess) { continue }
$ownedListeners = @(Get-NetTCPConnection -OwningProcess $ownedPid -State Listen -ErrorAction SilentlyContinue | ForEach-Object { [pscustomobject]@{ address = $_.LocalAddress; port = $_.LocalPort } })
$ownedRows.Add([pscustomobject]@{ pid = [int]$ownedProcess.ProcessId; started = $ownedProcess.CreationDate.ToUniversalTime().ToString('o'); executablePath = $ownedProcess.ExecutablePath; commandLine = $ownedProcess.CommandLine; listeners = $ownedListeners })
  foreach ($descendant in @(Get-CimInstance Win32_Process -Filter ('ParentProcessId = ' + $ownedPid))) { $ownedQueue.Enqueue([int]$descendant.ProcessId) }
}
ConvertTo-Json -InputObject @($ownedRows.ToArray()) -Depth 6 -Compress
`),
  )
}

async function stillOwnedProcesses() {
  if (!ownedDesktopProcesses.length) return []
  const filter = ownedDesktopProcesses.map((entry) => `ProcessId = ${entry.pid}`).join(' OR ')
  const current = JSON.parse(
    await powershellOutput(
      `ConvertTo-Json -InputObject @(Get-CimInstance Win32_Process -Filter '${filter}' | ForEach-Object { [pscustomobject]@{ pid = [int]$_.ProcessId; started = $_.CreationDate.ToUniversalTime().ToString('o') } }) -Compress`,
    ),
  )
  return current.filter((entry) =>
    ownedDesktopProcesses.some(
      (owned) => entry.pid === owned.pid && entry.started === owned.started,
    ),
  )
}

async function portStillOpen(url) {
  try {
    await fetch(url, { signal: AbortSignal.timeout(300) })
    return true
  } catch {
    return false
  }
}

async function startDesktop(env) {
  let cdpPort = desktopCdpAuto ? 0 : await freeLoopbackPort()
  Object.assign(env, {
    PISPER_DESKTOP_DATA_DIR: join(output, 'desktop-data'),
    PISPER_DESKTOP_TEST_CDP_PORT: String(cdpPort),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--enable-logging --log-file="${join(output, 'webview-debug.log')}" --v=1`,
  })
  child = spawn(desktopExecutable, [], {
    cwd: workspace,
    env,
    windowsHide: true,
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const cdpDiagnostics = { attempts: 0 }
  report.desktop = {
    pid: child.pid,
    cdpPort,
    transport: 'owned packaged Tauri WebView2 startup',
    cdpDiagnostics,
    automaticPort: desktopCdpAuto,
  }
  console.log(JSON.stringify({ ownedDesktopPid: child.pid, requestedCdpPort: cdpPort }))
  let spawnError
  child.on('error', (error) => {
    spawnError = error
  })
  const append = (chunk) => {
    report.serverLog += String(chunk).replaceAll(/([?&]token=)[^&\s"']+/g, '$1[REDACTED]')
  }
  child.stdout.on('data', append)
  child.stderr.on('data', append)
  let endpoint = `http://127.0.0.1:${cdpPort}`
  const deadline = Date.now() + 45000
  while (Date.now() < deadline) {
    if (spawnError) throw spawnError
    assert.equal(
      child.exitCode,
      null,
      `Owned Pisper shell exited before readiness: ${report.serverLog.slice(-1000)}`,
    )
    cdpDiagnostics.attempts += 1
    if (cdpDiagnostics.attempts === 1 || cdpDiagnostics.attempts % 50 === 0) {
      const processes = await ownedProcessTree(child.pid)
      cdpDiagnostics.processSnapshots ||= []
      cdpDiagnostics.processSnapshots.push({ at: new Date().toISOString(), processes })
    }
    try {
      if (desktopCdpAuto && cdpPort === 0) {
        const activePortPath = join(output, 'desktop-data/webview/EBWebView/DevToolsActivePort')
        const [port, browserPath] = (await readFile(activePortPath, 'utf8')).trim().split(/\r?\n/)
        const discovered = Number(port)
        assert.ok(Number.isInteger(discovered) && discovered > 0 && discovered <= 65535)
        assert.match(browserPath, /^\/devtools\/browser\/[a-f0-9-]+$/i)
        const owned = await ownedProcessTree(child.pid)
        assert.ok(
          owned.some(
            (process) =>
              process.executablePath?.endsWith('msedgewebview2.exe') &&
              process.listeners?.some(
                (listener) =>
                  listener.port === discovered && ['127.0.0.1', '::1'].includes(listener.address),
              ),
          ),
          'The automatic CDP listener must belong to this owned WebView2 process tree',
        )
        cdpPort = discovered
        endpoint = `http://127.0.0.1:${cdpPort}`
        cdpDiagnostics.automaticPortFile = activePortPath
        cdpDiagnostics.browserPath = browserPath
        report.desktop.cdpPort = cdpPort
      }
      const response = await fetch(endpoint + '/json/version', {
        signal: AbortSignal.timeout(1000),
      })
      if (response.ok) {
        browser = await chromium.connectOverCDP(endpoint, { timeout: 10000 })
        break
      }
      cdpDiagnostics.lastFailure = { status: response.status, endpoint: '/json/version' }
    } catch (error) {
      cdpDiagnostics.lastFailure = {
        message: error.message.slice(0, 400),
        code: error.code,
        causeCode: error.cause?.code,
        endpoint: '/json/version',
      }
      // 新启动的 WebView2 在自己端口尚未监听时继续等待，不复用其他进程。
    }
    await delay(200)
  }
  assert.ok(
    browser,
    `Owned installed Pisper did not expose its isolated WebView2 CDP endpoint: ${JSON.stringify(cdpDiagnostics.lastFailure)}`,
  )
  while (Date.now() < deadline) {
    const pages = browser.contexts().flatMap((context) => context.pages())
    desktopPage = pages.find((page) => /^http:\/\/127\.0\.0\.1:\d+/.test(page.url()))
    if (desktopPage) {
      base = new URL(desktopPage.url()).origin
      try {
        const health = await desktopPage.evaluate(async () => {
          const response = await fetch('/api/health')
          return response.ok ? response.json() : undefined
        })
        if (health?.engine === 'pi-rs') {
          const cookies = await desktopPage.context().cookies(base)
          const auth = cookies.find((entry) => entry.name === '__pisper_desktop')
          assert.ok(auth?.value, 'Installed shell must complete its real HttpOnly-cookie handshake')
          assert.equal(auth.httpOnly, true, 'Desktop auth cookie must actually be HttpOnly')
          const appInfo = await desktopPage.evaluate(() => window.pisperDesktop.getAppInfo())
          assert.equal(appInfo.desktop, true, 'Actual desktop IPC must be authorized')
          assert.equal(appInfo.packaged, true, 'Acceptance must exercise a packaged desktop shell')
          cookie = '__pisper_desktop=' + auth.value
          report.backend = base
          report.desktop = {
            pid: child.pid,
            cdpPort,
            cdpDiagnostics,
            transport: 'actual installed Tauri WebView2',
            handshake: 'http-only cookie',
            appInfo,
            startupNetworkBoundary:
              'External-browser interception begins after CDP attach; native startup requests are outside this assertion',
          }
          ownedDesktopProcesses = await ownedProcessTree(child.pid)
          report.desktop.ownedProcesses = ownedDesktopProcesses
          assert.equal((await json('/api/health')).engine, 'pi-rs')
          return health
        }
      } catch {
        // 跳转/引导 Cookie 尚未完成时只等待自己的 WebView。
      }
    }
    await delay(200)
  }
  assert.fail('Installed shell did not complete real sidecar/frontend bootstrap')
}

async function stop() {
  const current = child
  if (!current) return
  if (!desktopExecutable && current.exitCode !== null) {
    child = undefined
    assert.fail(`Owned Rust server exited before requested stdin shutdown: ${current.exitCode}`)
  }
  const exited =
    current.exitCode === null
      ? new Promise((done) => current.once('exit', done))
      : Promise.resolve()
  if (desktopExecutable) {
    if (current.exitCode === null) {
      const latest = await ownedProcessTree(current.pid)
      ownedDesktopProcesses = [...ownedDesktopProcesses, ...latest].filter(
        (entry, index, rows) =>
          rows.findIndex((other) => entry.pid === other.pid && entry.started === other.started) ===
          index,
      )
    }
    await browser?.close().catch(() => {})
    browser = undefined
    desktopPage = undefined
    // 只向本脚本创建的 PID 发关闭请求；托盘模式不退出时回收同一 owned tree。
    const close = spawn(
      'powershell.exe',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        `$ownedPisper = Get-Process -Id ${current.pid} -ErrorAction SilentlyContinue; if ($ownedPisper) { [void]$ownedPisper.CloseMainWindow() }`,
      ],
      { windowsHide: true, stdio: 'ignore' },
    )
    await new Promise((done) => close.once('exit', done))
    await Promise.race([exited, delay(1200)])
    let forced = false
    if (current.exitCode === null) {
      const kill = spawn('taskkill.exe', ['/PID', String(current.pid), '/T', '/F'], {
        windowsHide: true,
        stdio: 'ignore',
      })
      await new Promise((done) => kill.once('exit', done))
      forced = true
      await Promise.race([exited, delay(5000)])
    }
    for (const entry of await stillOwnedProcesses()) {
      // PID 与创建时间都仍匹配本脚本的启动记录，才清理已退出壳的残留子进程。
      const kill = spawn('taskkill.exe', ['/PID', String(entry.pid), '/T', '/F'], {
        windowsHide: true,
        stdio: 'ignore',
      })
      await new Promise((done) => kill.once('exit', done))
      forced = true
    }
    const deadline = Date.now() + 5000
    let alive
    let portsOpen
    do {
      alive = await stillOwnedProcesses()
      portsOpen = (
        await Promise.all([
          portStillOpen(base + '/api/health'),
          portStillOpen(`http://127.0.0.1:${report.desktop.cdpPort}/json/version`),
        ])
      ).some(Boolean)
      if (!alive.length && !portsOpen) break
      await delay(100)
    } while (Date.now() < deadline)
    report.desktopCleanups ||= []
    report.desktopCleanups.push({
      pid: current.pid,
      mode: forced ? 'forced-owned-process-tree' : 'graceful',
      remainingProcesses: alive,
      portsClosed: !portsOpen,
    })
    assert.deepEqual(alive, [], 'Owned desktop descendants must all exit')
    assert.equal(portsOpen, false, 'Owned backend and CDP ports must both close')
    assert.notEqual(current.exitCode, null, 'Owned desktop process tree did not exit')
    child = undefined
    return
  }
  const began = Date.now()
  const logStart = report.serverLog.length
  let forced = false
  current.stdin.end('shutdown\n')
  await Promise.race([exited, delay(5000)])
  if (current.exitCode === null) {
    forced = true
    current.kill()
    await Promise.race([exited, delay(5000)])
  }
  report.serverCleanups ||= []
  report.serverCleanups.push({
    pid: current.pid,
    mode: forced ? 'forced-owned-process' : 'graceful-stdin',
    exitCode: current.exitCode,
    elapsedMs: Date.now() - began,
  })
  child = undefined
  assert.equal(
    forced,
    false,
    'Rust stdin shutdown must settle within 5 seconds without force cleanup',
  )
  assert.equal(current.exitCode, 0, 'Rust stdin shutdown must exit successfully')
  assert.doesNotMatch(
    report.serverLog.slice(logStart),
    /(?:Agent|Goal|Child session|Session) shutdown failed/i,
    'Rust shutdown must not swallow an actual runtime cleanup failure',
  )
}

async function waitForHeldModel(label, present = true, timeout = 20000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    if (heldModelRequests.has(label) === present) return
    await delay(25)
  }
  assert.equal(
    heldModelRequests.has(label),
    present,
    `Actual model connection ${label} must ${present ? 'start' : 'close'} within ${timeout}ms`,
  )
}

async function waitForHeldImage(label, present = true, timeout = 20000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    if (heldImageRequests.has(label) === present) return
    await delay(25)
  }
  assert.equal(
    heldImageRequests.has(label),
    present,
    `Actual image connection ${label} must ${present ? 'start' : 'close'} within ${timeout}ms`,
  )
}

async function waitForAgentState(parentId, id, predicate, timeout = 5000) {
  const deadline = Date.now() + timeout
  let value
  while (Date.now() < deadline) {
    const data = await json(
      `/api/sessions/${parentId}/agents`,
      'GET',
      undefined,
      Math.max(1, deadline - Date.now()),
    )
    value = data.agents.find((entry) => entry.id === id)
    if (value && predicate(value)) return value
    await delay(25)
  }
  assert.fail(`Agent ${id} did not reach the required real state: ${JSON.stringify(value)}`)
}

function contentText(messages) {
  return messages
    .map((message) =>
      typeof message.content === 'string'
        ? message.content
        : Array.isArray(message.content)
          ? message.content
              .filter((part) => part.type === 'text' || part.type === 'input_text')
              .map((part) => part.text || '')
              .join('\n')
          : JSON.stringify(message.content),
    )
    .join('\n')
}

async function createFixture() {
  fixture = createServer(async (req, res) => {
    try {
      const path = new URL(req.url, 'http://fixture').pathname
      if (path === '/mcp') {
        assert.equal(req.headers.authorization, 'Bearer synthetic-mcp-test-key')
        if (req.method === 'GET') {
          res.writeHead(405)
          res.end()
          return
        }
        if (req.method === 'DELETE') {
          res.writeHead(204)
          res.end()
          return
        }
        assert.equal(req.method, 'POST')
        let raw = ''
        for await (const chunk of req) raw += chunk
        const body = JSON.parse(raw)
        report.mcpRequests.push({ method: body.method, params: body.params })
        if (body.id === undefined) {
          res.writeHead(202)
          res.end()
          return
        }
        let result
        if (body.method === 'initialize') {
          result = {
            protocolVersion: body.params.protocolVersion,
            serverInfo: { name: 'rust-usability-mcp', version: '1.0.0' },
            capabilities: { tools: {} },
          }
        } else if (body.method === 'tools/list') {
          result = {
            tools: [
              {
                name: 'echo',
                description: 'Return an isolated acceptance proof',
                inputSchema: {
                  type: 'object',
                  properties: { text: { type: 'string' } },
                  required: ['text'],
                },
              },
            ],
          }
        } else if (body.method === 'tools/call') {
          assert.equal(body.params.name, 'echo')
          assert.equal(body.params.arguments.text, 'isolated MCP proof')
          result = { content: [{ type: 'text', text: mcpSentinel }], isError: false }
        } else if (body.method === 'ping') result = {}
        else {
          res.writeHead(200, { 'Content-Type': 'application/json' })
          res.end(
            JSON.stringify({
              jsonrpc: '2.0',
              id: body.id,
              error: { code: -32601, message: 'Method not found' },
            }),
          )
          return
        }
        res.writeHead(200, { 'Content-Type': 'application/json' })
        res.end(JSON.stringify({ jsonrpc: '2.0', id: body.id, result }))
        return
      }
      if (req.method === 'GET' && path.endsWith('/models')) {
        res.writeHead(200, { 'Content-Type': 'application/json' })
        res.end(
          JSON.stringify({
            object: 'list',
            data: [{ id: modelId, object: 'model', owned_by: 'local-fixture' }],
          }),
        )
        return
      }
      if (req.method === 'POST' && /\/images\/(generations|edits)$/.test(path)) {
        const chunks = []
        for await (const chunk of req) chunks.push(chunk)
        const raw = Buffer.concat(chunks)
        const multipart = path.endsWith('/edits')
        let model,
          prompt,
          images = []
        if (multipart) {
          const form = await new Response(raw, {
            headers: { 'Content-Type': req.headers['content-type'] },
          }).formData()
          model = form.get('model')
          prompt = form.get('prompt')
          for (const [name, value] of form) {
            if (typeof value === 'string') continue
            const bytes = Buffer.from(await value.arrayBuffer())
            const decoded = PNG.sync.read(bytes)
            images.push({
              name,
              mimeType: value.type,
              size: bytes.length,
              width: decoded.width,
              height: decoded.height,
            })
          }
          assert.ok(images.length, 'Native edits must upload the actual PNG reference')
        } else {
          const body = JSON.parse(raw)
          model = body.model
          prompt = body.prompt
        }
        assert.equal(model, imageModelId)
        assert.equal(typeof prompt, 'string')
        const record = { path, model, prompt, images, imageCount: images.length, multipart }
        report.imageFixtureRequests.push(record)
        const hold = prompt.match(/native-game-hold:([a-z-]+)/)?.[1]
        if (hold) {
          record.heldLabel = hold
          await new Promise((done) => {
            heldImageRequests.set(hold, done)
            res.once('close', () => {
              heldImageRequests.delete(hold)
              done()
            })
          })
          heldImageRequests.delete(hold)
          if (res.destroyed) return
        }
        if (
          prompt.includes('native-game-fail-second-direction') &&
          /north facing|Every frame faces BACK \(back toward viewer\)/i.test(prompt)
        ) {
          record.failed = true
          res.writeHead(500, { 'Content-Type': 'application/json' })
          res.end(JSON.stringify({ error: { message: 'Synthetic second direction failure' } }))
          return
        }
        const png = new PNG({ width: 8, height: 4 })
        const background = prompt.match(/Solid (#[0-9a-f]{6}) background/i)?.[1] || '#ff00ff'
        const color = [1, 3, 5].map((start) => parseInt(background.slice(start, start + 2), 16))
        for (let y = 0; y < 4; y++) {
          for (let x = 0; x < 8; x++) {
            const offset = (y * 8 + x) * 4
            const sprite = y > 0 && y < 3 && (x === 1 || x === 2 || x === 5 || x === 6)
            const rgb = sprite ? (x < 4 ? [240, 50, 20] : [20, 80, 240]) : color
            png.data.set([...rgb, 255], offset)
          }
        }
        res.writeHead(200, { 'Content-Type': 'application/json' })
        res.end(JSON.stringify({ data: [{ b64_json: PNG.sync.write(png).toString('base64') }] }))
        return
      }
      assert.equal(req.method, 'POST')
      assert.ok(
        path.endsWith('/chat/completions') || path.endsWith('/responses'),
        `Unexpected fixture endpoint ${path}`,
      )
      let raw = ''
      for await (const chunk of req) raw += chunk
      const body = JSON.parse(raw)
      if (path.endsWith('/responses')) {
        assert.equal(body.model, modelId)
        assert.ok(
          contentText(body.input || []).includes('Rust UI usability hello'),
          'Responses UI request must contain the real typed message',
        )
        report.fixtureRequests.push({
          path,
          model: body.model,
          stream: body.stream,
          tools: (body.tools || []).map((tool) => tool.name),
          toolResults: [],
        })
        const content = 'Rust 流式聊天验收通过'
        const item = {
          id: 'msg_rust_ui_fixture',
          type: 'message',
          role: 'assistant',
          status: 'completed',
          content: [{ type: 'output_text', text: content, annotations: [] }],
        }
        const response = {
          id: 'resp_rust_ui_fixture',
          object: 'response',
          model: modelId,
          status: 'completed',
          output: [item],
          usage: {
            input_tokens: 12,
            output_tokens: 8,
            total_tokens: 20,
            input_tokens_details: { cached_tokens: 0 },
            output_tokens_details: { reasoning_tokens: 0 },
          },
        }
        if (!body.stream) {
          res.writeHead(200, { 'Content-Type': 'application/json' })
          res.end(JSON.stringify(response))
          return
        }
        res.writeHead(200, {
          'Content-Type': 'text/event-stream; charset=utf-8',
          'Cache-Control': 'no-cache',
        })
        let sequence = 0
        const event = (type, value) =>
          res.write(
            `event: ${type}\ndata: ${JSON.stringify({ type, sequence_number: sequence++, ...value })}\n\n`,
          )
        event('response.created', { response: { ...response, status: 'in_progress', output: [] } })
        event('response.output_item.added', {
          output_index: 0,
          item: { ...item, status: 'in_progress', content: [] },
        })
        event('response.output_text.delta', {
          output_index: 0,
          content_index: 0,
          item_id: item.id,
          delta: content.slice(0, 5),
        })
        await delay(40)
        event('response.output_text.delta', {
          output_index: 0,
          content_index: 0,
          item_id: item.id,
          delta: content.slice(5),
        })
        event('response.output_item.done', { output_index: 0, item })
        event('response.completed', { response })
        res.end()
        return
      }
      const messages = body.messages || []
      const lastUserIndex = messages.findLastIndex((message) => message.role === 'user')
      const lastUserText = contentText(messages.slice(lastUserIndex, lastUserIndex + 1))
      const toolResults = messages
        .slice(lastUserIndex + 1)
        .filter((message) => message.role === 'tool')
      const request = {
        path,
        model: body.model,
        stream: body.stream,
        tools: (body.tools || []).map((tool) => tool.function?.name),
        toolResults: toolResults.map((message) => ({
          tool_call_id: message.tool_call_id,
          text: contentText([message]),
        })),
        promptLabel: lastUserText.match(/rust-goal-[a-z-]+/)?.[0],
        imageCount: Array.isArray(messages[lastUserIndex]?.content)
          ? messages[lastUserIndex].content.filter((part) => part.type === 'image_url').length
          : 0,
      }
      report.fixtureRequests.push(request)
      assert.equal(body.model, modelId)
      const toolTest = contentText(messages.slice(lastUserIndex, lastUserIndex + 1)).includes(
        'rust-tool-test',
      )
      const mcpTest = contentText(messages.slice(lastUserIndex, lastUserIndex + 1)).includes(
        'rust-mcp-test',
      )
      const planTest = contentText(messages.slice(lastUserIndex, lastUserIndex + 1)).includes(
        'rust-plan-read',
      )
      const writeTest = contentText(messages.slice(lastUserIndex, lastUserIndex + 1)).includes(
        'rust-write-test',
      )
      const snapshotPayload = lastUserText.match(/rust-snapshot-tool:([A-Za-z0-9+/=]+)/)?.[1]
      const snapshotTool = snapshotPayload
        ? JSON.parse(Buffer.from(snapshotPayload, 'base64').toString('utf8'))
        : undefined
      if (snapshotTool) {
        assert.ok(
          ['read', 'write', 'edit', 'bash', 'call_tool', 'discover_tools'].includes(
            snapshotTool.name,
          ),
        )
        assert.equal(typeof snapshotTool.args, 'object')
      }
      // A restored Goal now correctly includes its old objective in hidden
      // attachment context. Only the current explicit fixture instruction may
      // hold a model stream; inherited objective text is not another request.
      const currentInstruction = lastUserText.split(
        '\n\n---\nAttachment context (injected by Pisper):\n',
      )[0]
      const hold = currentInstruction.startsWith('[Pisper internal goal continuation]')
        ? undefined
        : currentInstruction.match(/native-parity-hold:([a-z-]+)/)?.[1]
      if (hold) request.heldLabel = hold
      const goalTest =
        lastUserText.includes('rust-goal-complete') ||
        (lastUserText.includes('rust-goal-rounds') &&
          lastUserText.startsWith('[Pisper internal goal continuation]'))
      const callTool = goalTest
        ? toolResults.length < 2
        : (toolTest || mcpTest || planTest || writeTest || snapshotTool) && !toolResults.length
      const toolName = goalTest
        ? toolResults.length
          ? 'update_goal'
          : 'get_goal'
        : snapshotTool
          ? snapshotTool.name
          : mcpTest
            ? 'call_tool'
            : planTest
              ? 'get_plan'
              : writeTest
                ? 'write'
                : 'read'
      if (callTool)
        assert.ok(
          request.tools.includes(toolName),
          `Engine must expose the actual ${toolName} tool`,
        )
      const content = mcpTest
        ? `MCP 工具验收：${mcpSentinel}`
        : toolTest
          ? `工具读取验收：${sentinel}`
          : 'Rust 流式聊天验收通过'
      if (!body.stream) {
        res.writeHead(200, { 'Content-Type': 'application/json' })
        res.end(
          JSON.stringify({
            id: 'local-rust-fixture',
            object: 'chat.completion',
            model: modelId,
            choices: [{ index: 0, message: { role: 'assistant', content }, finish_reason: 'stop' }],
            usage: { prompt_tokens: 12, completion_tokens: 8, total_tokens: 20 },
          }),
        )
        return
      }
      res.writeHead(200, {
        'Content-Type': 'text/event-stream; charset=utf-8',
        'Cache-Control': 'no-cache',
      })
      const chunk = (delta, finishReason = null) =>
        res.write(
          `data: ${JSON.stringify({ id: 'local-rust-fixture', object: 'chat.completion.chunk', created: Math.floor(Date.now() / 1000), model: modelId, choices: [{ index: 0, delta, finish_reason: finishReason }] })}\n\n`,
        )
      chunk({ role: 'assistant' })
      if (hold) {
        chunk({ content: `native-held:${hold}` })
        await new Promise((done) => {
          heldModelRequests.set(hold, done)
          res.once('close', () => {
            heldModelRequests.delete(hold)
            done()
          })
        })
        heldModelRequests.delete(hold)
        if (res.destroyed) return
        chunk({ content: ':completed' })
        chunk({}, 'stop')
        res.end('data: [DONE]\n\n')
        return
      }
      if (callTool) {
        chunk({
          tool_calls: [
            {
              index: 0,
              id: 'call_rust_read_fixture',
              type: 'function',
              function: {
                name: toolName,
                arguments: JSON.stringify(
                  snapshotTool
                    ? snapshotTool.args
                    : goalTest
                      ? toolResults.length
                        ? { status: 'complete' }
                        : {}
                      : planTest
                        ? {}
                        : writeTest
                          ? {
                              path: join(workspace, 'generated-proof.txt'),
                              content: 'Native generated asset proof\n',
                            }
                          : mcpTest
                            ? { name: mcpTool, arguments: { text: 'isolated MCP proof' } }
                            : { path: join(workspace, 'read-proof.txt') },
                ),
              },
            },
          ],
        })
        chunk({}, 'tool_calls')
      } else {
        chunk({ content: content.slice(0, 5) })
        await delay(40)
        chunk({ content: content.slice(5) })
        chunk({}, 'stop')
      }
      res.write(
        `data: ${JSON.stringify({ id: 'local-rust-fixture', object: 'chat.completion.chunk', model: modelId, choices: [], usage: { prompt_tokens: 12, completion_tokens: 8, total_tokens: 20 } })}\n\n`,
      )
      res.end('data: [DONE]\n\n')
    } catch (error) {
      report.failures.push({ name: 'local model fixture', error: error.stack || String(error) })
      if (!res.headersSent) res.writeHead(500, { 'Content-Type': 'application/json' })
      res.end(JSON.stringify({ error: { message: error.message } }))
    }
  })
  await new Promise((done, reject) => {
    fixture.once('error', reject)
    fixture.listen(0, '127.0.0.1', done)
  })
  return `http://127.0.0.1:${fixture.address().port}/v1`
}

async function chat(message, id = sessionId, options = {}) {
  const response = await fetch(base + '/api/chat', {
    method: 'POST',
    headers: { Origin: base, Cookie: cookie, 'Content-Type': 'application/json' },
    body: JSON.stringify({
      sessionId: id,
      message,
      attachments: [],
      goalMode: false,
      teamMode: false,
      ...options,
    }),
    signal: AbortSignal.timeout(30000),
  })
  assert.equal(response.status, 200, 'Chat must open SSE')
  assert.match(response.headers.get('content-type') || '', /text\/event-stream/)
  const raw = await response.text()
  const events = raw
    .split(/\r?\n\r?\n/)
    .filter((frame) => frame.includes('data:'))
    .map((frame) => ({
      event: frame.match(/^event:\s*(.*)$/m)?.[1] || 'message',
      data: JSON.parse(frame.match(/^data:\s*(.*)$/m)?.[1] || 'null'),
    }))
  assert.ok(
    events.some((event) => event.event === 'run'),
    'Chat must announce replayable run',
  )
  assert.ok(
    events.some((event) => event.event === 'text_delta'),
    'Chat must deliver real text deltas',
  )
  assert.equal(
    events.some((event) => event.event === 'error'),
    false,
    JSON.stringify(events),
  )
  assert.ok(
    events.some((event) => event.event === 'done'),
    'Chat must complete',
  )
  const expected = message.includes('rust-mcp-test')
    ? mcpSentinel
    : message.includes('rust-tool-test')
      ? sentinel
      : 'Rust 流式聊天验收通过'
  assert.ok(
    events.some((event) => event.event === 'done' && event.data.text?.includes(expected)),
    `Done text must contain ${expected}: ${JSON.stringify(events)}`,
  )
  return events
}

async function waitForMcp(predicate) {
  const deadline = Date.now() + 15000
  let data
  do {
    data = await json('/api/mcp?refresh=0')
    if (predicate(data)) return data
    await delay(150)
  } while (Date.now() < deadline)
  assert.fail(`Native MCP discovery did not reach requested state: ${JSON.stringify(data)}`)
}

async function checkMcpNativeRoundtrip(label) {
  await check(label, async () => {
    const permission = await json(`/api/sessions/${sessionId}/execution-mode`, 'PUT', {
      mode: 'full-access',
    })
    assert.equal(permission.executionMode, 'full-access')
    const events = await chat('rust-mcp-test: call the isolated MCP proof')
    assert.ok(
      events.some((event) => event.event === 'tool_start' && event.data.name === 'call_tool'),
    )
    assert.ok(
      events.some(
        (event) =>
          event.event === 'tool_end' && event.data.result?.details?.gatewayToolName === mcpTool,
      ),
    )
    assert.ok(report.mcpRequests.some((request) => request.method === 'tools/call'))
    assert.ok(
      report.fixtureRequests.some((request) =>
        request.toolResults.some((result) => result.text.includes(mcpSentinel)),
      ),
      'Real MCP result must return to the model',
    )
    return {
      tool: mcpTool,
      realProtocolCalls: report.mcpRequests.filter((request) => request.method === 'tools/call')
        .length,
    }
  })
}

async function runUi(modelBaseUrl) {
  if (!desktopExecutable)
    browser = await chromium.launch({
      headless: true,
      ...(process.env.PISPER_UI_BROWSER_PATH
        ? { executablePath: process.env.PISPER_UI_BROWSER_PATH }
        : process.platform === 'win32'
          ? { channel: 'msedge' }
          : {}),
    })
  const context = desktopExecutable
    ? desktopPage.context()
    : await browser.newContext({
        viewport: { width: 1440, height: 960 },
        locale: 'zh-CN',
      })
  if (!desktopExecutable)
    await context.addCookies([
      { name: '__pisper_desktop', value: 'rust_usability_sandbox_only', url: base },
    ])
  await context.addInitScript(() => {
    if (window !== window.top) return
    localStorage.setItem('pisper-language', 'zh-CN')
    localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
  })
  await context.route('**/*', async (route) => {
    const url = new URL(route.request().url())
    if (!['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)) {
      report.deniedBrowserRequests.push(route.request().url())
      await route.abort()
    } else if (url.pathname.startsWith('/api/app-update')) {
      await route.fulfill({
        json: {
          state: 'current',
          currentVersion: '0.0.0',
          currentCommit: '0'.repeat(40),
          availableCommit: '0'.repeat(40),
          behindBy: 0,
          branch: 'develop-rust',
          notes: '',
          releaseUrl: '',
          canDownload: false,
          checkedAt: Date.now(),
        },
      })
    } else if (legacyProbe && url.pathname === '/api/config') {
      // 旧后端由源代码可证会读取 Windows 真实 KnownFolder，阻断这一个接口。
      await route.fulfill({
        json: {
          provider: '',
          model: '',
          baseUrl: null,
          maxTokens: 4096,
          contextWindow: 128000,
          hasCredential: false,
        },
      })
    } else await route.continue()
  })
  const page = desktopExecutable ? desktopPage : await context.newPage()
  page.setDefaultTimeout(12000)
  page.on('pageerror', (error) => report.pageErrors.push(error.message))
  page.on('console', (message) => {
    if (message.type() === 'error') report.consoleErrors.push(message.text())
  })
  page.on('response', (response) => {
    if (response.url().includes('/api/') && response.status() >= 400) {
      const path = new URL(response.url()).pathname
      const record = { path, status: response.status() }
      if (response.status() === 501 && disabledFeature(path))
        report.expectedUnsupportedApi.push(record)
      else report.failedApi.push(record)
    }
  })
  let navigationIndex = 0
  let captureIndex = 0
  const navigate = (path) => page.goto(base + '/?rustUsability=' + ++navigationIndex + '#/' + path)
  const uiCheck = (name, fn) =>
    check(name, async () => {
      try {
        return await fn()
      } finally {
        const label = String(++captureIndex).padStart(2, '0')
        await page.screenshot({ path: join(output, `ui-${label}.png`), fullPage: true })
        await writeFile(join(output, `ui-${label}.txt`), await page.locator('body').innerText())
        await writeFile(join(output, `ui-${label}.html`), await page.content())
        if (name.includes('create a session'))
          report.uiSessionDom = await page.evaluate(() => ({
            url: location.href,
            forms: [...document.querySelectorAll('form')].map((form) => ({
              controls: [...form.querySelectorAll('[aria-controls]')].map((node) =>
                node.getAttribute('aria-controls'),
              ),
              textareaValues: [...form.querySelectorAll('textarea')].map((node) => node.value),
            })),
            activeButtons: [...document.querySelectorAll('[aria-current], [aria-selected]')].map(
              (node) => ({
                id: node.id,
                current: node.getAttribute('aria-current'),
                selected: node.getAttribute('aria-selected'),
                label: node.textContent,
              }),
            ),
          }))
      }
    })
  await uiCheck(
    'UI: model settings render and save a connection through the real wizard',
    async () => {
      await navigate('config/models')
      await page.locator('[data-model-provider-split-panel]').waitFor()
      await page
        .locator('main > header')
        .getByRole('button', { name: '快速设置', exact: true })
        .click()
      const dialog = page.getByRole('dialog')
      await dialog.getByLabel('Base URL', { exact: true }).fill(modelBaseUrl)
      await dialog.getByRole('button', { name: '下一步', exact: true }).click()
      await dialog.getByRole('button', { name: '下一步', exact: true }).click()
      await dialog.getByLabel('显示名称', { exact: true }).fill('Rust UI Fixture')
      await dialog.locator('input[type="password"]').fill('synthetic-rust-test-key')
      await dialog.getByRole('button', { name: '获取模型', exact: true }).click()
      await dialog.getByRole('button', { name: modelId, exact: true }).click()
      await dialog.getByRole('button', { name: '保存修改', exact: true }).click()
      await dialog.waitFor({ state: 'hidden' })
      await page
        .getByRole('navigation', { name: '连接', exact: true })
        .getByRole('button', { name: 'Rust UI Fixture', exact: true })
        .waitFor()
      await page.screenshot({ path: join(output, 'model-settings.png'), fullPage: true })
      const config = await json('/api/config')
      assertConfig(config)
      const provider = config.providers.find((entry) => entry.name === 'Rust UI Fixture')
      assert.ok(provider?.configured)
      return { provider: provider.id, model: provider.defaultModel }
    },
  )
  await uiCheck('UI: create a session, send a message, and display streamed reply', async () => {
    await navigate('chat')
    const createdResponse = page
      .waitForResponse(
        (response) =>
          new URL(response.url()).pathname === '/api/sessions' &&
          response.request().method() === 'POST',
      )
      .catch((error) => error)
    await page.getByTestId('workbench-new-task').click()
    const creation = await createdResponse
    assert.ok(!(creation instanceof Error), 'New-task button must create a real session')
    const created = await creation.json()
    report.uiCreatedSession = created
    assert.equal(typeof created.id, 'string')
    // 新任务完成异步建档和 Dock 激活后再输入，避免把文字填入即将切换的旧会话。
    const form = page
      .locator('form')
      .filter({ has: page.locator(`[aria-controls="composer-tool-tray-${created.id}"]`) })
    await form.waitFor()
    await page.evaluate(
      () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
    )
    const prompt = form.getByRole('textbox', { name: '任务描述', exact: true })
    await prompt.waitFor()
    await prompt.fill('Rust UI usability hello')
    await page.evaluate(
      () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
    )
    assert.equal(
      await prompt.inputValue(),
      'Rust UI usability hello',
      'Typed message must survive new-session activation',
    )
    const sentRequest = page
      .waitForRequest(
        (request) => new URL(request.url()).pathname === '/api/chat' && request.method() === 'POST',
      )
      .catch((error) => error)
    await form.getByRole('button', { name: '发送消息', exact: true }).click()
    const submission = await sentRequest
    assert.ok(
      !(submission instanceof Error),
      'Normal send interaction must submit its real chat request',
    )
    assert.equal(
      submission.postDataJSON().sessionId,
      created.id,
      'Normal UI submission must target the newly created session',
    )
    await page
      .getByText('Rust 流式聊天验收通过', { exact: true })
      .first()
      .waitFor({ timeout: 30000 })
    await page.screenshot({ path: join(output, 'chat.png'), fullPage: true })
    const listed = await json('/api/sessions')
    assert.ok(listed.sessions.some((session) => session.messageCount >= 2))
  })
  for (const [path, heading, apiPath] of [
    ['config/interface', '设置', '/api/runtime/capabilities'],
    ['config/notifications', '设置', '/api/settings/notifications'],
    ['plugins', '插件', '/api/plugins'],
    ['mcp', 'MCP', '/api/mcp'],
    ['skills', '技能', '/api/skills'],
    ['assets', '资产', '/api/assets'],
    ['workflows', '工作流', '/api/workflows'],
    ['schedules', '定时任务', '/api/schedules'],
    ['memory', '星忆', '/api/memory'],
    ['config/interface?view=widgets', '设置', '/api/custom-ui/components'],
  ]) {
    await uiCheck(`UI page: ${path}`, async () => {
      const initialErrors = report.pageErrors.length
      const initialFailed = report.failedApi.length
      const unsupported = disabledFeature(apiPath)
      const loaded = unsupported
        ? undefined
        : page
            .waitForResponse((response) => new URL(response.url()).pathname === apiPath)
            .catch((error) => error)
      await navigate(path)
      if (unsupported) {
        await page.waitForURL((url) => url.hash === '#/chat')
        await page.getByTestId('workbench-new-task').waitFor()
        assert.equal(
          await page
            .getByTestId('workbench-sidebar')
            .getByRole('button', { name: heading, exact: true })
            .count(),
          0,
        )
        assert.equal(report.pageErrors.length, initialErrors, 'Capability fallback must not crash')
        assert.equal(
          report.failedApi.length,
          initialFailed,
          'Capability fallback must not call unavailable APIs',
        )
        const evidence = await assertUnsupported(apiPath)
        report.unsupported.push({ ...evidence, status: 'release-capability-fallback' })
        return evidence
      }
      assert.equal(
        new URL(page.url()).hash,
        '#/' + path,
        'Requested page must not redirect to chat',
      )
      await page.getByRole('heading', { name: heading, exact: true, level: 1 }).waitFor()
      const response = await loaded
      assert.ok(!(response instanceof Error), `Domain API ${apiPath} was not requested`)
      await response.finished()
      // 领域请求完成后等待 React 的两次绘制，而不把持续通知长轮询当成加载失败。
      await page.evaluate(
        () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
      )
      const text = await page.locator('body').innerText()
      const fileStem = path.replaceAll(/[^a-z0-9-]/g, '-')
      await writeFile(join(output, fileStem + '.txt'), text)
      await page.screenshot({
        path: join(output, fileStem + '.png'),
        fullPage: true,
      })
      assert.equal(
        report.pageErrors.length,
        initialErrors,
        report.pageErrors.slice(initialErrors).join('\n'),
      )
      assert.equal(
        report.failedApi.length,
        initialFailed,
        JSON.stringify(report.failedApi.slice(initialFailed)),
      )
      assert.ok(text.trim().length > 30, 'Page must render real content')
      assert.equal(
        await page.locator('main[role="alert"]').count(),
        0,
        'Route must not render its error boundary',
      )
      assert.doesNotMatch(
        text,
        /Cannot read properties|服务器响应格式异常|页面加载失败|重新加载|无法读取已安装组件/,
      )
    })
  }
  await check('UI: no uncaught React errors across core pages', () =>
    assert.deepEqual(report.pageErrors, []),
  )
  await check('UI: no failed core API calls', () => assert.deepEqual(report.failedApi, []))
  if (!desktopExecutable) {
    await context.close()
    await browser.close()
    browser = undefined
  }
}

async function runTerminalContracts() {
  const nativeEvidence = JSON.parse(await readFile(terminalEvidencePath, 'utf8'))
  const terminalBrowser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_UI_BROWSER_PATH
      ? { executablePath: process.env.PISPER_UI_BROWSER_PATH }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })
  const baseOrigin = new URL(base).origin
  const ownedPages = new Set()
  let failure
  const cleanupFailures = []
  try {
    const createIsolatedPage = async ({
      name = 'terminal-bridge-positive',
      mobile = false,
    } = {}) => {
      const context = await terminalBrowser.newContext({
        viewport: mobile ? { width: 390, height: 844 } : { width: 1440, height: 960 },
        locale: 'zh-CN',
        isMobile: mobile,
      })
      const evidence = { name, pageErrors: [], failedApi: [], deniedRequests: [], closed: false }
      report.terminalPages ||= []
      report.terminalPages.push(evidence)
      let page
      let disposing
      const owned = {
        context,
        dispose() {
          // 检验失败也必须关闭自己的 context；清理错误与测试错误分别保留。
          disposing ||= (async () => {
            const errors = []
            const validate = () => {
              for (const [label, values] of [
                ['page errors', evidence.pageErrors],
                ['unexpected API errors', evidence.failedApi],
                ['out-of-scope requests', evidence.deniedRequests],
              ]) {
                try {
                  assert.deepEqual(values, [], `${name}: ${label}`)
                } catch (error) {
                  errors.push(error)
                }
              }
            }
            validate()
            try {
              await context.close()
              evidence.closed = true
              ownedPages.delete(owned)
            } catch (error) {
              errors.push(error)
            }
            // 关闭期间到达的响应也要进入证据，不把真正的 HTTP 错误漏掉。
            if (!errors.length) validate()
            if (errors.length)
              throw new AggregateError(errors, `${name}: validation/cleanup failed`)
          })()
          return disposing
        },
      }
      ownedPages.add(owned)
      try {
        await context.addCookies([
          { name: '__pisper_desktop', value: 'rust_usability_sandbox_only', url: base },
        ])
        await context.route('**/*', async (route) => {
          const address = route.request().url()
          const url = new URL(address)
          const embedded =
            url.protocol === 'data:' || (url.protocol === 'blob:' && url.origin === baseOrigin)
          if (url.origin !== baseOrigin && !embedded) {
            evidence.deniedRequests.push(address)
            report.deniedBrowserRequests.push(address)
            await route.abort()
          } else if (url.origin === baseOrigin && url.pathname.startsWith('/api/app-update')) {
            await route.fulfill({
              json: {
                state: 'current',
                currentVersion: '0.0.0',
                currentCommit: '0'.repeat(40),
                availableCommit: '0'.repeat(40),
                behindBy: 0,
                branch: 'develop-rust',
                notes: '',
                releaseUrl: '',
                canDownload: false,
                checkedAt: Date.now(),
              },
            })
          } else await route.continue()
        })
        page = await context.newPage()
        page.on('pageerror', (error) => evidence.pageErrors.push(error.message))
        page.on('response', (response) => {
          const url = new URL(response.url())
          if (
            url.origin === baseOrigin &&
            url.pathname.startsWith('/api/') &&
            response.status() >= 400
          ) {
            evidence.failedApi.push({
              path: url.pathname,
              method: response.request().method(),
              status: response.status(),
            })
          }
        })
        owned.page = page
        return owned
      } catch (error) {
        try {
          await owned.dispose()
        } catch (cleanup) {
          throw new AggregateError([error, cleanup], `${name}: creation and cleanup failed`)
        }
        throw error
      }
    }
    const owned = await createIsolatedPage()
    let assertionFailure
    let disposalFailure
    try {
      await checkTerminalBridgeUiParity({
        check,
        page: owned.page,
        base,
        nativeEvidence,
        isolatedContext: true,
        json,
        workspace,
      })
    } catch (error) {
      assertionFailure = error
    } finally {
      try {
        await owned.dispose()
      } catch (error) {
        disposalFailure = error
      }
    }
    if (assertionFailure && disposalFailure)
      throw new AggregateError(
        [assertionFailure, disposalFailure],
        'Terminal assertion and owned page cleanup failed',
      )
    if (assertionFailure) throw assertionFailure
    if (disposalFailure) throw disposalFailure
    await checkTerminalEntrypointNegatives({ check, base, createIsolatedPage, json, workspace })
  } catch (error) {
    failure = error
  } finally {
    for (const owned of ownedPages) {
      try {
        await owned.dispose()
      } catch (error) {
        cleanupFailures.push(error)
      }
    }
    try {
      await terminalBrowser.close()
    } catch (error) {
      cleanupFailures.push(error)
    }
  }
  if (failure && cleanupFailures.length)
    throw new AggregateError(
      [failure, ...cleanupFailures],
      'Terminal contracts and browser cleanup failed',
    )
  if (failure) throw failure
  if (cleanupFailures.length)
    throw new AggregateError(cleanupFailures, 'Terminal browser/context cleanup failed')
}

async function runGameUi() {
  const bytes = await readFile(join(root, 'scripts/fixtures/pisper-game-asset-workbench.zip'))
  const sha256 = createHash('sha256').update(bytes).digest('hex')
  assert.equal(sha256, 'dfb6f1372ba9f6e4f9ff0137399f4ede74c3d15fc857ee0917a29ae80c162730')
  report.gameWorkbenchFixture = {
    repository: 'https://github.com/ling-kong-ran/pisper-components',
    commit: 'ad03ef0e62947051453663a7bbde5fc5d74eed3c',
    sha256,
  }
  const testBrowser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_UI_BROWSER_PATH
      ? { executablePath: process.env.PISPER_UI_BROWSER_PATH }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })
  const context = await testBrowser.newContext({
    viewport: { width: 1440, height: 960 },
    locale: 'zh-CN',
  })
  try {
    await context.addCookies([
      { name: '__pisper_desktop', value: 'rust_usability_sandbox_only', url: base },
    ])
    await context.addInitScript(() => {
      if (window !== window.top) return
      localStorage.setItem('pisper-language', 'zh-CN')
      localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
    })
    const denied = []
    await context.route('**/*', async (route) => {
      const url = route.request().url()
      if (/^(?:blob:|data:)/.test(url) || new URL(url).origin === new URL(base).origin) {
        await route.continue()
      } else {
        denied.push(url)
        await route.abort()
      }
    })
    const page = await context.newPage()
    const errors = []
    page.on('pageerror', (error) => errors.push(error.message))
    await checkGameAssetUiParity({
      check,
      page,
      base,
      output,
      json,
      request,
      workspace,
      agent,
      delay,
      imageProviderId,
      imageModelId,
      workbenchZip: {
        name: 'game-asset-workbench.zip',
        mimeType: 'application/zip',
        buffer: bytes,
      },
    })
    assert.deepEqual(errors, [], 'Actual game workbench must have no uncaught errors')
    assert.deepEqual(denied, [], 'The workbench must use only its isolated backend')
  } finally {
    await context.close()
    await testBrowser.close()
  }
}

try {
  await Promise.all([sandboxHome, agent, workspace].map((path) => mkdir(path, { recursive: true })))
  const digestFile = async (path) =>
    createHash('sha256')
      .update(await readFile(path))
      .digest('hex')
  report.sourceSha256 = await digestFile(executable)
  if (desktopExecutable) {
    report.desktopSha256 = await digestFile(desktopExecutable)
    report.installedSidecar = join(dirname(desktopExecutable), 'pisper-sidecar.exe')
    report.installedSidecarSha256 = await digestFile(report.installedSidecar)
    assert.equal(
      report.installedSidecarSha256,
      report.sourceSha256,
      'Installed shell must load the expected newly-built Rust server artifact',
    )
  }
  await writeFile(join(workspace, 'read-proof.txt'), sentinel + '\n')
  if (!legacyProbe)
    await writeFile(
      join(agent, 'mcp.json'),
      JSON.stringify({ fixtureRoot: { unknown: 'preserve' }, mcpServers: {} }),
    )
  const modelBaseUrl = await createFixture()
  if (legacyProbe) {
    await writeFile(
      join(agent, 'models.json'),
      JSON.stringify(
        {
          providers: {
            [providerId]: {
              baseUrl: modelBaseUrl,
              api: 'openai-completions',
              apiKey: 'synthetic-rust-test-key',
              models: [
                {
                  id: modelId,
                  name: modelId,
                  contextWindow: 128000,
                  maxTokens: 4096,
                  reasoning: false,
                  input: ['text', 'image'],
                },
              ],
            },
          },
        },
        null,
        2,
      ),
    )
  }
  const health = await check('Isolated Rust sidecar bootstrap and health', start)
  assert.ok(health, 'Cannot continue without isolated sidecar')
  if (desktopExecutable)
    await checkNativeDesktopTerminalIpcParity({ check, page: desktopPage, workspace })
  await check('MCP dashboard contract', async () => {
    const result = await json('/api/mcp?refresh=0')
    assert.ok(Array.isArray(result.services), 'MCP dashboard must contain services[]')
    return result
  })
  await check('Provider discovery contract', async () => {
    const result = await json('/api/providers/discovery')
    assert.ok(Array.isArray(result.providers))
    assert.ok(Array.isArray(result.errors))
    return result
  })
  await check('Browser preference bootstrap contract', async () => {
    const result = await json('/api/local/browser-preferences')
    assert.ok(result && typeof result === 'object')
    return result
  })
  await check('Runtime capabilities advertise actual support', async () => {
    runtimeCapabilities = await json('/api/runtime/capabilities')
    assert.ok(runtimeCapabilities.features && typeof runtimeCapabilities.features === 'object')
    assert.equal(runtimeCapabilities.features.chat, true)
    assert.equal(runtimeCapabilities.features.sessions, true)
    assert.equal(runtimeCapabilities.features.providers, true)
    report.supportedFeatures = runtimeCapabilities.features
    return runtimeCapabilities.features
  })
  await check(
    'Native speech HTTP exposes release catalog, settings and cancellation contracts',
    async () => {
      const catalog = await json('/api/speech/models')
      assert.ok(Array.isArray(catalog.models) && catalog.models.length >= 2)
      assert.ok(catalog.models.every((model) => model.id && typeof model.status === 'string'))
      assert.equal(typeof (await json('/api/settings/speech')).projectTermsEnabled, 'boolean')
      assert.equal(
        (await json('/api/settings/speech', 'PATCH', { projectTermsEnabled: false }))
          .projectTermsEnabled,
        false,
      )
      assert.ok(Array.isArray((await json('/api/speech/terms')).terms))
      const cancelled = await json('/api/speech/cancel', 'POST', {
        requestId: '00000000-0000-4000-8000-000000000001',
      })
      assert.equal(cancelled.cancelled, false)
      return { catalogModels: catalog.models.length, releaseSettings: true, cancelled }
    },
  )
  await check('Schedules dashboard contract', async () => {
    if (disabledFeature('/api/schedules')) return assertUnsupported('/api/schedules')
    const data = await json('/api/schedules')
    assert.ok(Array.isArray(data.tasks), 'Schedules must contain tasks[]')
    assert.ok(Array.isArray(data.runs), 'Schedules must contain runs[]')
    return { tasks: data.tasks.length, runs: data.runs.length }
  })
  await check('Workflows dashboard contract', async () => {
    if (disabledFeature('/api/workflows')) return assertUnsupported('/api/workflows')
    const data = await json('/api/workflows')
    for (const key of ['workflows', 'runs', 'models', 'skills'])
      assert.ok(Array.isArray(data[key]), `Workflows must contain ${key}[]`)
    assert.equal(typeof data.limits?.running, 'number')
    assert.equal(typeof data.limits?.maxConcurrent, 'number')
    assert.equal(typeof data.notificationTargets?.browser?.enabled, 'boolean')
    return { workflows: data.workflows.length, runs: data.runs.length }
  })
  await check('Memory dashboard contract', async () => {
    const data = await json('/api/memory')
    for (const key of ['spaces', 'nodes', 'links', 'candidates'])
      assert.ok(Array.isArray(data[key]), `Memory must contain ${key}[]`)
    assert.equal(typeof data.selectedSpaceId, 'string')
    return { spaces: data.spaces.length, nodes: data.nodes.length }
  })
  await check(
    'Memory HTTP creates, searches, updates and deletes real SQLite records',
    async () => {
      const dashboard = await json('/api/memory')
      const memory = await json('/api/memory/nodes', 'POST', {
        spaceId: dashboard.selectedSpaceId,
        title: 'Isolated memory proof',
        content: 'Native SQLite persistence proof',
      })
      assert.equal(typeof memory.id, 'string')
      const found = await json('/api/memory?query=SQLite')
      assert.ok(found.nodes.some((node) => node.id === memory.id))
      const updated = await json(`/api/memory/nodes/${memory.id}`, 'PATCH', {
        title: 'Updated proof',
        content: 'Native SQLite updated proof',
      })
      assert.equal(updated.title, 'Updated proof')
      assert.equal((await json(`/api/memory/nodes/${memory.id}`, 'DELETE')).deleted, true)
      assert.ok(!(await json('/api/memory')).nodes.some((node) => node.id === memory.id))
      return { realSqliteCrud: true }
    },
  )
  await check('Assets HTTP uploads, deduplicates, previews and streams a byte range', async () => {
    const asset = await json('/api/assets', 'POST', {
      name: 'proof.txt',
      text: 'Native asset proof',
    })
    assert.equal(typeof asset.id, 'string')
    const duplicate = await json('/api/assets', 'POST', {
      name: 'proof-copy.txt',
      text: 'Native asset proof',
    })
    assert.equal(duplicate.id, asset.id)
    assert.equal((await json(`/api/assets/${asset.id}/content`)).text, 'Native asset proof')
    const range = await fetch(base + `/api/assets/${asset.id}/download`, {
      headers: { Cookie: cookie, Range: 'bytes=0-5' },
      signal: AbortSignal.timeout(10000),
    })
    assert.equal(range.status, 206)
    assert.equal(await range.text(), 'Native')
    assert.equal((await json(`/api/assets/${asset.id}`, 'DELETE')).deleted, true)
    return { realFileUpload: true, deduplicated: true, streamedRange: true }
  })
  if (!skipUi) await check('Browser execution', () => runUi(modelBaseUrl))
  if (!skipUi && terminalEvidencePath)
    await check('Production terminal bridge and platform entry contracts', runTerminalContracts)
  if (!legacyProbe) {
    await check('Frontend config schema', async () => {
      const config = await json('/api/config')
      assertConfig(config)
      return { providerCount: config.providers.length }
    })
    await check('Discover a connection against local /v1/models', async () => {
      const data = await json('/api/providers/models/discover-connection', 'POST', {
        providerType: 'chat',
        api: 'openai-completions',
        baseUrl: modelBaseUrl,
        apiKey: 'synthetic-rust-test-key',
      })
      assert.ok(data.models.some((model) => model.id === modelId && model.kind === 'chat'))
      return data
    })
    await check('Create and save a usable provider connection', async () => {
      const data = await json('/api/providers', 'POST', {
        id: providerId,
        name: 'Rust API Fixture',
        providerType: 'chat',
        api: 'openai-completions',
        baseUrl: modelBaseUrl,
        apiKey: 'synthetic-rust-test-key',
        model: modelId,
        modelKind: 'chat',
        enabled: true,
      })
      assertConfig(data)
      const provider = data.providers.find((entry) => entry.id === providerId)
      assert.equal(provider.configured, true)
      assert.equal(provider.baseUrl, modelBaseUrl)
      await json(`/api/providers/${providerId}/models/options`, 'PUT', {
        id: modelId,
        input: ['text', 'image'],
      })
    })
    await check('Save default provider/model through React PUT contract', async () => {
      const data = await json('/api/config', 'PUT', {
        provider: providerId,
        model: modelId,
        setAsDefault: true,
      })
      assertConfig(data)
      assert.equal(data.defaultProvider || data.provider, providerId)
      assert.equal(data.defaultModel || data.model, modelId)
    })
  }
  await check('Create a real session with frontend summary', async () => {
    const created = await json('/api/sessions', 'POST', {
      name: 'Rust usability API session',
      cwd: workspace,
    })
    assert.equal(typeof created.id, 'string')
    sessionId = created.id
    assert.equal(created.name, 'Rust usability API session')
    assert.equal(created.cwd, workspace)
    assert.equal(created.model, `${providerId}/${modelId}`)
    return created
  })
  if (sessionId) {
    await check('Live session is JSON with usable messages/pageInfo', async () => {
      const data = await json(`/api/sessions/${sessionId}/live`, 'GET', undefined, 5000)
      assertMessages(data)
      return { count: data.messages.length }
    })
    await check('Plain chat produces real streamed assistant text', () =>
      chat('Rust API usability hello'),
    )
    await check('History contains the user and assistant conversation', async () => {
      const data = await json(`/api/sessions/${sessionId}/messages?limit=50`)
      assertMessages(data)
      assert.ok(
        data.messages.some(
          (message) => message.role === 'user' && message.text.includes('Rust API usability hello'),
        ),
      )
      assert.ok(
        data.messages.some(
          (message) => message.role === 'agent' && message.text.includes('Rust 流式聊天验收通过'),
        ),
      )
      return data
    })
    await check(
      'Real tool roundtrip executes sandbox read and returns result to model',
      async () => {
        const events = await chat('rust-tool-test: read the isolated proof file')
        assert.ok(
          events.some((event) => event.event === 'tool_start' && event.data.name === 'read'),
        )
        assert.ok(events.some((event) => event.event === 'tool_end'))
        assert.ok(
          report.fixtureRequests.some((request) =>
            request.toolResults.some((result) => result.text.includes(sentinel)),
          ),
          'Model must receive real read output',
        )
        return events
      },
    )
    if (!legacyProbe) {
      await check(
        'Native write archives real workspace output and restores message attachments',
        async () => {
          await json(`/api/sessions/${sessionId}/execution-mode`, 'PUT', { mode: 'full-access' })
          const events = await chat('rust-write-test: create the isolated generated asset proof')
          assert.equal(
            await readFile(join(workspace, 'generated-proof.txt'), 'utf8'),
            'Native generated asset proof\n',
          )
          const write = events.find(
            (event) => event.event === 'tool_start' && event.data.name === 'write',
          )
          assert.ok(write, 'The actual native write tool must start')
          assert.ok(
            events.some(
              (event) =>
                event.event === 'tool_end' &&
                event.data.id === write.data.id &&
                event.data.error === false,
            ),
            'The same write call must finish successfully',
          )
          const generated = events.find(
            (event) =>
              event.event === 'generated_asset' && event.data.name === 'generated-proof.txt',
          )
          assert.ok(generated, 'Actual generated file must be emitted before done')
          const download = await fetch(new URL(generated.data.downloadUrl, base), {
            headers: { Cookie: '__pisper_desktop=rust_usability_sandbox_only' },
          })
          assert.equal(download.status, 200)
          assert.equal(await download.text(), 'Native generated asset proof\n')
          const snapshot = await json(`/api/sessions/${sessionId}/live`)
          assert.ok(
            snapshot.messages.some(
              (message) =>
                message.role === 'agent' &&
                message.attachments?.some((item) => item.id === generated.data.id),
            ),
          )
          const history = await json(`/api/sessions/${sessionId}/messages`)
          assert.ok(
            history.messages.some(
              (message) =>
                message.role === 'agent' &&
                message.attachments?.some((item) => item.id === generated.data.id),
            ),
          )
          return { asset: generated.data.id, persistedAttachment: true }
        },
      )
      await check('MCP add preserves unknown config and exposes real native tools', async () => {
        const data = await json('/api/mcp', 'POST', {
          spec: JSON.stringify({
            name: mcpId,
            url: modelBaseUrl.replace('/v1', '/mcp?token=synthetic-mcp-query-secret'),
            headers: { Authorization: 'Bearer synthetic-mcp-test-key' },
            exposure: 'direct',
            fixtureUnknown: { preserved: true },
          }),
        })
        assert.ok(data.services.some((service) => service.id === mcpId && service.enabled))
        assert.equal(
          data.services.find((service) => service.id === mcpId).status,
          'unverified',
          'Native state must not be invented from config',
        )
        const dashboard = await waitForMcp((result) =>
          result.tools.some((tool) => tool.piName === mcpTool && tool.enabled),
        )
        const saved = JSON.parse(await readFile(join(agent, 'mcp.json'), 'utf8'))
        assert.deepEqual(saved.fixtureRoot, { unknown: 'preserve' })
        assert.deepEqual(saved.mcpServers[mcpId].fixtureUnknown, { preserved: true })
        return dashboard
      })
      await check(
        'MCP connection test performs actual initialize/list and redacts secrets',
        async () => {
          const before = report.mcpRequests.length
          const data = await json(`/api/mcp/${mcpId}/test`, 'POST', {})
          assert.equal(data.test?.ok, true)
          assert.equal(data.test?.toolCount, 1)
          const service = data.services.find((service) => service.id === mcpId)
          assert.equal(service.status, 'online')
          assert.equal(service.statusSource, 'explicit-connection-test')
          assert.equal(service.authCount, 1)
          assert.ok(
            report.mcpRequests.slice(before).some((request) => request.method === 'initialize'),
          )
          assert.ok(
            report.mcpRequests.slice(before).some((request) => request.method === 'tools/list'),
          )
          assert.doesNotMatch(
            JSON.stringify(data),
            /synthetic-mcp-test-key|synthetic-mcp-query-secret/,
          )
          assert.equal(data.tools.find((tool) => tool.piName === mcpTool).name, 'echo')
          return data.test
        },
      )
      await checkMcpNativeRoundtrip(
        'MCP native engine executes actual tool and returns result to model',
      )
      await check(
        'MCP exact tool permission reload preserves config and real tool catalog',
        async () => {
          await json(`/api/mcp/${mcpId}/tools/echo`, 'PATCH', { enabled: false })
          await waitForMcp(
            (result) => !result.tools.some((tool) => tool.piName === mcpTool && tool.enabled),
          )
          const disabled = await json('/api/plugins')
          assert.ok(
            !disabled.callableToolNames.includes(mcpTool),
            'Hidden MCP tool must not remain callable',
          )
          await json(`/api/mcp/${mcpId}/tools/echo`, 'PATCH', { enabled: true })
          await waitForMcp((result) =>
            result.tools.some((tool) => tool.piName === mcpTool && tool.enabled),
          )
          const saved = JSON.parse(await readFile(join(agent, 'mcp.json'), 'utf8'))
          assert.deepEqual(saved.fixtureRoot, { unknown: 'preserve' })
          assert.deepEqual(saved.mcpServers[mcpId].fixtureUnknown, { preserved: true })
          assert.equal(saved.mcpServers[mcpId].toolExposure.echo, 'direct')
        },
      )
      await check(
        'MCP server disable/enable reload actually unregisters/registers tools',
        async () => {
          await json(`/api/mcp/${mcpId}`, 'PATCH', { enabled: false })
          const data = await waitForMcp(
            (result) =>
              result.services.some(
                (service) => service.id === mcpId && service.status === 'disabled',
              ) && !result.tools.some((tool) => tool.piName === mcpTool && tool.enabled),
          )
          const disabled = await json('/api/plugins')
          assert.ok(!disabled.callableToolNames.includes(mcpTool))
          await json(`/api/mcp/${mcpId}`, 'PATCH', { enabled: true })
          await waitForMcp((result) =>
            result.tools.some((tool) => tool.piName === mcpTool && tool.enabled),
          )
          await json(`/api/mcp/${mcpId}/test`, 'POST', {})
          return { disabled: data.services.find((service) => service.id === mcpId).status }
        },
      )
    }
    await check('Session switching preserves prior history and continued chat', async () => {
      const other = await json('/api/sessions', 'POST', { name: 'Rust second session' })
      await chat('second session hello', other.id)
      const original = await json(`/api/sessions/${sessionId}/messages?limit=50`)
      assertMessages(original)
      assert.ok(
        original.messages.some((message) => message.text.includes('Rust API usability hello')),
      )
      await chat('continued original session')
    })
    if (!legacyProbe) {
      await check(
        'Native sessions run concurrently and enforce mutation/configuration isolation',
        async () => {
          const first = await json('/api/sessions', 'POST', { name: 'Native concurrent A' })
          const second = await json('/api/sessions', 'POST', { name: 'Native concurrent B' })
          const open = (id, message) =>
            fetch(base + '/api/chat', {
              method: 'POST',
              headers: { Origin: base, Cookie: cookie, 'Content-Type': 'application/json' },
              body: JSON.stringify({ sessionId: id, message }),
              // 流的总时限包含两次新建 Pi runtime；取消 API 另用五秒硬限验收。
              signal: AbortSignal.timeout(60000),
            })
          const a = await open(first.id, 'native-parity-hold:session-a')
          const b = await open(second.id, 'native-parity-hold:session-b')
          assert.equal(a.status, 200)
          assert.equal(b.status, 200)
          const aText = a.text()
          const bText = b.text()
          void aText.catch(() => {})
          void bText.catch(() => {})
          try {
            const deadline = Date.now() + 20000
            while (
              Date.now() < deadline &&
              !(heldModelRequests.has('session-a') && heldModelRequests.has('session-b'))
            )
              await delay(25)
            assert.ok(
              heldModelRequests.has('session-a') && heldModelRequests.has('session-b'),
              'Both real Pi model calls must overlap before either one finishes',
            )
            const duplicate = await open(first.id, 'same session concurrent request')
            assert.equal(duplicate.status, 409)
            assert.equal((await duplicate.json()).code, 'session_busy')
            const config = await fetch(base + '/api/config', {
              method: 'PUT',
              headers: { Origin: base, Cookie: cookie, 'Content-Type': 'application/json' },
              body: JSON.stringify({ provider: providerId, model: modelId }),
            })
            assert.equal(config.status, 409)
            await json(`/api/sessions/${first.id}/abort`, 'POST', {}, 5000)
            const stopped = await aText
            assert.match(stopped, /"aborted":true/)
            assert.ok(!stopped.includes('native-held:session-b'))
            const live = await json(`/api/sessions/${second.id}/live`)
            assert.equal(live.streaming, true, 'Stopping A must keep B active')
            heldModelRequests.get('session-b')()
            const completed = await bText
            assert.match(completed, /native-held:session-b:completed/)
            assert.ok(!completed.includes('native-held:session-a'))
            return { first: first.id, second: second.id, overlap: true, stopIsolated: true }
          } finally {
            for (const label of ['session-a', 'session-b']) heldModelRequests.get(label)?.()
            await Promise.allSettled([aText, bText])
          }
        },
      )
      await check(
        'Native plan HTTP, real Pi tool, child parent-plan reading and usage accounting',
        async () => {
          const updated = await json(`/api/sessions/${sessionId}/plan`, 'PUT', {
            items: [{ id: 'proof', title: 'Native plan parity proof', status: 'in_progress' }],
          })
          assert.equal(updated.plan.items[0].title, 'Native plan parity proof')
          const events = await chat('rust-plan-read: inspect the current parent plan')
          const realRead = events.find((event) => event.event === 'tool_end' && event.data.output)
          assert.ok(realRead, 'A real Pi get_plan call must return an actual tool result')
          assert.deepEqual(
            JSON.parse(realRead.data.output).plan.items,
            [],
            'An ordinary new message clears stale plans, matching release',
          )
          await json(`/api/sessions/${sessionId}/plan`, 'PUT', {
            items: [{ id: 'proof', title: 'Native plan parity proof', status: 'in_progress' }],
          })
          const usageBefore = await json('/api/usage/today')
          const fixtureStart = report.fixtureRequests.length
          const created = await json(`/api/sessions/${sessionId}/agents`, 'POST', {
            taskName: 'plan-proof',
            message: 'rust-plan-read: inspect the delegated parent plan and return the proof',
          })
          const child = created.agent
          assert.ok(child.id)
          const deadline = Date.now() + 20000
          let completed
          while (Date.now() < deadline) {
            const data = await json(`/api/sessions/${sessionId}/agents`)
            completed = data.agents.find((agent) => agent.id === child.id)
            if (['completed', 'failed', 'interrupted'].includes(completed?.status)) break
            await delay(50)
          }
          assert.equal(completed.status, 'completed', JSON.stringify(completed))
          assert.ok(
            completed.usage.totalTokens > 0,
            'Child must account actual reported model usage',
          )
          assert.ok(completed.availableTools.includes('get_plan'))
          assert.ok(!completed.availableTools.includes('update_plan'))
          assert.ok(!completed.availableTools.includes('spawn_agent'))
          assert.ok(
            report.fixtureRequests
              .slice(fixtureStart)
              .some((request) =>
                request.toolResults?.some((result) =>
                  result.text.includes('Native plan parity proof'),
                ),
              ),
            'The child actual get_plan result must contain the parent plan, not its own empty plan',
          )
          assert.equal(
            (await json(`/api/sessions/${sessionId}/plan`)).plan.items[0].status,
            'in_progress',
          )
          const requiredUsage = completed.runUsage.totalTokens
          assert.ok(requiredUsage > 0, 'Child run must expose actual nonzero reported usage')
          const usageDeadline = Date.now() + 5000
          let today
          do {
            today = await json('/api/usage/today')
            if (today.totalTokens >= usageBefore.totalTokens + requiredUsage) break
            await delay(25)
          } while (Date.now() < usageDeadline)
          assert.ok(
            today.totalTokens >= usageBefore.totalTokens + requiredUsage,
            `Global ledger must add this private child run: before=${usageBefore.totalTokens}, child=${requiredUsage}, after=${today.totalTokens}`,
          )
          return { child: child.id, usage: completed.usage, today }
        },
      )
      await check(
        'Goal continuation executes real Pi tools, preserves first-round images and hides internal prompts',
        async () => {
          const created = await json('/api/sessions', 'POST', { cwd: workspace })
          const fixtureStart = report.fixtureRequests.length
          const events = await chat(
            'rust-goal-rounds: complete after a real continuation round',
            created.id,
            {
              goalMode: true,
              goalTokenBudget: 1000,
              attachments: [
                {
                  kind: 'image',
                  name: 'goal.png',
                  mimeType: 'image/png',
                  data: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jZQAAAABJRU5ErkJggg==',
                },
              ],
            },
          )
          assert.ok(
            events.some((event) => event.event === 'tool_start' && event.data.name === 'get_goal'),
          )
          assert.ok(
            events.some(
              (event) => event.event === 'tool_start' && event.data.name === 'update_goal',
            ),
          )
          assert.ok(
            events.some(
              (event) => event.event === 'goal_update' && event.data.goal?.status === 'complete',
            ),
          )
          const goal = (await json(`/api/sessions/${created.id}/goal`)).goal
          assert.equal(goal.status, 'complete')
          assert.ok(goal.tokensUsed >= 20)
          const requests = report.fixtureRequests
            .slice(fixtureStart)
            .filter((request) => request.promptLabel === 'rust-goal-rounds')
          assert.ok(
            requests.length >= 3,
            'The real model must receive a continuation and tool result',
          )
          assert.equal(
            requests[0].imageCount,
            1,
            'Prepared images reach the first real Goal request',
          )
          assert.ok(
            requests.slice(1).every((request) => request.imageCount === 0),
            'Continuations must not resend initial images',
          )
          const live = await json(`/api/sessions/${created.id}/live`)
          assert.ok(
            live.messages.every(
              (message) => message.role !== 'user' || !message.text.includes('[Pisper internal'),
            ),
          )
          return { sessionId: created.id, goal, actualRequests: requests.length }
        },
      )
      await check(
        'Goal budget stops continuation and runs exactly one real summary round',
        async () => {
          const created = await json('/api/sessions', 'POST', { cwd: workspace })
          const fixtureStart = report.fixtureRequests.length
          await chat(
            'rust-goal-budget: summarize after the explicit budget is exhausted',
            created.id,
            { goalMode: true, goalTokenBudget: 10 },
          )
          const goal = (await json(`/api/sessions/${created.id}/goal`)).goal
          assert.equal(goal.status, 'budget_limited')
          assert.equal(goal.tokensUsed, 20)
          const requests = report.fixtureRequests
            .slice(fixtureStart)
            .filter((request) => request.promptLabel === 'rust-goal-budget')
          assert.equal(
            requests.length,
            2,
            'One initial model request and one budget summary, with no further loop',
          )
          await json(`/api/sessions/${created.id}/goal`, 'PATCH', {
            action: 'set-budget',
            tokenBudget: 100,
          })
          assert.equal((await json(`/api/sessions/${created.id}/goal`)).goal.status, 'paused')
          return { sessionId: created.id, realRounds: requests.length, goal }
        },
      )
      await check(
        'Team mode boots the native coordinator and exposes its real HTTP projection',
        async () => {
          const created = await json('/api/sessions', 'POST', { cwd: workspace })
          await json(`/api/sessions/${created.id}/run-mode`, 'PUT', { mode: 'team' })
          const events = await chat(
            'rust-goal-complete: verify a native Team goal and complete',
            created.id,
            { teamMode: true },
          )
          const goal = (await json(`/api/sessions/${created.id}/goal`)).goal
          const team = (await json(`/api/sessions/${created.id}/team`)).team
          assert.equal(goal.mode, 'team')
          assert.equal(goal.status, 'complete')
          assert.equal(goal.teamTokenBudget, null)
          assert.equal(team.status, 'complete')
          assert.ok(events.some((event) => event.event === 'team_update'))
          const live = await json(`/api/sessions/${created.id}/live`)
          assert.equal(live.runMode, 'team')
          assert.equal(live.team.id, team.id)
          return { sessionId: created.id, goalId: goal.id, teamId: team.id }
        },
      )
      await check(
        'Stopping a held Goal closes its model stream and resumes the same objective explicitly',
        async () => {
          const created = await json('/api/sessions', 'POST', { cwd: workspace })
          const response = await fetch(base + '/api/chat', {
            method: 'POST',
            headers: { Origin: base, Cookie: cookie, 'Content-Type': 'application/json' },
            body: JSON.stringify({
              sessionId: created.id,
              message: 'native-parity-hold:goal-stop',
              goalMode: true,
            }),
            signal: AbortSignal.timeout(60000),
          })
          assert.equal(response.status, 200)
          const pending = response.text()
          pending.catch(() => {})
          try {
            await waitForHeldModel('goal-stop')
            const duplicate = await fetch(base + '/api/chat', {
              method: 'POST',
              headers: { Origin: base, Cookie: cookie, 'Content-Type': 'application/json' },
              body: JSON.stringify({ sessionId: created.id, message: 'concurrent request' }),
            })
            assert.equal(duplicate.status, 409)
            const before = (await json(`/api/sessions/${created.id}/goal`)).goal
            await json(`/api/sessions/${created.id}/abort`, 'POST', {}, 5000)
            assert.match(await pending, /"aborted":true/)
            await waitForHeldModel('goal-stop', false)
            assert.equal((await json(`/api/sessions/${created.id}/goal`)).goal.status, 'paused')
            await chat('rust-goal-complete: resume and complete this same objective', created.id, {
              goalMode: true,
            })
            const after = (await json(`/api/sessions/${created.id}/goal`)).goal
            assert.equal(after.id, before.id)
            assert.equal(after.objective, before.objective)
            assert.equal(after.status, 'complete')
            return {
              sessionId: created.id,
              goalId: after.id,
              streamClosed: true,
              explicitResume: true,
            }
          } finally {
            heldModelRequests.get('goal-stop')?.()
            const current = await json(`/api/sessions/${created.id}/goal`)
            if (current.goal?.status === 'active') {
              await json(`/api/sessions/${created.id}/abort`, 'POST', {}, 5000)
              await waitForHeldModel('goal-stop', false)
            }
            await Promise.allSettled([pending])
          }
        },
      )
      await check(
        'Held child interrupt closes the real stream and preserves follow-up context',
        async () => {
          const label = 'child-interrupt'
          try {
            const created = await json(`/api/sessions/${sessionId}/agents`, 'POST', {
              taskName: 'held-child-interrupt',
              message: `native-parity-hold:${label}`,
            })
            const owned = created.agent
            assert.ok(owned.id)
            await waitForHeldModel(label)
            const interrupted = await json(
              `/api/sessions/${sessionId}/agents/${owned.id}/interrupt`,
              'POST',
              {},
              5000,
            )
            assert.equal(interrupted.status, 'interrupted')
            await waitForHeldModel(label, false)
            await waitForAgentState(sessionId, owned.id, (value) => value.status === 'interrupted')
            const followup = await json(
              `/api/sessions/${sessionId}/agents/${owned.id}/followup`,
              'POST',
              { message: 'rust-plan-read: verify the preserved parent plan after interruption' },
              5000,
            )
            assert.equal(followup.id, owned.id, 'Follow-up must reuse the same owned child context')
            const completed = await waitForAgentState(sessionId, owned.id, (value) =>
              ['completed', 'failed', 'interrupted'].includes(value.status),
            )
            assert.equal(completed.status, 'completed', JSON.stringify(completed))
            assert.ok(
              completed.runNumber >= 2,
              'Follow-up must actually execute after releasing the cancelled run',
            )
            return {
              child: owned.id,
              interrupted: true,
              modelConnectionClosed: true,
              followupRun: completed.runNumber,
            }
          } finally {
            heldModelRequests.get(label)?.()
          }
        },
      )
      await check(
        'Parent abort cascades to its held children and preserves another parent child',
        async () => {
          const labels = ['cascade-child-a', 'cascade-child-b', 'independent-child']
          let parent
          try {
            parent = await json('/api/sessions', 'POST', {
              name: 'Held child cascade parent',
            })
            const children = []
            for (const label of labels.slice(0, 2)) {
              const created = await json(`/api/sessions/${parent.id}/agents`, 'POST', {
                taskName: label,
                message: `native-parity-hold:${label}`,
              })
              assert.ok(created.agent.id)
              children.push(created.agent)
            }
            const independent = await json(`/api/sessions/${sessionId}/agents`, 'POST', {
              taskName: labels[2],
              message: `native-parity-hold:${labels[2]}`,
            })
            assert.ok(independent.agent.id)
            await Promise.all(labels.map((label) => waitForHeldModel(label)))
            await json(`/api/sessions/${parent.id}/abort`, 'POST', {}, 5000)
            for (const [index, owned] of children.entries()) {
              await waitForHeldModel(labels[index], false)
              await waitForAgentState(
                parent.id,
                owned.id,
                (value) => value.status === 'interrupted',
              )
            }
            assert.ok(
              heldModelRequests.has(labels[2]),
              'Another parent actual child stream must remain open',
            )
            const other = await waitForAgentState(
              sessionId,
              independent.agent.id,
              (value) => value.status === 'running',
            )
            heldModelRequests.get(labels[2])()
            const completed = await waitForAgentState(sessionId, other.id, (value) =>
              ['completed', 'failed', 'interrupted'].includes(value.status),
            )
            assert.equal(completed.status, 'completed', JSON.stringify(completed))
            await chat('cascade parent can chat again after child cancellation', parent.id)
            return {
              parent: parent.id,
              interruptedChildren: children.map((value) => value.id),
              unrelatedChildCompleted: other.id,
            }
          } finally {
            for (const label of labels) heldModelRequests.get(label)?.()
            if (parent) await json(`/api/sessions/${parent.id}/abort`, 'POST', {}, 5000)
            await json(`/api/sessions/${sessionId}/abort`, 'POST', {}, 5000)
          }
        },
      )
    }
    if (!legacyProbe && runtimeCapabilities?.features?.workflows) {
      await checkWorkflowParity({
        check,
        json,
        request,
        workspace,
        agent,
        providerId,
        modelId,
        heldModelRequests,
        waitForHeldModel,
        delay,
      })
      notificationRestart = await checkNotificationParity({
        check,
        json,
        request,
        workspace,
        agent,
        providerId,
        modelId,
        delay,
      })
      fileChangeRestart = await checkFileChangeParity({
        check,
        json,
        request,
        chat,
        workspace,
        agent,
        providerId,
        modelId,
        delay,
        waitForHeldModel,
        heldModelRequests,
      })
      await checkSessionProjectionReads({ check, json, request, workspace, agent })
      await checkWebSearchParity({ check, json, request, chat, workspace, providerId, modelId })
      customUiRestart = await checkCustomUiParity({ check, json, request, workspace, agent, delay })
      gameAssetRestart = await checkGameAssetParity({
        check,
        json,
        request,
        workspace,
        agent,
        delay,
        imageProviderId,
        imageModelId,
        imageBaseUrl: modelBaseUrl,
        imageFixtureRequests: report.imageFixtureRequests,
        heldImageRequests,
        waitForHeldImage,
      })
      if (!skipUi) await check('Browser game workbench execution', runGameUi)
      await checkImageAgentParity({
        check,
        json,
        request,
        chat,
        workspace,
        agent,
        output,
        providerId,
        modelId,
        imageProviderId,
        imageModelId,
        imageBaseUrl: modelBaseUrl,
        imageFixtureRequests: report.imageFixtureRequests,
        fixtureRequests: report.fixtureRequests,
        delay,
      })
      // 插件关机验收保留真实运行中的 worker；先完成需要空闲会话的 provider 写入。
      await checkVisualParity({
        check,
        json,
        request,
        chat,
        workspace,
        agent,
        output,
        providerId,
        modelId,
        imageProviderId,
        imageModelId,
        imageBaseUrl: modelBaseUrl,
        imageFixtureRequests: report.imageFixtureRequests,
        fixtureRequests: report.fixtureRequests,
        delay,
      })
      channelsRestart = await checkChannelsParity({
        check,
        json,
        request,
        chat,
        workspace,
        agent,
        output,
        providerId,
        modelId,
        fixtureRequests: report.fixtureRequests,
        delay,
      })
      browserRestart = await checkBrowserParity({
        check,
        json,
        request,
        chat,
        workspace,
        agent,
        output,
        providerId,
        modelId,
        fixtureRequests: report.fixtureRequests,
        delay,
      })
      pluginsRestart = await checkPluginsParity({
        check,
        json,
        request,
        chat,
        workspace,
        agent,
        providerId,
        modelId,
        delay,
      })
    }
    await check('Restart persists config, sessions, history and functioning chat', async () => {
      const label = 'shutdown-child'
      const scheduleLabel = 'shutdown-schedule'
      let pendingChild
      let heldStarts
      let pendingSchedule
      let pendingScheduleSession
      let heldScheduleStarts
      try {
        if (!legacyProbe) {
          const created = await json(`/api/sessions/${sessionId}/agents`, 'POST', {
            taskName: 'held-child-shutdown',
            message: `native-parity-hold:${label}`,
          })
          pendingChild = created.agent
          assert.ok(pendingChild.id)
          await waitForHeldModel(label)
          await waitForAgentState(sessionId, pendingChild.id, (value) => value.status === 'running')
          heldStarts = report.fixtureRequests.filter(
            (request) => request.heldLabel === label,
          ).length
          assert.equal(
            heldStarts,
            1,
            'One real child model call must still be held before shutdown',
          )
          const scheduled = await json('/api/schedules', 'POST', {
            name: 'native-schedule-held-shutdown',
            targetType: 'prompt',
            prompt: `native-parity-hold:${scheduleLabel}`,
            enabled: false,
            frequency: 'interval',
            intervalValue: 1,
            intervalUnit: 'hours',
            timezone: 'Asia/Seoul',
            cwd: workspace,
            model: { provider: providerId, model: modelId },
            notifications: [],
          })
          pendingSchedule = scheduled.task
          const scheduledRun = await json(`/api/schedules/${pendingSchedule.id}/run`, 'POST', {})
          assert.equal(scheduledRun.started, true)
          await waitForHeldModel(scheduleLabel)
          const catalog = await json('/api/sessions')
          pendingScheduleSession = catalog.sessions.find(
            (entry) => entry.name === `定时任务 · ${pendingSchedule.name}`,
          )?.id
          assert.ok(pendingScheduleSession, 'The actual scheduled model must own a public session')
          heldScheduleStarts = report.fixtureRequests.filter(
            (request) => request.heldLabel === scheduleLabel,
          ).length
          assert.equal(heldScheduleStarts, 1)
        }
        // 其他保留夹具会按预期更新全局插件设置；在这些修改完成后、停机前
        // 冻结通道重启要保护的原始配置字节，避免混入插件夹具的清理写入。
        await channelsRestart?.prepareRestart?.()
        await browserRestart?.prepareRestart?.()
        await stop()
        if (pendingSchedule) {
          await waitForHeldModel(scheduleLabel, false)
          const saved = JSON.parse(await readFile(join(agent, 'pisper-schedules.json'), 'utf8'))
          const task = saved.tasks.find((entry) => entry.id === pendingSchedule.id)
          const run = saved.runs.find((entry) => entry.taskId === pendingSchedule.id)
          assert.ok(task && run, 'Shutdown must persist the active schedule and its run')
          assert.ok(
            desktopExecutable
              ? ['running', 'interrupted'].includes(run.status)
              : run.status === 'interrupted',
            `Shutdown must persist a truthful schedule status: ${run.status}`,
          )
          if (!desktopExecutable) assert.equal(task.lastStatus, 'interrupted')
        }
        if (pendingChild) {
          await waitForHeldModel(label, false)
          const saved = JSON.parse(await readFile(join(agent, 'pisper-agents.json'), 'utf8'))
          const record = saved.records.find((value) => value.id === pendingChild.id)
          assert.ok(record, 'The active child must have a durable restart record')
          // GUI 进程树清理可能是强制的；正常 stdin 关机必须在退出前持久化中断状态。
          assert.ok(
            desktopExecutable
              ? ['running', 'starting', 'interrupted'].includes(record.status)
              : record.status === 'interrupted',
            `Shutdown must persist a truthful child status: ${JSON.stringify(record)}`,
          )
        }
        await start()
        if (pendingChild) {
          const recovered = await waitForAgentState(
            sessionId,
            pendingChild.id,
            (value) => value.status === 'interrupted',
          )
          await delay(750)
          assert.equal(
            heldModelRequests.has(label),
            false,
            'Restart must not automatically resume a stopped child',
          )
          assert.equal(
            report.fixtureRequests.filter((request) => request.heldLabel === label).length,
            heldStarts,
            'Restart must not send another actual child model call',
          )
          const saved = JSON.parse(await readFile(join(agent, 'pisper-agents.json'), 'utf8'))
          assert.equal(
            saved.records.find((value) => value.id === pendingChild.id).status,
            'interrupted',
          )
          report.activeChildRestart = {
            child: pendingChild.id,
            status: recovered.status,
            automaticRuns: 0,
            shutdownMode: desktopExecutable
              ? report.desktopCleanups.at(-1).mode
              : report.serverCleanups.at(-1).mode,
          }
        }
        if (pendingSchedule) {
          const dashboard = await json('/api/schedules')
          const task = dashboard.tasks.find((entry) => entry.id === pendingSchedule.id)
          const run = dashboard.runs.find((entry) => entry.taskId === pendingSchedule.id)
          assert.equal(task.lastStatus, 'interrupted')
          assert.equal(run.status, 'interrupted')
          assert.equal(task.nextRunAt, null)
          assert.equal(heldModelRequests.has(scheduleLabel), false)
          assert.equal(
            report.fixtureRequests.filter((request) => request.heldLabel === scheduleLabel).length,
            heldScheduleStarts,
            'Restart must not automatically repeat the interrupted scheduled model call',
          )
          const live = await json(`/api/sessions/${pendingScheduleSession}/live`)
          assert.equal(live.streaming, false)
          report.activeScheduleRestart = {
            task: pendingSchedule.id,
            run: run.id,
            session: pendingScheduleSession,
            status: run.status,
            automaticRuns: 0,
            shutdownMode: desktopExecutable
              ? report.desktopCleanups.at(-1).mode
              : report.serverCleanups.at(-1).mode,
          }
          assert.equal((await json(`/api/schedules/${pendingSchedule.id}`, 'DELETE')).deleted, true)
          assert.equal(
            (await json(`/api/sessions/${pendingScheduleSession}`, 'DELETE')).deleted,
            true,
          )
        }
      } finally {
        heldModelRequests.get(label)?.()
        heldModelRequests.get(scheduleLabel)?.()
      }
      if (!legacyProbe) {
        const config = await json('/api/config')
        assertConfig(config)
        assert.equal(config.defaultProvider || config.provider, providerId)
        assert.equal(config.defaultModel || config.model, modelId)
      }
      const listed = await json('/api/sessions')
      assert.ok(listed.sessions.some((session) => session.id === sessionId))
      const data = await json(`/api/sessions/${sessionId}/messages?limit=50`)
      assertMessages(data)
      assert.ok(data.messages.some((message) => message.text.includes('Rust API usability hello')))
      await chat('after restart hello')
    })
    if (!legacyProbe) {
      if (channelsRestart)
        await check(
          'Native channel reconnect preserves peer sessions and sends through the restarted Agent',
          channelsRestart,
        )
      if (browserRestart) await browserRestart()
      if (pluginsRestart)
        await check('Native plugins and worker shutdown survive process restart', pluginsRestart)
      if (notificationRestart) {
        await check(
          'Notifications preserve config, templates and browser events across process restart',
          notificationRestart,
        )
      }
      if (fileChangeRestart) {
        await check(
          'File snapshots survive restart with original bytes and safe revert',
          fileChangeRestart,
        )
      }
      if (customUiRestart)
        await check(
          'Custom UI components persist and scoped view tokens expire at restart',
          customUiRestart,
        )
      if (gameAssetRestart)
        await check(
          'Game jobs, edited frames and atlas bytes survive restart without generation replay',
          gameAssetRestart,
        )
      await checkMcpNativeRoundtrip(
        'MCP native configuration and actual tool survive process restart',
      )
      await check(
        'MCP delete removes config and native tools, preserving unknown root fields',
        async () => {
          await json(`/api/mcp/${mcpId}`, 'DELETE')
          await waitForMcp(
            (result) =>
              !result.services.some((service) => service.id === mcpId) &&
              !result.tools.some((tool) => tool.piName === mcpTool),
          )
          const saved = JSON.parse(await readFile(join(agent, 'mcp.json'), 'utf8'))
          assert.deepEqual(saved.fixtureRoot, { unknown: 'preserve' })
          assert.ok(!saved.mcpServers[mcpId])
          const plugins = await json('/api/plugins')
          assert.ok(!plugins.callableToolNames.includes(mcpTool))
        },
      )
    }
  }
} catch (error) {
  report.failures.push({ name: 'Harness orchestration', error: error.stack || String(error) })
} finally {
  await browserRestart
    ?.dispose?.()
    .catch((error) =>
      report.failures.push({ name: 'Owned browser fixture cleanup', error: String(error) }),
    )
  await channelsRestart
    ?.dispose?.()
    .catch((error) =>
      report.failures.push({ name: 'Owned channel fixture cleanup', error: String(error) }),
    )
  await browser?.close().catch(() => {})
  await stop().catch((error) =>
    report.failures.push({ name: 'Harness server cleanup', error: String(error) }),
  )
  if (fixture?.listening) await new Promise((done) => fixture.close(done))
  report.finished = new Date().toISOString()
  report.status = report.failures.length
    ? 'failed'
    : report.unsupported.length
      ? 'core-passed-with-unsupported-features'
      : 'passed'
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
  await writeFile(join(output, 'server.log'), report.serverLog)
  console.log(
    JSON.stringify(
      {
        status: report.status,
        checks: report.checks.length,
        failures: report.failures.length,
        report: join(output, 'report.json'),
      },
      null,
      2,
    ),
  )
  process.exitCode = report.failures.length ? 1 : 0
}
