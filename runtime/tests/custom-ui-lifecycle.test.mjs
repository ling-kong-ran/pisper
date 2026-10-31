// 组件信任与生命周期（A8，规格 5.4、5.7、5.9）：静态扫描、导入暂存与信任确认、版本 / 对比 / 回滚、导出 ZIP、
// 回收站恢复与 7 天清理、能力使用台账与撤销。真实服务器 + 桌面 Cookie 鉴权，与模块路由测试同一套脚手架。
import assert from 'node:assert/strict'
import { lstat, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { strToU8, zipSync } from 'fflate'
import { createApiHandler } from '../http/api-handler.mjs'
import { authorizeDesktopRequest } from '../desktop-sidecar-auth.mjs'
import { CustomUiService } from '../services/custom-ui-service.mjs'
import { unpackCustomUiZip } from '../services/custom-ui-import.mjs'
import { scanComponentSource } from '../services/custom-ui-scan.mjs'
import { TRASH_RETENTION_MS } from '../services/custom-ui-trash.mjs'

const DAY = 24 * 60 * 60 * 1000
const MANIFEST = {
  runtime: 'native',
  sdk: 1,
  name: 'CI 状态',
  version: '1.0.0',
  placements: [{ surface: 'overview-card', sizes: ['S', 'M'] }, { surface: 'feature-page' }],
  capabilities: {
    network: { hosts: ['api.github.com'], reason: '读取 CI' },
    storage: { quotaKb: 64, reason: '缓存' },
    notify: { reason: '失败提醒' },
  },
}
const SOURCE = 'export default function Card({ ctx }) { return ctx.html`<p>v1</p>` }\n'

function bundle(id = 'ci-status', files = {}, manifest = MANIFEST) {
  return zipSync({
    [`share/${id}/manifest.json`]: strToU8(JSON.stringify(manifest)),
    [`share/${id}/component.js`]: strToU8(SOURCE),
    ...Object.fromEntries(
      Object.entries(files).map(([path, content]) => [`share/${id}/${path}`, strToU8(content)]),
    ),
  })
}

async function startServer(t, { now } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-lifecycle-'))
  let clock = Date.parse('2026-10-31T08:00:00.000Z')
  const service = new CustomUiService({ dataDir: directory, now: now || (() => clock) })
  const events = []
  await service.start({
    onChange: (id, detail) => events.push({ id, ...detail }),
    hostVersion: '0.6.5',
  })
  const handler = createApiHandler({}, { customUi: service })
  const server = createServer(async (req, res) => {
    const origin = `http://127.0.0.1:${server.address().port}`
    const url = new URL(req.url, origin)
    if (authorizeDesktopRequest(req, res, url, { token: 'fixture', origin })) return
    await handler(req, res, url)
  })
  t.after(async () => {
    service.dispose()
    await new Promise((resolveClose, reject) =>
      server.close((error) => (error ? reject(error) : resolveClose())),
    )
    await rm(directory, { recursive: true, force: true })
  })
  await new Promise((resolveListen) => server.listen(0, '127.0.0.1', resolveListen))
  const origin = `http://127.0.0.1:${server.address().port}`
  const headers = { Cookie: '__pisper_desktop=fixture', Origin: origin }
  const call = (method, path, body, extra = {}) =>
    fetch(origin + path, {
      method,
      headers: { ...headers, ...extra },
      ...(body === undefined ? {} : { body }),
    })
  const callJson = async (method, path, body) => {
    const response = await call(
      method,
      path,
      body === undefined ? undefined : JSON.stringify(body),
      body === undefined ? {} : { 'Content-Type': 'application/json' },
    )
    return { status: response.status, body: await response.json() }
  }
  const upload = async (bytes, name = 'ci-status.zip') => {
    const response = await call(
      'POST',
      `/api/custom-ui/import?stage=1&name=${encodeURIComponent(name)}`,
      bytes,
      { 'Content-Type': 'application/zip' },
    )
    return { status: response.status, body: await response.json() }
  }
  return {
    directory,
    service,
    events,
    call,
    callJson,
    upload,
    tick: (ms) => {
      clock += ms
    },
  }
}

test('静态扫描：直连 fetch / XHR / WebSocket / eval 标红，ctx.net.fetch 不算；主机按声明对照', () => {
  const result = scanComponentSource(
    [
      {
        path: 'component.js',
        content: [
          "const ok = await ctx.net.fetch('https://api.github.com/repos')",
          "const raw = await fetch('https://evil.example.com/x')",
          "window.fetch('/api/sessions')",
          'new XMLHttpRequest()',
          "new WebSocket('wss://socket.example.org')",
          "eval('1')",
          "new Function('return 1')",
          'document.cookie',
          "localStorage.getItem('k')",
          "import('./lazy.js')",
          'const svg = "http://www.w3.org/2000/svg"',
          "const sub = 'https://ci.github.io/badge'",
        ].join('\n'),
      },
      { path: 'style.css', content: '.x { background: url(https://cdn.example.net/a.png) }' },
      { path: 'logo.png', content: new Uint8Array([1, 2, 3]) },
    ],
    { capabilities: { network: { hosts: ['api.github.com', '*.github.io'] } } },
  )
  assert.equal(result.status, 'ok')
  const rules = result.findings.map((entry) => `${entry.rule}:${entry.line}`)
  assert.deepEqual(rules.sort(), [
    'app-api:3',
    'cookie:8',
    'dynamic-import:10',
    'eval:6',
    'fetch:2',
    'fetch:3',
    'function:7',
    'local-storage:9',
    'websocket:5',
    'xhr:4',
  ])
  assert.equal(
    result.findings.find((entry) => entry.rule === 'fetch' && entry.line === 2).excerpt,
    "const raw = await fetch('https://evil.example.com/x')",
  )
  assert.deepEqual(
    result.hosts.map((entry) => [entry.host, entry.declared]),
    [
      ['api.github.com', true],
      ['evil.example.com', false],
      ['socket.example.org', false],
      ['ci.github.io', true],
      ['cdn.example.net', false],
    ],
  )
  assert.deepEqual(result.summary, { danger: 6, warn: 3, info: 1, undeclaredHosts: 3 })
})

test('导入暂存：只暂存不安装；旧格式与坏包拒绝；信任后启用、来源 import，暂存删除', async (t) => {
  const { service, callJson, call, upload, events } = await startServer(t)
  const legacy = zipSync({
    'old/manifest.json': strToU8(JSON.stringify({ name: 'Old', entry: 'index.html' })),
    'old/index.html': strToU8('<p>'),
  })
  assert.deepEqual((await upload(legacy)).body.code, 'component_legacy_unsupported')
  const html = await upload(bundle('ci-status', { 'page.html': '<p>' }))
  assert.equal(html.status, 400)
  assert.equal(html.body.errors[0].code, 'component_html_unsupported')

  const staged = await upload(bundle(), '../../evil\nname.zip')
  assert.equal(staged.status, 201)
  const stageId = staged.body.staged.stageId
  assert.match(stageId, /^[a-f0-9]{16}$/)
  assert.equal(staged.body.staged.fileName, 'evilname.zip')
  assert.equal(staged.body.staged.replaces, null)
  assert.match(staged.body.staged.sha256, /^[a-f0-9]{64}$/)
  // 不进 custom-ui/、不执行、不发事件；模块路由服务不到它。
  assert.equal(await lstat(join(service.root, 'ci-status')).catch(() => null), null)
  assert.equal(events.length, 0)
  assert.equal(
    (await call('GET', '/api/custom-ui/components/ci-status/r/x/component.js')).status,
    404,
  )
  const catalog = await callJson('GET', '/api/custom-ui/components')
  assert.deepEqual(
    catalog.body.staged.map((row) => [row.id, row.fileName]),
    [['ci-status', 'evilname.zip']],
  )

  const detail = await callJson('GET', `/api/custom-ui/staged/${stageId}`)
  assert.equal(detail.status, 200)
  assert.deepEqual(
    detail.body.files.map((file) => file.path),
    ['component.js', 'manifest.json'],
  )
  assert.equal(detail.body.manifest.capabilities.network.hosts[0], 'api.github.com')
  assert.equal(detail.body.scan.status, 'ok')
  assert.equal(detail.body.installed, null)
  assert.equal(detail.body.diff, null)
  const file = await callJson('GET', `/api/custom-ui/staged/${stageId}/file?path=component.js`)
  assert.equal(file.body.content, SOURCE)
  for (const path of ['../meta.json', '/etc/passwd', '.hidden', ''])
    assert.equal(
      (
        await callJson(
          'GET',
          `/api/custom-ui/staged/${stageId}/file?path=${encodeURIComponent(path)}`,
        )
      ).status,
      400,
      path,
    )
  assert.equal((await callJson('GET', '/api/custom-ui/staged/not-an-id')).status, 404)

  const trusted = await callJson('POST', `/api/custom-ui/staged/${stageId}/trust`, {})
  assert.equal(trusted.status, 200)
  assert.equal(trusted.body.replaced, false)
  assert.equal(trusted.body.component.status, 'ready')
  assert.equal(trusted.body.component.enabled, true)
  assert.equal(trusted.body.component.source, 'import')
  assert.deepEqual(events.at(-1), {
    id: 'ci-status',
    rev: trusted.body.component.rev,
    reason: 'created',
  })
  assert.equal((await callJson('GET', `/api/custom-ui/staged/${stageId}`)).status, 404)
  assert.deepEqual((await callJson('GET', '/api/custom-ui/components')).body.staged, [])

  // 丢弃：暂存目录删掉，组件目录不受影响。
  const second = await upload(bundle('other-card'))
  assert.equal(
    (await callJson('POST', `/api/custom-ui/staged/${second.body.staged.stageId}/discard`)).status,
    200,
  )
  assert.deepEqual(await readdir(join(service.root, '..', 'custom-ui-staging')), [])
})

test('替换导入：给出与已安装版本的 diff，信任后先快照再替换，可以回滚', async (t) => {
  const { service, callJson, upload } = await startServer(t)
  const first = await upload(bundle())
  await callJson('POST', `/api/custom-ui/staged/${first.body.staged.stageId}/trust`, {})
  const next = zipSync({
    'ci-status/manifest.json': strToU8(JSON.stringify({ ...MANIFEST, version: '1.1.0' })),
    'ci-status/component.js': strToU8(SOURCE.replace('v1', 'v2')),
    'ci-status/style.css': strToU8('.card {}\n'),
  })
  const staged = await upload(next, 'ci-status-1.1.zip')
  assert.equal(staged.body.staged.replaces, 'native')
  const detail = await callJson('GET', `/api/custom-ui/staged/${staged.body.staged.stageId}`)
  assert.deepEqual(detail.body.diff.files, [
    { path: 'component.js', status: 'modified' },
    { path: 'manifest.json', status: 'modified' },
    { path: 'style.css', status: 'added' },
  ])
  assert.match(detail.body.diff.diff, /^diff --git a\/component\.js b\/component\.js$/m)
  assert.match(detail.body.diff.diff, /^-export default .*v1/m)
  assert.match(detail.body.diff.diff, /^\+export default .*v2/m)
  assert.equal(detail.body.installed.version, '1.0.0')

  const trusted = await callJson(
    'POST',
    `/api/custom-ui/staged/${staged.body.staged.stageId}/trust`,
    {},
  )
  assert.equal(trusted.body.replaced, true)
  assert.equal(trusted.body.component.version, '1.1.0')
  const versions = await callJson('GET', '/api/custom-ui/components/ci-status/versions')
  assert.equal(versions.body.current.n, 2)
  assert.equal(versions.body.current.source, 'import')
  assert.equal(versions.body.current.summary, '导入 ci-status-1.1.zip')
  assert.deepEqual(versions.body.current.files.map((entry) => entry.path).sort(), [
    'component.js',
    'manifest.json',
    'style.css',
  ])
  assert.deepEqual(
    versions.body.versions.map((entry) => entry.n),
    [1],
  )
  assert.equal(
    await readFile(join(service.root, 'ci-status', 'component.js'), 'utf8'),
    SOURCE.replace('v1', 'v2'),
  )
})

test('版本：代理写入产生快照与改动摘要；对比给出统一 diff；回滚生成新版本、不改写历史', async (t) => {
  const { service, callJson, events, tick } = await startServer(t)
  await service.writeComponent({
    mode: 'create',
    id: 'notes',
    manifest: { runtime: 'native', name: '便签', version: '1.0.0' },
    files: [{ path: 'component.js', content: 'export default () => "v1"\n' }],
  })
  tick(1000)
  await service.writeComponent({
    mode: 'update',
    id: 'notes',
    files: [{ path: 'component.js', content: 'export default () => "v2"\nconst extra = 1\n' }],
    summary: '加一行',
  })
  tick(1000)
  await service.writeComponent({
    mode: 'update',
    id: 'notes',
    files: [{ path: 'lib.js', content: 'export const x = 1\n' }],
    summary: '拆出 lib',
  })
  const versions = await callJson('GET', '/api/custom-ui/components/notes/versions')
  assert.equal(versions.status, 200)
  assert.equal(versions.body.max, 20)
  assert.deepEqual(versions.body.current, {
    n: 3,
    at: '2026-10-31T08:00:02.000Z',
    summary: '拆出 lib',
    source: 'agent',
    files: [{ path: 'lib.js', added: 2, removed: 0 }],
    deleted: [],
  })
  assert.deepEqual(
    versions.body.versions.map((entry) => [entry.n, entry.summary, entry.source]),
    [
      [2, '加一行', 'agent'],
      [1, '', null],
    ],
  )
  assert.deepEqual(versions.body.versions[0].files, [
    { path: 'component.js', added: 2, removed: 1 },
  ])

  const diff = await callJson('GET', '/api/custom-ui/components/notes/versions/1/diff')
  assert.equal(diff.status, 200)
  assert.deepEqual(diff.body.files, [
    { path: 'component.js', status: 'modified' },
    { path: 'lib.js', status: 'added' },
  ])
  assert.match(diff.body.diff, /^new file mode 100644$/m)
  assert.equal(diff.body.truncated, false)
  for (const n of ['0', '9', 'x'])
    assert.equal(
      (await callJson('GET', `/api/custom-ui/components/notes/versions/${n}/diff`)).status,
      404,
    )

  const restored = await callJson('POST', '/api/custom-ui/components/notes/versions/1/restore', {})
  assert.equal(restored.status, 200)
  assert.equal(restored.body.restored, 1)
  assert.equal(restored.body.current.n, 4)
  assert.equal(restored.body.current.summary, '回滚到 v1')
  assert.equal(restored.body.current.source, 'restore')
  assert.deepEqual(restored.body.current.deleted, ['lib.js'])
  assert.deepEqual(
    restored.body.versions.map((entry) => entry.n),
    [3, 2, 1],
  )
  assert.equal(
    await readFile(join(service.root, 'notes', 'component.js'), 'utf8'),
    'export default () => "v1"\n',
  )
  assert.equal(await lstat(join(service.root, 'notes', 'lib.js')).catch(() => null), null)
  assert.equal(events.at(-1).reason, 'files')
  assert.equal(
    (await callJson('POST', '/api/custom-ui/components/notes/versions/1/restore', { force: true }))
      .status,
    400,
  )
})

test('导出 ZIP：与导入同一布局，只含组件文件，同一内容两次导出字节相同', async (t) => {
  const { service, call } = await startServer(t)
  await service.writeComponent({
    mode: 'create',
    id: 'ci-status',
    manifest: MANIFEST,
    files: [
      { path: 'component.js', content: SOURCE },
      { path: 'lib/util.js', content: 'export const a = 1\n' },
    ],
  })
  await service.storageSet('ci-status', 'cache', { value: { secret: 'not exported' } })
  const response = await call('GET', '/api/custom-ui/components/ci-status/export')
  assert.equal(response.status, 200)
  assert.equal(response.headers.get('content-type'), 'application/zip')
  assert.equal(response.headers.get('content-disposition'), 'attachment; filename="ci-status.zip"')
  const bytes = new Uint8Array(await response.arrayBuffer())
  const { id, assets } = unpackCustomUiZip(bytes)
  assert.equal(id, 'ci-status')
  assert.deepEqual([...assets.keys()].sort(), ['component.js', 'lib/util.js', 'manifest.json'])
  assert.ok(!Buffer.from(bytes).includes('not exported'))
  const again = new Uint8Array(
    await (await call('GET', '/api/custom-ui/components/ci-status/export')).arrayBuffer(),
  )
  assert.deepEqual(again, bytes)
  assert.equal((await call('GET', '/api/custom-ui/components/missing/export')).status, 404)
})

test('回收站：删除后列出、7 天到期；恢复目录、存储、状态与放置；同 id 已存在拒绝；永久删除', async (t) => {
  const { service, callJson } = await startServer(t)
  await service.writeComponent({
    mode: 'create',
    id: 'ci-status',
    manifest: MANIFEST,
    files: [{ path: 'component.js', content: SOURCE }],
  })
  await service.setEnabled('ci-status', { enabled: false })
  await service.setEnabled('ci-status', { enabled: true })
  await service.storageSet('ci-status', 'cache', { value: 7 })
  await service.createInstance({ componentId: 'ci-status', surface: 'overview-card', size: 'M' })
  await service.deleteComponent('ci-status')

  const listed = await callJson('GET', '/api/custom-ui/trash')
  assert.equal(listed.body.retentionDays, 7)
  const [entry] = listed.body.entries
  assert.equal(entry.id, 'ci-status')
  assert.equal(entry.kind, 'native')
  assert.equal(entry.reason, 'deleted')
  assert.equal(entry.instances, 1)
  assert.equal(entry.hasData, true)
  assert.equal(Date.parse(entry.expiresAt) - Date.parse(entry.deletedAt), TRASH_RETENTION_MS)

  // 同 id 新组件占着：恢复被拒绝，不覆盖。
  await service.writeComponent({
    mode: 'create',
    id: 'ci-status',
    manifest: MANIFEST,
    files: [{ path: 'component.js', content: SOURCE }],
  })
  assert.equal(
    (await callJson('POST', `/api/custom-ui/trash/${entry.entry}/restore`, {})).body.code,
    'component_already_installed',
  )
  await service.deleteComponent('ci-status')
  const entries = (await callJson('GET', '/api/custom-ui/trash')).body.entries
  assert.equal(entries.length, 2)
  // 先永久删除较新的那条，再恢复最早那条。
  const newest = entries.find((item) => item.entry !== entry.entry)
  assert.equal((await callJson('DELETE', `/api/custom-ui/trash/${newest.entry}`)).status, 200)
  const restored = await callJson('POST', `/api/custom-ui/trash/${entry.entry}/restore`, {})
  assert.equal(restored.status, 200)
  assert.equal(restored.body.restoredInstances, 1)
  assert.equal(restored.body.component.status, 'ready')
  assert.deepEqual(await service.storageGet('ci-status', 'cache'), { key: 'cache', value: 7 })
  const { instances } = await service.listInstances()
  assert.deepEqual(
    instances.map((instance) => [instance.componentId, instance.surface, instance.size]),
    [['ci-status', 'overview-card', 'M']],
  )
  assert.deepEqual((await callJson('GET', '/api/custom-ui/trash')).body.entries, [])
  for (const bad of ['..', 'ci-status', 'ci-status-20261031T000000Z%2F..'])
    assert.equal(
      (await callJson('POST', `/api/custom-ui/trash/${bad}/restore`, {})).status,
      404,
      bad,
    )
})

test('回收站 7 天清理：启动时删过期的已删除组件，迁移进来的旧组件不自动删', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-trash-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const trash = join(directory, 'custom-ui-trash')
  const write = async (name, meta) => {
    await mkdir(join(trash, name, 'component'), { recursive: true })
    await writeFile(join(trash, name, 'component', 'manifest.json'), '{"runtime":"native"}')
    await writeFile(join(trash, name, 'meta.json'), JSON.stringify(meta))
  }
  await write('old-card-20261020T000000Z', { deletedAt: '2026-10-20T00:00:00.000Z' })
  await write('fresh-card-20261030T000000Z', { deletedAt: '2026-10-30T00:00:00.000Z' })
  await write('weather-20261020T000000Z', {
    deletedAt: '2026-10-20T00:00:00.000Z',
    reason: 'legacy-retired',
  })
  const service = new CustomUiService({
    dataDir: directory,
    now: () => Date.parse('2026-10-31T00:00:00.000Z'),
  })
  await service.start()
  service.dispose()
  assert.deepEqual((await readdir(trash)).sort(), [
    'fresh-card-20261030T000000Z',
    'weather-20261020T000000Z',
  ])
  const listed = await service.listTrashEntries()
  const legacy = listed.entries.find((item) => item.id === 'weather')
  assert.equal(legacy.reason, 'legacy-retired')
  assert.equal(legacy.expiresAt, null)
  assert.equal(
    listed.entries.find((item) => item.id === 'fresh-card').expiresAt,
    new Date(Date.parse('2026-10-30T00:00:00.000Z') + 7 * DAY).toISOString(),
  )
})

