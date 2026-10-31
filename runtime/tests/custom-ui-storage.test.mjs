// 组件存储（配额、key 规则、能力门控、原子串行写、严格 JSON、删除进回收站）、错误环形缓冲
// （脱敏、主目录缩写、限流与 dropped、容量）与绑定会话（复用 / 归档后新建）。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { authorizeDesktopRequest } from '../desktop-sidecar-auth.mjs'
import { createApiHandler } from '../http/api-handler.mjs'
import { ComponentErrorLog } from '../services/custom-ui-errors.mjs'
import { CustomUiService } from '../services/custom-ui-service.mjs'

async function fixture(t, options = {}) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-storage-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const service = new CustomUiService({ dataDir, ...options })
  await service.start({ hostVersion: '0.6.5' })
  t.after(() => service.dispose())
  const write = async (id, manifest) => {
    await mkdir(join(service.root, id), { recursive: true })
    await writeFile(join(service.root, id, 'manifest.json'), JSON.stringify(manifest))
    await writeFile(join(service.root, id, 'component.js'), 'export default () => null\n')
  }
  return { dataDir, service, write }
}

const rejectsCode = (promise, code, status) =>
  assert.rejects(promise, (error) => {
    assert.equal(error.code, code)
    if (status) assert.equal(error.statusCode, status)
    return true
  })

test('存储：声明 storage 才能用；get / set / remove；缺失 404；key 规则', async (t) => {
  const { service, write, dataDir } = await fixture(t)
  await write('notes', {
    runtime: 'native',
    name: 'Notes',
    capabilities: { storage: { quotaKb: 1, reason: 'notes' } },
  })
  await write('plain', { runtime: 'native', name: 'Plain' })
  await write('old', { name: 'Legacy', entry: 'index.html' })

  await rejectsCode(service.storageGet('plain', 'a'), 'component_capability_denied', 403)
  await rejectsCode(service.storageGet('old', 'a'), 'component_legacy_unsupported', 409)
  await rejectsCode(service.storageGet('missing', 'a'), 'component_not_found', 404)
  await rejectsCode(service.storageGet('notes', 'a'), 'component_storage_missing', 404)
  for (const key of ['', '.hidden', 'a/b', 'a b', 'x'.repeat(129), '../x'])
    await rejectsCode(service.storageGet('notes', key), 'component_storage_key_invalid', 400)

  const saved = await service.storageSet('notes', 'note:1', { value: { text: 'hi', n: [1, 2] } })
  assert.equal(saved.ok, true)
  assert.equal(saved.quotaBytes, 1024)
  assert.ok(saved.usedBytes > 0 && saved.usedBytes <= 1024)
  assert.deepEqual(await service.storageGet('notes', 'note:1'), {
    key: 'note:1',
    value: { text: 'hi', n: [1, 2] },
  })
  const file = JSON.parse(await readFile(join(dataDir, 'custom-ui-data', 'notes.json'), 'utf8'))
  assert.deepEqual(file, { version: 1, values: { 'note:1': { text: 'hi', n: [1, 2] } } })
  assert.equal(
    (await readFile(join(dataDir, 'custom-ui-data', 'notes.json'))).length,
    saved.usedBytes,
  )
  assert.deepEqual(await service.storageRemove('notes', 'note:1'), { ok: true })
  assert.deepEqual(await service.storageRemove('notes', 'note:1'), { ok: true })
  await rejectsCode(service.storageGet('notes', 'note:1'), 'component_storage_missing', 404)
  // 存储不写进被监听的 custom-ui/：写存储不会触发热重载。
  assert.deepEqual((await readdir(join(service.root, 'notes'))).sort(), [
    'component.js',
    'manifest.json',
  ])
})

