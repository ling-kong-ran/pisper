// 使用真实进程覆盖全新目录、旧版文件原地读取和重启；不读取开发者数据。
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdtemp, mkdir, readFile, writeFile, readdir, realpath, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'

const root = resolve(import.meta.dirname, '..')
const executable = resolve(
  process.argv[2] ||
    join(
      root,
      'runtime-rs/target/debug',
      process.platform === 'win32' ? 'pisper-server.exe' : 'pisper-server',
    ),
)
const sandbox = await mkdtemp(join(await realpath(tmpdir()), 'pisper-data-paths-'))
const home = join(sandbox, 'home')
const agent = join(home, '.pisper/agent')
const frontend = join(sandbox, 'frontend')
const decoy = join(sandbox, 'unused-data')
let child, ended, base, cookie

async function start(override) {
  let handshake,
    failure,
    output = '',
    diagnostics = ''
  child = spawn(executable, [], {
    cwd: sandbox,
    windowsHide: true,
    env: {
      ...Object.fromEntries(
        Object.entries(process.env).filter(([key]) =>
          /^(PATH|PATHEXT|SYSTEMROOT|WINDIR|TEMP|TMP|TMPDIR)$/i.test(key),
        ),
      ),
      HOME: home,
      USERPROFILE: home,
      ...(override ? { PISPER_AGENT_DIR: override } : {}),
      // 这两个变量不能改变 Pisper 数据源，也不能产生第二份配置。
      PI_CODING_AGENT_DIR: decoy,
      PISPER_RS_DATA_DIR: decoy,
      PISPER_APP_ROOT: root,
      PISPER_FRONTEND_ROOT: frontend,
      PISPER_PARENT_PID: String(process.pid),
      PISPER_EXIT_ON_STDIN_CLOSE: '1',
      PI_SKIP_VERSION_CHECK: '1',
      PI_TELEMETRY: '0',
      PI_OFFLINE: '1',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  ended = new Promise((resolveExit) => child.once('close', (code) => resolveExit(code)))
  child.on('error', (error) => {
    failure = error
  })
  child.stdin.on('error', (error) => {
    failure = error
  })
  child.stderr.on('data', (chunk) => {
    diagnostics = (diagnostics + chunk).slice(-2000)
  })
  child.stdout.on('data', (chunk) => {
    output += chunk
    const lines = output.split('\n')
    output = lines.pop()
    for (const line of lines) {
      if (line.startsWith('PISPER_SIDECAR_READY ')) {
        try {
          handshake = JSON.parse(line.slice('PISPER_SIDECAR_READY '.length))
        } catch (error) {
          failure = error
        }
      }
    }
  })
  const deadline = Date.now() + 45000
  while (!handshake && !failure && child.exitCode === null && Date.now() < deadline) await delay(50)
  assert.ok(
    handshake && !failure,
    `Runtime startup failed: ${diagnostics.replaceAll(sandbox, '<fixture>').replaceAll(root, '<repository>')}`,
  )
  base = handshake.url
  const response = await fetch(handshake.bootstrapUrl, {
    redirect: 'manual',
    signal: AbortSignal.timeout(10000),
  })
  assert.equal(response.status, 302)
  cookie = response.headers.get('set-cookie')?.split(';')[0]
  assert.ok(cookie)
  assert.equal((await json('/api/health')).ok, true)
  const page = await fetch(base, { headers: { Cookie: cookie } })
  assert.ok((await page.text()).includes('startup fixture'))
}

async function stop() {
  if (!child) return
  if (child.exitCode === null) child.stdin.end('shutdown\n')
  const timer = setTimeout(() => child.kill('SIGKILL'), 15000)
  try {
    assert.equal(await ended, 0, 'Runtime must shut down cleanly')
  } finally {
    clearTimeout(timer)
    child = undefined
  }
}

async function json(path, method = 'GET', body) {
  const response = await fetch(base + path, {
    method,
    headers: { Origin: base, Cookie: cookie, 'Content-Type': 'application/json' },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(15000),
  })
  assert.equal(response.ok, true, `${method} ${path}: ${response.status}`)
  return response.json()
}

try {
  await mkdir(home)
  await mkdir(frontend)
  await writeFile(join(frontend, 'index.html'), '<!doctype html><title>startup fixture</title>')
  await start()
  const listing = await json('/api/sessions')
  const sessions = listing.sessions || listing
  const session = sessions[0]
  assert.ok(session?.id)
  assert.equal(resolve(session.cwd), home, 'Default workspace remains the user home')
  await stop()
  assert.ok((await readdir(join(agent, 'sessions'))).some((name) => name.endsWith('.jsonl')))
  const sessionFile = (await readdir(join(agent, 'sessions'))).find((name) =>
    name.endsWith('.jsonl'),
  )
  const sessionBytes = await readFile(join(agent, 'sessions', sessionFile))
  const oldWorkspace = join(sandbox, 'original-project')
  await mkdir(oldWorkspace)

  const metadata = {
    [session.id]: {
      name: '旧版会话',
      cwd: oldWorkspace,
      manual: true,
      futureField: { keep: true },
      organization: {
        pinned: true,
        archived: false,
        unread: true,
        failed: true,
        lastCompletedAt: '2026-01-01T00:00:00.000Z',
        futureFlag: 42,
      },
    },
  }
  const remote = {
    version: 1,
    enabled: true,
    pairingCode: null,
    devices: [
      {
        id: 'old-phone',
        name: '旧手机',
        tokenHash: 'fixture-hash',
        createdAt: '2026-01-01T00:00:00.000Z',
        lastSeenAt: '2026-01-01T00:00:00.000Z',
        revokedAt: null,
      },
    ],
    futureField: true,
  }
  await writeFile(join(agent, 'pisper-sessions.json'), JSON.stringify(metadata))
  await writeFile(join(agent, 'remote-access.json'), JSON.stringify(remote))
  await start()
  const restored = await json('/api/sessions')
  const old = (restored.sessions || restored).find((item) => item.id === session.id)
  assert.equal(old.name, '旧版会话')
  assert.equal(resolve(old.cwd), oldWorkspace)
  const changes = await json(`/api/sessions/${session.id}/file-changes`)
  assert.equal(resolve(changes.cwd), oldWorkspace)
  assert.equal(old.pinned, true)
  assert.equal(old.unread, true)
  assert.equal(old.needsAttention, true)
  assert.equal((await json('/api/remote/status')).enabled, true)
  assert.equal((await json('/api/remote/devices')).devices[0].id, 'old-phone')
  await json(`/api/sessions/${session.id}/organization`, 'PATCH', { read: true })
  await json('/api/remote/enabled', 'PUT', { enabled: false })
  await stop()
  assert.deepEqual(await readFile(join(agent, 'sessions', sessionFile)), sessionBytes)
  const saved = JSON.parse(await readFile(join(agent, 'pisper-sessions.json'), 'utf8'))
  assert.deepEqual(saved[session.id].futureField, metadata[session.id].futureField)
  assert.equal(saved[session.id].organization.futureFlag, 42)
  assert.equal(saved[session.id].organization.unread, false)
  const savedRemote = JSON.parse(await readFile(join(agent, 'remote-access.json'), 'utf8'))
  assert.equal(savedRemote.enabled, false)
  assert.equal(savedRemote.futureField, true)
  assert.deepEqual(savedRemote.devices, remote.devices)
  for (const wrong of [
    'session-meta.json',
    'devices.json',
    'pairing-requests.json',
    'extensions.json',
    'schedules.json',
    'workflows.json',
    'plugins.json',
    'assets.json',
    'memory.json',
  ]) {
    assert.ok(!(await readdir(agent)).includes(wrong), `Must not create ${wrong}`)
  }
  assert.ok(!(await readdir(sandbox)).includes('unused-data'))
  const custom = join(sandbox, 'custom-agent')
  await start(custom)
  await stop()
  assert.ok((await readdir(join(custom, 'sessions'))).some((name) => name.endsWith('.jsonl')))
  assert.deepEqual(JSON.parse(await readFile(join(agent, 'pisper-sessions.json'), 'utf8')), saved)
  console.log(
    'Rust data paths: fresh startup, original metadata, pairing settings, restart and explicit override passed',
  )
} finally {
  try {
    await stop()
  } finally {
    await rm(sandbox, { recursive: true })
  }
}