test('能力台账与撤销：网络与存储由 Runtime 记，notify / sessions 由 ctx 上报；撤销后 Runtime 拒绝', async (t) => {
  const { service, callJson } = await startServer(t)
  await service.writeComponent({
    mode: 'create',
    id: 'ci-status',
    manifest: MANIFEST,
    files: [{ path: 'component.js', content: SOURCE }],
  })
  assert.equal(
    (await callJson('PUT', '/api/custom-ui/components/ci-status/storage/cache', { value: 1 }))
      .status,
    200,
  )
  assert.equal(
    (
      await callJson('POST', '/api/custom-ui/components/ci-status/usage', {
        entries: [
          { capability: 'notify', count: 2, target: '构建失败' },
          { capability: 'sessions.read', count: 1 },
        ],
      })
    ).status,
    202,
  )
  // 组件不能替 Runtime 记网络与存储；格式不对整批拒绝。
  for (const entries of [
    [{ capability: 'network', count: 1 }],
    [{ capability: 'notify', count: 0 }],
    [{ capability: 'notify', count: 1, target: 5 }],
  ])
    assert.equal(
      (await callJson('POST', '/api/custom-ui/components/ci-status/usage', { entries })).status,
      400,
    )
  const usage = await callJson('GET', '/api/custom-ui/components/ci-status/usage')
  assert.equal(usage.status, 200)
  assert.deepEqual(usage.body.revoked, [])
  assert.deepEqual(
    Object.fromEntries(
      Object.entries(usage.body.usage).map(([key, value]) => [key, [value.today, value.target]]),
    ),
    { storage: [1, 'set'], notify: [2, '构建失败'], 'sessions.read': [1, ''] },
  )

  const revoked = await callJson('PATCH', '/api/custom-ui/components/ci-status', {
    revoked: ['storage', 'network'],
  })
  assert.equal(revoked.status, 200)
  assert.deepEqual(revoked.body.component.revoked, ['network', 'storage'])
  assert.equal(revoked.body.component.enabled, true)
  const denied = await callJson('GET', '/api/custom-ui/components/ci-status/storage/cache')
  assert.deepEqual(
    [denied.status, denied.body.code, denied.body.capability],
    [403, 'component_capability_revoked', 'storage'],
  )
  const fetchDenied = await callJson('POST', '/api/custom-ui/components/ci-status/fetch', {
    url: 'https://api.github.com/repos',
  })
  assert.deepEqual(
    [fetchDenied.status, fetchDenied.body.code],
    [403, 'component_capability_revoked'],
  )
  for (const body of [{ revoked: ['clipboard'] }, { revoked: 'storage' }, {}, { other: true }])
    assert.equal(
      (await callJson('PATCH', '/api/custom-ui/components/ci-status', body)).status,
      400,
      JSON.stringify(body),
    )
  const back = await callJson('PATCH', '/api/custom-ui/components/ci-status', { revoked: [] })
  assert.deepEqual(back.body.component.revoked, [])
  assert.equal(
    (await callJson('GET', '/api/custom-ui/components/ci-status/storage/cache')).status,
    200,
  )
})
