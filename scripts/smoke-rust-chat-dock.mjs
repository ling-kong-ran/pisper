// 用隔离的 Rust 进程验证旧版 Dock 布局契约；不读取个人配置或调用外部模型。
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { verifyProductionChatDock } from './smoke-chat-dock-production.mjs'

const root = resolve(import.meta.dirname, '..')
const executable = resolve(
  process.argv[2] ||
    join(
      root,
      'runtime-rs/target/debug',
      process.platform === 'win32' ? 'pisper-server.exe' : 'pisper-server',
    ),
)
const sandbox = await mkdtemp(join(await realpath(tmpdir()), 'pisper-chat-dock-'))
const agent = join(sandbox, 'agent')
const product = join(sandbox, 'product')
const workspace = join(sandbox, 'workspace')
const layoutName = 'pisper-chat-dock-layout.json'
const layoutPath = join(agent, layoutName)
const endpoint = '/api/settings/chat-dock-layout'
const token = 'synthetic-chat-dock-token'
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
    `Dock fixture startup failed: ${redact(failure?.message || diagnostics)}`,
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
    assert.equal(forced, false, 'Dock fixture must shut down without being killed')
    assert.deepEqual(result, { code: 0, signal: null }, 'Dock fixture must exit cleanly')
  } finally {
    clearTimeout(timer)
    child = undefined
    base = undefined
  }
}

async function api({ method = 'GET', body, status = 200 } = {}) {
  const response = await fetch(base + endpoint, {
    method,
    headers: {
      Origin: base,
      Cookie: `__pisper_desktop=${token}`,
      'Content-Type': 'application/json',
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(20000),
  })
  assert.equal(response.status, status, `${method} ${endpoint}: status`)
  return response.json()
}

// 保留真实 v1 envelope、两个分组和非 ASCII 文案，后端不得删字段或改布局形状。
const legacyLayout = {
  version: 1,
  engine: 'dockview',
  activePanelId: 'session:alpha',
  layout: {
    grid: {
      root: {
        type: 'branch',
        data: ['alpha', 'beta'].map((id) => ({
          type: 'leaf',
          data: { id: `group-${id}`, views: [`session:${id}`], activeView: `session:${id}` },
          size: 600,
        })),
        size: 800,
      },
      width: 1200,
      height: 800,
      orientation: 'HORIZONTAL',
    },
    panels: Object.fromEntries(
      ['alpha', 'beta'].map((id) => [
        `session:${id}`,
        {
          id: `session:${id}`,
          contentComponent: 'session',
          title: `并行会话 ${id} 🧪`,
          renderer: 'always',
          params: { sessionId: id },
        },
      ]),
    ),
    activeGroup: 'group-alpha',
  },
  future: { preserve: true },
}

try {
  await Promise.all([agent, product, workspace].map((path) => mkdir(path)))
  const original = JSON.stringify(legacyLayout, null, 2) + '\n'
  const productSentinel = JSON.stringify({ unrelatedProductLayout: true })
  await writeFile(layoutPath, original)
  await writeFile(join(product, layoutName), productSentinel)
  await start()

  assert.deepEqual(await api(), legacyLayout)
  assert.equal(await readFile(layoutPath, 'utf8'), original)
  report.checks.push('legacy v1 layout is read from the original agent path without rewriting')

  const updated = structuredClone(legacyLayout)
  updated.activePanelId = 'session:beta'
  updated.layout.activeGroup = 'group-beta'
  updated.layout.grid.root.data[0].size = 450
  updated.layout.grid.root.data[1].size = 750
  assert.deepEqual(await api({ method: 'PUT', body: updated }), updated)
  assert.deepEqual(JSON.parse(await readFile(layoutPath, 'utf8')), updated)
  assert.deepEqual(await api(), updated)
  assert.equal(await readFile(join(product, layoutName), 'utf8'), productSentinel)
  report.checks.push('PUT returns and persists the complete layout in agent_dir, not product data')

  const beforeInvalid = await readFile(layoutPath, 'utf8')
  for (const body of [null, [], ['invalid'], 'invalid', 42, true]) {
    const error = await api({ method: 'PUT', body, status: 400 })
    assert.equal(error.code, 'bad_request')
    assert.equal(typeof error.error, 'string')
    assert.equal(await readFile(layoutPath, 'utf8'), beforeInvalid)
    assert.deepEqual(await api(), updated)
  }
  report.checks.push('invalid PUT values return the stable error and cannot overwrite saved layout')

  // 旧端点允许任意对象，不能为了恢复界面而收紧为仅接受 Dockview envelope。
  const generic = { arbitraryObject: true, future: { data: ['中文', 7] } }
  assert.deepEqual(await api({ method: 'PUT', body: generic }), generic)
  assert.deepEqual(await api(), generic)
  report.checks.push('legacy arbitrary-object PUT contract remains compatible')

  const snapshots = Array.from({ length: 12 }, (_, revision) => ({ ...updated, revision }))
  for (const snapshot of snapshots) {
    assert.deepEqual(await api({ method: 'PUT', body: snapshot }), snapshot)
    assert.deepEqual(JSON.parse(await readFile(layoutPath, 'utf8')), snapshot)
  }
  report.checks.push('consecutive saves always leave complete valid JSON with the latest snapshot')

  // 并发结果的完成顺序不作假定；每次磁盘观察都必须是某个完整快照。
  const results = await Promise.allSettled(
    snapshots.map(async (snapshot) => {
      assert.deepEqual(await api({ method: 'PUT', body: snapshot }), snapshot)
      const stored = JSON.parse(await readFile(layoutPath, 'utf8'))
      assert.deepEqual(stored, snapshots[stored.revision])
    }),
  )
  for (const result of results) if (result.status === 'rejected') throw result.reason
  const concurrentSaved = JSON.parse(await readFile(layoutPath, 'utf8'))
  assert.deepEqual(concurrentSaved, snapshots[concurrentSaved.revision])
  assert.deepEqual(await api(), concurrentSaved)
  report.checks.push('concurrent saves and disk reads never expose partial or mixed JSON')

  await stop()
  await start()
  assert.deepEqual(await api(), concurrentSaved)
  assert.deepEqual(JSON.parse(await readFile(layoutPath, 'utf8')), concurrentSaved)
  assert.equal(await readFile(join(product, layoutName), 'utf8'), productSentinel)
  report.checks.push('a fresh Rust process restores the last saved layout from the original path')

  await rm(layoutPath)
  assert.equal(await api(), null)
  await assert.rejects(readFile(layoutPath), { code: 'ENOENT' })
  report.checks.push('an absent layout returns null without inventing or migrating a file')

  await verifyProductionChatDock({ base, token, workspace })
  report.checks.push(
    'production ChatPage splits independent drafts, restores after reload, opens history in a split, and keeps mobile Dockview-free',
  )
  report.checks.push(
    'group + creates tabs without waiting for catalog refresh, inherits its cwd, and handles concurrent responses and a closed target group',
  )

  await stop()
  console.log(JSON.stringify({ status: 'passed', ...report }, null, 2))
} finally {
  await stop()
  await rm(sandbox, { recursive: true })
}
