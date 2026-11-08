// 历史用户数据不能让 Runtime 起不来。这里跑真实的 sidecar 进程：
// - fixtures/historical-data：真实旧版本（含 Rust 时期）在隔离目录里自己写出来的数据，字段和表结构都是当时的样子；
// - 损坏矩阵：把数据目录里所有 JSON 同时弄坏，新增的数据文件靠源码扫描自动进矩阵，不用登记。
// 重新生成数据：安装旧版本 sidecar，用它的接口写入后导出（路径换成 /home/fixture，没有真实凭据）。
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { cp, mkdir, mkdtemp, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import { DatabaseSync } from 'node:sqlite'

const appRoot = resolve(process.env.PISPER_TEST_APP_ROOT || '.')
const fixtures = join(appRoot, 'runtime', 'tests', 'fixtures', 'historical-data')
const TOKEN = 'startup-resilience-token'

// 按注册的反序清理：Windows 上进程没退、数据库没关时目录删不掉，不能依赖 t.after 的顺序。
const cleanups = new WeakMap()
function defer(t, cleanup) {
  let stack = cleanups.get(t)
  if (!stack) {
    stack = []
    cleanups.set(t, stack)
    t.after(async () => {
      for (const run of stack.reverse()) await run()
    })
  }
  stack.push(cleanup)
}

async function bootSidecar(dataDir, { env = {}, settleMs = 45_000 } = {}) {
  const home = await mkdtemp(join(tmpdir(), 'pisper-startup-home-'))
  const child = spawn(process.execPath, [join(appRoot, 'runtime', 'sidecar.mjs')], {
    cwd: appRoot,
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      PI_OFFLINE: '1',
      PISPER_AGENT_DIR: dataDir,
      PISPER_DESKTOP_TOKEN: TOKEN,
      PISPER_EXIT_ON_STDIN_CLOSE: '1',
      PISPER_STARTUP_TIMING: '1',
      PISPER_WORKSPACE_DIR: home,
      ...env,
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const state = { ready: null, stages: [], exit: undefined, stderr: '' }
  let settle
  const settled = new Promise((resolveSettled) => (settle = resolveSettled))
  let stdout = ''
  child.stdout.on('data', (chunk) => {
    stdout += chunk
    const match = stdout.match(/PISPER_SIDECAR_READY (.*)\n/)
    if (match && !state.ready) state.ready = JSON.parse(match[1])
  })
  let stderr = ''
  child.stderr.on('data', (chunk) => {
    stderr += chunk
    let index
    while ((index = stderr.indexOf('\n')) >= 0) {
      const line = stderr.slice(0, index)
      stderr = stderr.slice(index + 1)
      if (!line.startsWith('PISPER_SIDECAR_STAGE ')) {
        state.stderr += `${line}\n`
        continue
      }
      const { stage } = JSON.parse(line.slice('PISPER_SIDECAR_STAGE '.length))
      state.stages.push(stage)
      if (stage === 'runtime-initialized' || stage === 'failed') settle()
    }
  })
  child.on('exit', (code) => {
    state.exit = code
    settle()
  })
  const timer = setTimeout(settle, settleMs)
  await settled
  clearTimeout(timer)

  let cookie = ''
  let origin = ''
  if (state.ready && state.exit === undefined) {
    // 失败时进程可能正在退出，这里拿不到 Cookie 不算错，后面的断言会说明情况。
    const response = await fetch(state.ready.bootstrapUrl, { redirect: 'manual' }).catch(() => null)
    cookie = response?.headers.get('set-cookie')?.split(';')[0] ?? ''
    origin = new URL(state.ready.url).origin
  }
  return {
    ...state,
    get exit() {
      return state.exit
    },
    get stderr() {
      return state.stderr
    },
    api: (method, path, body) =>
      fetch(`${origin}${path}`, {
        method,
        headers: { cookie, origin, 'content-type': 'application/json' },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      }),
    async json(path) {
      const response = await this.api('GET', path)
      return { status: response.status, data: await response.json().catch(() => null) }
    },
    async close() {
      if (state.exit === undefined) {
        const exited = new Promise((resume) => child.once('exit', resume))
        child.stdin.write('shutdown\n')
        await Promise.race([exited, new Promise((resume) => setTimeout(resume, 8_000))])
        if (state.exit === undefined) child.kill('SIGKILL')
      }
      await rm(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
    },
  }
}

async function copyFixture(t, name) {
  const dir = await mkdtemp(join(tmpdir(), `pisper-fixture-${name}-`))
  defer(t, () => rm(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))
  await cp(join(fixtures, name), dir, { recursive: true })
  const manifest = JSON.parse(await readFile(join(dir, 'fixture-manifest.json'), 'utf8'))
  await rm(join(dir, 'fixture-manifest.json'))
  return { dir, manifest }
}

async function assertAppUsable(sidecar) {
  for (const path of [
    '/api/sessions',
    '/api/config',
    '/api/schedules',
    '/api/workflows',
    '/api/mcp',
  ]) {
    const { status } = await sidecar.json(path)
    assert.equal(status, 200, `${path} should answer after startup`)
  }
}

for (const era of await readdir(fixtures)) {
  test(`data written by ${era} opens without losing anything`, { timeout: 90_000 }, async (t) => {
    const { dir, manifest } = await copyFixture(t, era)
    const sidecar = await bootSidecar(dir)
    defer(t, () => sidecar.close())
    assert.ok(sidecar.stages.includes('runtime-initialized'), sidecar.stderr)
    await assertAppUsable(sidecar)

    const status = (await sidecar.json('/api/startup-status')).data
    assert.equal(status.state, 'ready')
    assert.deepEqual(status.setAside, [], 'healthy old data must not be set aside')
    assert.deepEqual(status.failed, [])

    const sessions = (await sidecar.json('/api/sessions')).data
    const list = sessions.sessions ?? sessions
    assert.ok(list.length >= manifest.sessions, `sessions: ${list.length} < ${manifest.sessions}`)
    assert.ok(
      (await sidecar.json('/api/schedules')).data.tasks?.length >= 1 ||
        (await sidecar.json('/api/schedules')).data.length >= 1,
      'schedule from the old version is still there',
    )

    await sidecar.close()
    const db = new DatabaseSync(join(dir, 'pisper-memory.sqlite'), { readOnly: true })
    defer(t, () => db.close())
    assert.equal(
      db.prepare('SELECT COUNT(*) AS n FROM memories').get().n,
      manifest.memories + manifest.candidates,
      'every old memory survives and pending candidates are promoted',
    )
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_candidates').get().n, 0)
    assert.ok(await stat(join(dir, 'pisper-memory.sqlite.pre-v5.bak')), 'upgrade keeps a backup')
  })
}

// 数据目录里出现过的 JSON 文件名：扫源码，新加的数据文件自动进矩阵。
async function dataFileNames() {
  const names = new Set()
  const walk = async (dir) => {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      if (entry.name === 'tests' || entry.name === 'node_modules') continue
      const path = join(dir, entry.name)
      if (entry.isDirectory()) await walk(path)
      else if (entry.name.endsWith('.mjs')) {
        const source = await readFile(path, 'utf8')
        for (const match of source.matchAll(/join\([^()]*?'([\w.-]+\.json)'\s*\)/g))
          names.add(match[1])
      }
    }
  }
  await walk(join(appRoot, 'runtime'))
  return [...names].filter(
    (name) => !['meta.json', 'manifest.json', 'index.json', 'package.json'].includes(name),
  )
}

const DAMAGE = {
  截断: '{"truncated": [1, 2',
  空文件: '',
  null: 'null',
  数字: '1',
  字符串: '"x"',
}

test('the file scan still finds the data files', async () => {
  const names = await dataFileNames()
  for (const name of [
    'pisper-schedules.json',
    'pisper-usage.json',
    'pisper-bots.json',
    'auth.json',
  ])
    assert.ok(names.includes(name), `${name} missing from ${names.join(', ')}`)
})

for (const [label, content] of Object.entries(DAMAGE)) {
  test(
    `every data file damaged at once (${label}) still starts`,
    { timeout: 90_000 },
    async (t) => {
      const { dir } = await copyFixture(t, 'v0.6.23')
      const present = (await readdir(dir)).filter((name) => name.endsWith('.json'))
      const names = [...new Set([...present, ...(await dataFileNames())])]
      for (const name of names) await writeFile(join(dir, name), content)
      const sidecar = await bootSidecar(dir)
      defer(t, () => sidecar.close())
      assert.ok(sidecar.stages.includes('runtime-initialized'), sidecar.stderr)
      await assertAppUsable(sidecar)

      const status = (await sidecar.json('/api/startup-status')).data
      assert.equal(status.state, 'ready')
      assert.deepEqual(status.failed, [])
      const moved = new Set(status.setAside.map((item) => item.file))
      for (const name of names) assert.ok(moved.has(name), `${name} should be set aside`)
      // 原内容留着，能找回。
      const kept = (await readdir(dir)).filter((name) =>
        name.startsWith('pisper-usage.json.corrupt-'),
      )
      assert.equal(kept.length, 1)
      assert.equal(await readFile(join(dir, kept[0]), 'utf8'), content)
    },
  )
}

test('a file saved with a BOM keeps its content', { timeout: 60_000 }, async (t) => {
  const { dir } = await copyFixture(t, 'v0.6.23')
  const path = join(dir, 'pisper-schedules.json')
  const original = await readFile(path, 'utf8')
  await writeFile(path, `﻿${original}`)
  const sidecar = await bootSidecar(dir)
  defer(t, () => sidecar.close())
  const status = (await sidecar.json('/api/startup-status')).data
  assert.deepEqual(status.setAside, [])
  assert.equal(await readFile(path, 'utf8').then((text) => text.charCodeAt(0) === 0xfeff), false)
})

test(
  'a file a service rejects is set aside and the service starts clean',
  { timeout: 60_000 },
  async (t) => {
    const { dir } = await copyFixture(t, 'v0.6.23')
    await writeFile(
      join(dir, 'pisper-schedules.json'),
      JSON.stringify({ version: 1, tasks: [null], runs: [null] }),
    )
    await writeFile(join(dir, 'models.json'), JSON.stringify({ providers: { gone: null } }))
    const sidecar = await bootSidecar(dir)
    defer(t, () => sidecar.close())
    assert.ok(sidecar.stages.includes('runtime-initialized'), sidecar.stderr)
    await assertAppUsable(sidecar)
    const status = (await sidecar.json('/api/startup-status')).data
    const moved = status.setAside.map((item) => item.file)
    assert.ok(moved.includes('pisper-schedules.json'))
    assert.ok(moved.includes('models.json'))
    assert.deepEqual(status.failed, [])
    // 新建定时任务能落盘，不会盖掉留底的旧文件。
    assert.ok(
      (await readdir(dir)).some((name) => name.startsWith('pisper-schedules.json.corrupt-')),
    )
  },
)

for (const [label, damage] of Object.entries({
  垃圾内容: () => Buffer.from('not a database at all '.repeat(400)),
  截断一半: (bytes) => bytes.subarray(0, Math.floor(bytes.length / 2)),
})) {
  test(
    `a corrupt memory database (${label}) is set aside and memory keeps working`,
    { timeout: 60_000 },
    async (t) => {
      const { dir } = await copyFixture(t, 'v0.6.23')
      const path = join(dir, 'pisper-memory.sqlite')
      const bytes = damage(await readFile(path))
      await writeFile(path, bytes)
      const sidecar = await bootSidecar(dir)
      defer(t, () => sidecar.close())
      assert.ok(sidecar.stages.includes('runtime-initialized'), sidecar.stderr)
      const status = (await sidecar.json('/api/startup-status')).data
      assert.deepEqual(status.failed, [])
      assert.ok(status.setAside.some((item) => item.file === 'pisper-memory.sqlite'))
      const kept = (await readdir(dir)).find((name) =>
        /^pisper-memory\.sqlite\.corrupt-\d+$/.test(name),
      )
      assert.deepEqual(await readFile(join(dir, kept)), bytes)
      const created = await sidecar.api('POST', '/api/memory/nodes', {
        title: '新记忆',
        content: '新库能写',
        type: 'preference',
        spaceId: 'global',
      })
      assert.ok(created.ok, await created.text())
    },
  )
}

test(
  'a memory database from a newer version is left alone and only memory is off',
  { timeout: 60_000 },
  async (t) => {
    const { dir } = await copyFixture(t, 'v0.6.23')
    const path = join(dir, 'pisper-memory.sqlite')
    const db = new DatabaseSync(path)
    db.exec('PRAGMA user_version = 99')
    const memoriesBefore = db.prepare('SELECT COUNT(*) AS n FROM memories').get().n
    db.close()
    const sidecar = await bootSidecar(dir)
    defer(t, () => sidecar.close())
    assert.ok(sidecar.stages.includes('runtime-initialized'), sidecar.stderr)
    await assertAppUsable(sidecar)
    const status = (await sidecar.json('/api/startup-status')).data
    assert.equal(status.state, 'ready')
    assert.deepEqual(
      status.failed.map((item) => item.service),
      ['memory'],
    )
    assert.deepEqual(status.setAside, [], 'never touch data a newer version wrote')
    const memory = await sidecar.api('GET', '/api/memory')
    assert.ok(memory.status >= 400)
    assert.match(await memory.text(), /newer than this app/)
    await sidecar.close()
    const after = new DatabaseSync(path, { readOnly: true })
    defer(t, () => after.close())
    assert.equal(after.prepare('PRAGMA user_version').get().user_version, 99)
    assert.equal(after.prepare('SELECT COUNT(*) AS n FROM memories').get().n, memoriesBefore)
  },
)

test(
  'a file sitting where the pet directory goes does not stop READY',
  { timeout: 60_000 },
  async (t) => {
    const { dir } = await copyFixture(t, 'v0.6.23')
    await writeFile(join(dir, 'desktop-pets'), 'x')
    const sidecar = await bootSidecar(dir)
    defer(t, () => sidecar.close())
    assert.ok(sidecar.ready, sidecar.stderr)
    assert.ok(sidecar.stages.includes('runtime-initialized'), sidecar.stderr)
  },
)

for (const name of [
  'pisper-memory-models/locked',
  'sprite-engines',
  'workflow-media',
  'plugins',
  'skills',
]) {
  test(
    `a blocked ${name} only costs that feature`,
    { timeout: 60_000, skip: process.platform === 'win32' },
    async (t) => {
      const { dir } = await copyFixture(t, 'v0.6.23')
      const path = join(dir, name)
      await mkdir(join(path, '..'), { recursive: true })
      await writeFile(path, 'x')
      const sidecar = await bootSidecar(dir)
      defer(t, () => sidecar.close())
      assert.ok(sidecar.stages.includes('runtime-initialized'), sidecar.stderr)
      const { status } = await sidecar.json('/api/sessions')
      assert.equal(status, 200)
    },
  )
}

test(
  'when the runtime cannot start the desktop page gets the reason, without personal paths',
  { timeout: 60_000 },
  async (t) => {
    const parent = await mkdtemp(join(tmpdir(), 'pisper-blocked-'))
    defer(t, () => rm(parent, { recursive: true, force: true }))
    // 数据目录被文件占了：会话目录建不出来，这是没法跳过的那一类。
    const blocked = join(parent, 'agent')
    await writeFile(blocked, 'x')
    const sidecar = await bootSidecar(blocked, {
      env: { PISPER_IROH_STATUS_FILE: join(parent, 'iroh.json') },
    })
    defer(t, () => sidecar.close())
    assert.ok(sidecar.ready, 'READY still goes out so the window can open')
    assert.equal(sidecar.exit, undefined, 'the desktop sidecar stays up to explain')
    const { status, data } = await sidecar.json('/api/startup-status')
    assert.equal(status, 200)
    assert.equal(data.state, 'failed')
    assert.ok(data.failure.code)
    assert.ok(!data.failure.message.includes(parent), data.failure.message)
    assert.equal((await sidecar.json('/api/sessions')).status, 503)
  },
)

test('the TUI still sees a failed runtime as an exit', { timeout: 60_000 }, async (t) => {
  const parent = await mkdtemp(join(tmpdir(), 'pisper-blocked-tui-'))
  defer(t, () => rm(parent, { recursive: true, force: true }))
  const blocked = join(parent, 'agent')
  await writeFile(blocked, 'x')
  const sidecar = await bootSidecar(blocked, { env: { PISPER_IROH_STATUS_FILE: '' } })
  defer(t, () => sidecar.close())
  for (let index = 0; index < 50 && sidecar.exit === undefined; index += 1)
    await new Promise((resume) => setTimeout(resume, 100))
  assert.equal(sidecar.exit, 1)
})

test('only the desktop shell asks the sidecar to stay up after a failed start', async () => {
  const desktop = await readFile(join(appRoot, 'src-tauri/src/desktop_shell/mod.rs'), 'utf8')
  const tui = await readFile(join(appRoot, 'src-tui/src/sidecar.rs'), 'utf8')
  assert.match(desktop, /"PISPER_IROH_STATUS_FILE"/)
  assert.doesNotMatch(tui, /PISPER_IROH_STATUS_FILE/)
})
