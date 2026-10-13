// 隔离验证网页终端的真实 PTY 生命周期，不读取个人配置。
import assert from 'node:assert/strict'
import { randomUUID, randomBytes } from 'node:crypto'
import { request } from 'node:http'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, realpath, rm, writeFile, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { chromium } from 'playwright-core'

const root = resolve(import.meta.dirname, '..')
const executable = resolve(
  process.argv[2] ||
    join(
      root,
      'runtime-rs/target/debug',
      process.platform === 'win32' ? 'pisper-server.exe' : 'pisper-server',
    ),
)
const sandbox = await mkdtemp(join(await realpath(tmpdir()), 'pisper-web-terminal-'))
const agent = join(sandbox, 'agent')
const product = join(sandbox, 'product')
const workspace = join(sandbox, 'workspace')
const token = 'synthetic-web-terminal-token'
const report = { checks: [] }
let child, ended, base

function redact(text) {
  return String(text)
    .replaceAll(token, '<token>')
    .replaceAll(sandbox, '<fixture>')
    .replaceAll(root, '<repository>')
}

async function start() {
  base = undefined
  let pending = '',
    diagnostics = '',
    failure
  child = spawn(executable, [], {
    cwd: workspace,
    env: {
      ...Object.fromEntries(
        Object.entries(process.env).filter(([key]) =>
          /^(PATH|PATHEXT|SYSTEMROOT|WINDIR|TEMP|TMP|TMPDIR)$/i.test(key),
        ),
      ),
      HOME: sandbox,
      USERPROFILE: sandbox,
      PISPER_AGENT_DIR: agent,
      PI_CODING_AGENT_DIR: agent,
      PISPER_RS_DATA_DIR: product,
      PISPER_WORKSPACE_DIR: workspace,
      PISPER_APP_ROOT: root,
      PISPER_DESKTOP_TOKEN: token,
      PISPER_EXIT_ON_STDIN_CLOSE: '1',
      PI_SKIP_VERSION_CHECK: '1',
      PI_TELEMETRY: '0',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
  })
  ended = new Promise((resolveExit) =>
    child.once('close', (code, signal) => resolveExit({ code, signal })),
  )
  child.on('error', (error) => {
    failure = error
  })
  child.stdin.on('error', (error) => {
    failure = error
  })
  child.stderr.on('data', (chunk) => {
    diagnostics = (diagnostics + chunk).slice(-4000)
  })
  child.stdout.on('data', (chunk) => {
    pending += chunk
    const lines = pending.split('\n')
    pending = lines.pop()
    for (const line of lines) {
      if (!line.startsWith('PISPER_SIDECAR_READY ')) continue
      try {
        base = JSON.parse(line.slice('PISPER_SIDECAR_READY '.length)).url
      } catch (error) {
        failure = error
      }
    }
  })
  const deadline = Date.now() + 30000
  while (!base && !failure && child.exitCode === null && Date.now() < deadline) await delay(50)
  assert.ok(
    base && !failure,
    `Terminal fixture startup failed: ${redact(failure?.message || diagnostics)}`,
  )
}

async function stop() {
  if (!child) return
  let forced = false
  const timer = setTimeout(() => {
    forced = true
    child.kill('SIGKILL')
  }, 15000)
  try {
    if (child.pid && child.exitCode === null && child.signalCode === null)
      child.stdin.end('shutdown\n')
    const result = await ended
    assert.equal(forced, false, 'Terminal fixture must shut down without being killed')
    assert.deepEqual(result, { code: 0, signal: null }, 'Terminal fixture must exit cleanly')
  } finally {
    clearTimeout(timer)
    child = undefined
    base = undefined
  }
}

async function api(path, body) {
  const response = await fetch(base + path, {
    method: body ? 'POST' : 'GET',
    headers: {
      Origin: base,
      Cookie: `__pisper_desktop=${token}`,
      'Content-Type': 'application/json',
    },
    body: body ? JSON.stringify(body) : undefined,
  })
  assert.ok(response.ok, `${path}: ${response.status}`)
  return response.json()
}

async function rejected(origin, cookie, status) {
  await new Promise((resolve, reject) => {
    const upgrade = request(base + '/api/terminal/connect', {
      headers: {
        Origin: origin,
        Connection: 'Upgrade',
        Upgrade: 'websocket',
        'Sec-WebSocket-Version': '13',
        'Sec-WebSocket-Key': randomBytes(16).toString('base64'),
        ...(cookie ? { Cookie: `__pisper_desktop=${token}` } : {}),
      },
    })
    upgrade.setTimeout(10000, () => upgrade.destroy(new Error('Handshake timed out')))
    upgrade.once('upgrade', (_response, socket) => {
      socket.destroy()
      reject(new Error('Unsafe terminal handshake accepted'))
    })
    upgrade.once('response', (response) => {
      response.resume()
      try {
        assert.equal(response.statusCode, status)
        resolve()
      } catch (error) {
        reject(error)
      }
    })
    upgrade.once('error', reject)
    upgrade.end()
  })
}

let browser
try {
  await Promise.all([agent, product, workspace].map((path) => mkdir(path)))
  await writeFile(join(agent, 'models.json'), JSON.stringify({ providers: {} }))
  await start()
  const profiles = await api('/api/terminal/profiles')
  assert.ok(profiles.length > 0)
  await rejected('http://untrusted.invalid', true, 403)
  await rejected(base, false, 401)
  report.checks.push('cross-origin and unauthenticated WebSocket upgrades rejected')
  const session = await api('/api/sessions', { name: 'Web Terminal Fixture', cwd: workspace })
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_SMOKE_BROWSER
      ? { executablePath: process.env.PISPER_SMOKE_BROWSER }
      : {}),
  })
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } })
  await context.addCookies([{ name: '__pisper_desktop', value: token, url: base }])
  await context.addInitScript((id) => {
    localStorage.setItem('pisper-language', 'zh-CN')
    localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
    localStorage.setItem('pisper-active-session', id)
  }, session.id)
  const page = await context.newPage()
  const errors = []
  const sizes = []
  page.on('websocket', (socket) => {
    if (!socket.url().endsWith('/api/terminal/connect')) return
    socket.on('framesent', ({ payload }) => {
      const frame = JSON.parse(String(payload))
      if (frame.type === 'create' || frame.type === 'resize') sizes.push(frame.cols)
    })
  })
  page.on('pageerror', (error) => errors.push(error.message))
  await page.goto(base + '/#/chat')
  const toggle = page.getByRole('button', { name: '切换终端', exact: true })
  await toggle.click()
  const panel = page.locator('.terminal-panel.is-open')
  await panel.waitFor()
  await panel.locator('.terminal-empty').click()
  const input = panel.locator('.terminal-host.active .xterm-helper-textarea')
  try {
    await input.waitFor({ state: 'attached' })
  } catch (error) {
    console.error(redact(await panel.innerText()), errors)
    await page.screenshot({ path: join(root, 'release/layout-dev/web-terminal-failure.png') })
    throw error
  }
  const output = () => panel.locator('.terminal-host.active .xterm-rows')
  await page.waitForFunction(
    () =>
      document.querySelector('.terminal-tab.active i')?.getAttribute('data-status') === 'running',
  )
  const marker = 'WEB_PTY_' + randomUUID().replaceAll('-', '')
  // 拆开标记，避免把命令回显误认成 Shell 的执行结果。
  const windows = process.platform === 'win32'
  const command = windows
    ? `Write-Output ('${marker.slice(0, 12)}' + '${marker.slice(12)}'); Write-Output ((Get-Location).Path)`
    : `printf '%s%s\\n' '${marker.slice(0, 12)}' '${marker.slice(12)}'; pwd`
  await input.fill(command)
  await input.press('Enter')
  await page.waitForFunction(
    (value) =>
      document.querySelector('.terminal-host.active .xterm-rows')?.textContent.includes(value),
    marker,
  )
  assert.ok((await output().textContent()).includes(workspace))
  report.checks.push(
    'production xterm uses real WebSocket and native PTY: input, output, active session cwd',
  )
  const previousCols = sizes.at(-1)
  assert.ok(Number.isInteger(previousCols))
  await page.setViewportSize({ width: 1040, height: 760 })
  const resizedDeadline = Date.now() + 10000
  while (sizes.at(-1) === previousCols && Date.now() < resizedDeadline) await delay(20)
  const resizedCols = sizes.at(-1)
  assert.ok(resizedCols < previousCols, 'Viewport resize must reach the PTY transport')
  await input.fill(
    windows
      ? "Write-Output ('COLS:' + [Console]::WindowWidth)"
      : "printf 'COLS:%s\\n' \"$(stty size | cut -d' ' -f2)\"",
  )
  await input.press('Enter')
  await page.waitForFunction(
    (value) =>
      document.querySelector('.terminal-host.active .xterm-rows')?.textContent.includes(value),
    `COLS:${resizedCols}`,
  )
  await page.screenshot({ path: join(root, 'release/layout-dev/web-terminal.png') })
  await input.fill('exit 7')
  await input.press('Enter')
  await page.waitForFunction(
    () =>
      document.querySelector('.terminal-tab.active i')?.getAttribute('data-status') === 'exited',
  )
  assert.ok((await output().textContent()).includes('7'))
  await panel.getByRole('button', { name: '新建终端', exact: true }).click()
  if (profiles.length > 1)
    await panel
      .getByRole('button', {
        name: profiles.find((profile) => profile.default)?.label || profiles[0].label,
        exact: true,
      })
      .click()
  await page.waitForFunction(
    () =>
      document.querySelector('.terminal-tab.active i')?.getAttribute('data-status') === 'running',
  )
  // 记录隔离 Shell 的 PID，验证关页后操作系统确实回收了进程。
  const pidFile = join(workspace, 'shell.pid')
  await input.fill(
    windows
      ? `[System.IO.File]::WriteAllText('${pidFile}', [string]$PID)`
      : `echo $$ > '${pidFile}'`,
  )
  await input.press('Enter')
  let pid
  const deadline = Date.now() + 10000
  while (Date.now() < deadline) {
    try {
      pid = Number(await readFile(pidFile, 'utf8'))
      if (pid > 0) break
    } catch (error) {
      if (error.code !== 'ENOENT') throw error
    }
    await delay(30)
  }
  assert.ok(pid > 0)
  await page.close()
  const cleanupDeadline = Date.now() + 10000
  while (Date.now() < cleanupDeadline) {
    try {
      process.kill(pid, 0)
    } catch (error) {
      if (error.code === 'ESRCH') {
        pid = 0
        break
      }
      throw error
    }
    await delay(30)
  }
  assert.equal(pid, 0, 'Closing the browser must reap its shell')
  assert.deepEqual(errors, [])
  report.checks.push('resize and exit are delivered; closing the browser reaps its real shell')
  await browser.close()
  browser = undefined
  await stop()
  console.log(JSON.stringify({ status: 'passed', ...report }, null, 2))
} finally {
  await browser?.close()
  await stop()
  assert.equal(await realpath(sandbox), sandbox, 'Refuse to remove a redirected fixture root')
  await rm(sandbox, { recursive: true })
}