test('存储配额：按整个文件字节数计算，超出 413 且不落盘；请求体形状与严格 JSON', async (t) => {
  const { service, write, dataDir } = await fixture(t)
  await write('notes', {
    runtime: 'native',
    name: 'Notes',
    capabilities: { storage: { quotaKb: 1 } },
  })
  await service.storageSet('notes', 'a', { value: 'x'.repeat(500) })
  await rejectsCode(
    service.storageSet('notes', 'b', { value: 'y'.repeat(600) }),
    'component_storage_quota',
    413,
  )
  const file = JSON.parse(await readFile(join(dataDir, 'custom-ui-data', 'notes.json'), 'utf8'))
  assert.deepEqual(Object.keys(file.values), ['a'])
  // 覆盖同一个 key 时按替换后的大小算。
  await service.storageSet('notes', 'a', { value: 'z'.repeat(800) })
  for (const input of [null, {}, { value: 1, extra: 2 }, [1]])
    await rejectsCode(service.storageSet('notes', 'c', input), 'component_request_invalid', 400)
  await service.storageSet('notes', 'a', { value: 'bad \ud800 surrogate' })
  const raw = await readFile(join(dataDir, 'custom-ui-data', 'notes.json'), 'utf8')
  assert.doesNotMatch(raw, /\\ud800/i)
  assert.equal((await service.storageGet('notes', 'a')).value, 'bad � surrogate')
})

test('存储并发写按组件串行：同一毫秒的多次写入都不丢', async (t) => {
  const { service, write } = await fixture(t, { now: () => 1_700_000_000_000 })
  await write('notes', { runtime: 'native', name: 'Notes', capabilities: { storage: {} } })
  await Promise.all(
    Array.from({ length: 20 }, (_, index) =>
      service.storageSet('notes', `k${index}`, { value: index }),
    ),
  )
  for (let index = 0; index < 20; index += 1)
    assert.equal((await service.storageGet('notes', `k${index}`)).value, index)
})

test('删除组件：存储文件随组件进回收站，同名新组件不会读到旧值', async (t) => {
  const { service, write, dataDir } = await fixture(t)
  await write('notes', { runtime: 'native', name: 'Notes', capabilities: { storage: {} } })
  await service.storageSet('notes', 'a', { value: 'old' })
  const { trashedAs } = await service.deleteComponent('notes')
  const trashed = await readdir(join(dataDir, 'custom-ui-trash', trashedAs))
  assert.ok(trashed.includes('data.json'))
  await write('notes', { runtime: 'native', name: 'Notes', capabilities: { storage: {} } })
  await rejectsCode(service.storageGet('notes', 'a'), 'component_storage_missing', 404)
})

test('错误日志：字段校验、密钥脱敏、主目录缩写成 ~、长度上限', () => {
  const log = new ComponentErrorLog({ now: () => 1_700_000_000_000, home: '/home/alice' })
  for (const input of [
    null,
    {},
    { kind: 'render', level: 'error' },
    { kind: 'boom', level: 'error', message: 'x' },
    { kind: 'render', level: 'fatal', message: 'x' },
    { kind: 'render', level: 'error', message: 'x', extra: 1 },
    { kind: 'render', level: 'error', message: 'x', surface: 'board-card' },
    { kind: 'render', level: 'error', message: 'x', instanceId: 'bad id' },
    { kind: 'render', level: 'error', message: 'x', stack: 1 },
  ])
    assert.throws(
      () => log.record('demo', input),
      (error) => error.code === 'component_request_invalid',
    )
  log.record('demo', {
    kind: 'render',
    level: 'error',
    message: 'TypeError at /home/alice/.pisper/custom-ui/demo/component.js password=hunter2',
    stack: `Error\n    at file:///home/alice/x.js:1:1\n${'s'.repeat(5000)}`,
    instanceId: '6f1c2a40-1d3b-4c8e-9a7f-0b2d4e6f8a10',
    surface: 'overview-card',
  })
  const { entries, dropped } = log.list('demo')
  assert.equal(dropped, 0)
  assert.equal(entries.length, 1)
  assert.doesNotMatch(entries[0].message, /alice|hunter2/)
  assert.match(entries[0].message, /~\/\.pisper\/custom-ui\/demo\/component\.js/)
  assert.doesNotMatch(entries[0].stack, /alice/)
  assert.equal(entries[0].stack.length, 4000)
  assert.equal(entries[0].surface, 'overview-card')
  assert.equal(entries[0].at, new Date(1_700_000_000_000).toISOString())
  assert.equal(
    log.record('demo', { kind: 'log', level: 'warn', message: 'm'.repeat(800) }).ok,
    true,
  )
  assert.equal(log.list('demo').entries[1].message.length, 500)
})

test('错误日志：每分钟 20 条，超出 429 并计入 dropped；每个组件最多保留 100 条', () => {
  let now = 1_700_000_000_000
  const log = new ComponentErrorLog({ now: () => now, home: '' })
  const entry = (index) => ({ kind: 'log', level: 'warn', message: `m${index}` })
  for (let index = 0; index < 20; index += 1) log.record('demo', entry(index))
  assert.throws(
    () => log.record('demo', entry(99)),
    (error) => error.code === 'component_errors_rate_limited' && error.statusCode === 429,
  )
  assert.equal(log.list('demo').dropped, 1)
  log.record('other', entry(0))
  for (let minute = 1; minute <= 6; minute += 1) {
    now += 60_000
    for (let index = 0; index < 20; index += 1) log.record('demo', entry(minute * 100 + index))
  }
  const { entries } = log.list('demo')
  assert.equal(entries.length, 100)
  assert.equal(entries.at(-1).message, 'm619')
  log.forget('demo')
  assert.deepEqual(log.list('demo'), { entries: [], dropped: 0 })
})

test('绑定会话：首次新建“生成组件 · 名称”（cwd 是组件目录），之后复用；归档或不见了就新建', async (t) => {
  const { service, write } = await fixture(t)
  await write('ci', { runtime: 'native', name: 'CI 状态' })
  await write('old', { name: 'Legacy', entry: 'index.html' })
  const created = []
  const sessions = new Map()
  const deps = {
    createSession: async (name, cwd) => {
      const id = `s${created.length + 1}`
      created.push({ id, name, cwd })
      sessions.set(id, { archived: false })
      return { id }
    },
    findSession: async (id) => sessions.get(id) || null,
  }
  const first = await service.ensureAgentSession('ci', deps)
  assert.deepEqual(first, {
    sessionId: 's1',
    created: true,
    command: '/skill:pisper-component',
    session: { id: 's1' },
  })
  assert.deepEqual(created[0], {
    id: 's1',
    name: '生成组件 · CI 状态',
    cwd: join(service.root, 'ci'),
  })
  assert.equal((await service.state.get('ci')).boundSessionId, 's1')
  assert.deepEqual(await service.ensureAgentSession('ci', deps), {
    sessionId: 's1',
    created: false,
    command: '/skill:pisper-component',
  })
  sessions.get('s1').archived = true
  assert.equal((await service.ensureAgentSession('ci', deps)).sessionId, 's2')
  sessions.delete('s2')
  assert.equal((await service.ensureAgentSession('ci', deps)).sessionId, 's3')
  await rejectsCode(service.ensureAgentSession('old', deps), 'component_legacy_unsupported', 409)
  await rejectsCode(service.ensureAgentSession('nope', deps), 'component_not_found', 404)
})

test('HTTP：存储、错误与会话路由走鉴权链，请求体超限 413，错误体统一 {code, error}', async (t) => {
  const { service, write } = await fixture(t)
  await write('notes', { runtime: 'native', name: 'Notes', capabilities: { storage: {} } })
  const sessionsCreated = []
  const runtime = {
    sessionMeta: {},
    createSession: async (name, cwd) => {
      sessionsCreated.push({ name, cwd })
      return { id: 'session-1' }
    },
    findSessionInfo: async (id) => (id === 'session-1' ? { id } : null),
  }
  const handler = createApiHandler(runtime, { customUi: service })
  const server = createServer(async (req, res) => {
    const origin = `http://127.0.0.1:${server.address().port}`
    const url = new URL(req.url, origin)
    if (authorizeDesktopRequest(req, res, url, { token: 'fixture', origin })) return
    await handler(req, res, url)
  })
  await new Promise((resolveListen) => server.listen(0, '127.0.0.1', resolveListen))
  t.after(() => new Promise((resolveClose) => server.close(() => resolveClose())))
  const origin = `http://127.0.0.1:${server.address().port}`
  const headers = {
    Cookie: '__pisper_desktop=fixture',
    Origin: origin,
    'Content-Type': 'application/json',
  }
  const call = async (method, path, body) => {
    const response = await fetch(origin + path, {
      method,
      headers,
      ...(body === undefined
        ? {}
        : { body: typeof body === 'string' ? body : JSON.stringify(body) }),
    })
    return { status: response.status, body: await response.json() }
  }
  const base = '/api/custom-ui/components/notes'
  assert.equal((await fetch(`${origin}${base}/storage/a`)).status, 401)
  assert.deepEqual(await call('GET', `${base}/storage/a`), {
    status: 404,
    body: { code: 'component_storage_missing', error: 'component_storage_missing' },
  })
  assert.equal((await call('PUT', `${base}/storage/a`, { value: [1, 'two'] })).status, 200)
  assert.deepEqual((await call('GET', `${base}/storage/a`)).body, { key: 'a', value: [1, 'two'] })
  assert.equal(
    (await call('GET', `${base}/storage/a%2Fb`)).body.code,
    'component_storage_key_invalid',
  )
  assert.deepEqual((await call('DELETE', `${base}/storage/a`)).body, { ok: true })
  assert.equal(
    (await call('PUT', `${base}/storage/a`, '{bad')).body.code,
    'component_request_invalid',
  )
  const huge = await call('PUT', `${base}/storage/a`, { value: 'x'.repeat(1024 * 1024 + 10) })
  assert.equal(huge.status, 413)
  assert.equal(huge.body.code, 'component_request_too_large')

  const reported = await call('POST', `${base}/errors`, {
    kind: 'render',
    level: 'error',
    message: 'boom',
  })
  assert.deepEqual(reported, { status: 202, body: { ok: true } })
  const errors = await call('GET', `${base}/errors`)
  assert.equal(errors.status, 200)
  assert.equal(errors.body.entries[0].message, 'boom')
  assert.deepEqual(errors.body.network, { count: 0, lastAt: null, hosts: {} })
  const tooBig = await call('POST', `${base}/errors`, {
    kind: 'render',
    level: 'error',
    message: 'x'.repeat(20 * 1024),
  })
  assert.equal(tooBig.status, 413)
  assert.equal((await call('GET', '/api/custom-ui/components/nope/errors')).status, 404)

  const session = await call('POST', `${base}/session`, {})
  assert.deepEqual(session, {
    status: 200,
    // 只加字段：新建时带回会话摘要（与 POST /api/sessions 同形）；复用时没有。
    body: {
      sessionId: 'session-1',
      created: true,
      command: '/skill:pisper-component',
      session: { id: 'session-1' },
    },
  })
  const reused = (await call('POST', `${base}/session`, {})).body
  assert.equal(reused.created, false)
  assert.equal(Object.hasOwn(reused, 'session'), false)
  assert.equal(sessionsCreated.length, 1)
  // 会话被归档后不再复用。
  runtime.sessionMeta['session-1'] = { organization: { archived: true } }
  assert.equal((await call('POST', `${base}/session`, {})).body.created, true)
  assert.equal((await call('POST', `${base}/session`, { extra: 1 })).status, 400)
})
