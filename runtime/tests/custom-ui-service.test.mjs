// 原生组件服务测试：路径缩写、v2 目录分类、资源路径安全、旧 iframe 机制删除后的 410 兼容桩与鉴权前入口，
// 以及启动时把旧组件移进回收站的一次性迁移（负责人决定，A7 S4）。
import assert from 'node:assert/strict'
import { lstat, mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { displayCustomUiPath } from '../services/custom-ui-path.mjs'
import { resolveComponentFile } from '../services/custom-ui-files.mjs'
import { CustomUiService } from '../services/custom-ui-service.mjs'

const NATIVE = { runtime: 'native', name: 'Native', version: '1.0.0' }
const NATIVE_FILES = { 'component.js': 'export default function C() { return null }\n' }

async function createComponent(root, id, manifest, files = {}) {
  const dir = join(root, id)
  await mkdir(dir, { recursive: true })
  await writeFile(join(dir, 'manifest.json'), JSON.stringify(manifest))
  for (const [path, content] of Object.entries(files)) {
    const target = join(dir, path)
    await mkdir(join(target, '..'), { recursive: true })
    await writeFile(target, content)
  }
  return dir
}

test('Windows 组件目录用可展开的主目录变量且不展示用户名', () => {
  const options = { home: 'C:\\Users\\Fixture User', platform: 'win32' }
  assert.equal(
    displayCustomUiPath('C:\\Users\\Fixture User\\.pisper\\agent\\custom-ui', options),
    '%USERPROFILE%\\.pisper\\agent\\custom-ui',
  )
  assert.equal(
    displayCustomUiPath('c:/users/FIXTURE USER/.pisper/agent/custom-ui/my-board', options),
    '%USERPROFILE%\\.pisper\\agent\\custom-ui\\my-board',
  )
  assert.equal(displayCustomUiPath('C:\\Users\\Fixture User\\', options), '%USERPROFILE%')
})

test('Windows 主目录边界不缩写相似用户名、自定义盘符和外部 UNC 目录', () => {
  const options = { home: 'C:\\Users\\Fixture', platform: 'win32' }
  for (const path of [
    'C:\\Users\\Fixture-other\\agent\\custom-ui',
    'C:\\Users\\Fixture\\..\\Other\\custom-ui',
    'D:\\Pisper Data\\agent\\custom-ui',
    '\\\\fileserver\\Pisper Data\\agent\\custom-ui',
  ]) {
    assert.equal(displayCustomUiPath(path, options), path)
  }
  assert.equal(
    displayCustomUiPath('\\\\fileserver\\profiles\\Fixture\\agent\\custom-ui', {
      home: '\\\\FILESERVER\\profiles\\Fixture',
      platform: 'win32',
    }),
    '%USERPROFILE%\\agent\\custom-ui',
  )
})

test('POSIX 组件目录保留主目录缩写并尊重大小写和配置覆盖', () => {
  const options = { home: '/home/fixture', platform: 'linux' }
  assert.equal(
    displayCustomUiPath('/home/fixture/.pisper/agent/custom-ui', options),
    '~/.pisper/agent/custom-ui',
  )
  assert.equal(displayCustomUiPath('/home/fixture/', options), '~')
  for (const path of [
    '/home/fixture-other/agent/custom-ui',
    '/home/Fixture/agent/custom-ui',
    '/srv/pisper data/agent/custom-ui',
  ]) {
    assert.equal(displayCustomUiPath(path, options), path)
  }
  assert.equal(
    displayCustomUiPath('/Users/fixture/.pisper/agent/custom-ui', {
      home: '/Users/fixture',
      platform: 'darwin',
    }),
    '~/.pisper/agent/custom-ui',
  )
})

test('主目录未知时不猜测组件路径归属', () => {
  assert.equal(
    displayCustomUiPath('C:\\Pisper\\agent\\custom-ui', { home: '', platform: 'win32' }),
    'C:\\Pisper\\agent\\custom-ui',
  )
  assert.equal(
    displayCustomUiPath('agent/custom-ui', { home: '/home/fixture', platform: 'linux' }),
    'agent/custom-ui',
  )
})

test('catalog classifies native, legacy and broken directories and skips invalid names', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-'))
  t.after(async () => rm(directory, { recursive: true, force: true }))
  const service = new CustomUiService({ dataDir: directory })
  // 目录不存在 → 空列表而不是抛错。
  assert.deepEqual(await service.catalog(), {
    root: displayCustomUiPath(service.root),
    features: { import: false, versions: false, export: false, trash: 'move' },
    components: [],
  })
  const root = join(directory, 'custom-ui')
  await createComponent(root, 'native-card', { ...NATIVE, name: 'Native Card' }, NATIVE_FILES)
  await createComponent(
    root,
    'demo-board',
    { name: 'Demo Board', permissions: ['sessions.read'] },
    { 'index.html': '<html></html>' },
  )
  await mkdir(join(root, 'no-manifest'), { recursive: true })
  await mkdir(join(root, 'UPPER CASE!'), { recursive: true })
  await mkdir(join(root, '.component-write-x'), { recursive: true })
  const listed = await service.catalog()
  const rows = Object.fromEntries(listed.components.map((row) => [row.id, row]))
  assert.deepEqual(Object.keys(rows).sort(), ['demo-board', 'native-card', 'no-manifest'])
  assert.equal(rows['native-card'].kind, 'native')
  assert.equal(rows['native-card'].status, 'ready')
  assert.match(rows['native-card'].rev, /^[a-f0-9]{24}$/)
  assert.equal(rows['demo-board'].kind, 'legacy')
  assert.equal(rows['demo-board'].status, 'unsupported')
  assert.equal(rows['demo-board'].enabled, false)
  assert.equal(rows['no-manifest'].kind, 'broken')
  assert.deepEqual(rows['no-manifest'].errors, [
    { path: 'manifest.json', code: 'manifest_missing' },
  ])
  // 新列表不再返回旧字段：旧前端的 isComponent 会整体拒绝，不会把组件当成可用。
  for (const row of listed.components) {
    assert.equal('entryUrl' in row, false)
    assert.equal('permissions' in row, false)
    assert.equal('builtIn' in row, false)
  }
  assert.equal(
    rows['native-card'].directory,
    displayCustomUiPath(service.componentDir('native-card')),
  )
})

test('resolveComponentFile blocks traversal, hidden files, manifest and symlinks', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-assets-'))
  t.after(async () => rm(directory, { recursive: true, force: true }))
  const root = join(directory, 'custom-ui')
  await createComponent(root, 'demo', NATIVE, { ...NATIVE_FILES, 'js/app.js': 'console.log(1)' })
  const entry = await resolveComponentFile(root, 'demo', 'component.js')
  assert.ok(entry?.file.endsWith('component.js'))
  const nested = await resolveComponentFile(root, 'demo', 'js/app.js')
  assert.ok(nested?.file.endsWith('app.js'))
  // 路径穿越与绝对路径。
  assert.equal(await resolveComponentFile(root, 'demo', '../demo/component.js'), null)
  assert.equal(await resolveComponentFile(root, 'demo', '../../etc/passwd'), null)
  assert.equal(await resolveComponentFile(root, 'demo', '/etc/passwd'), null)
  assert.equal(await resolveComponentFile(root, 'demo', '..'), null)
  // manifest.json 与隐藏文件不作为资源。
  assert.equal(await resolveComponentFile(root, 'demo', 'manifest.json'), null)
  await writeFile(join(root, 'demo', '.secret'), 'x')
  assert.equal(await resolveComponentFile(root, 'demo', '.secret'), null)
  // 目录外符号链接被拒绝。
  const outside = join(directory, 'outside.txt')
  await writeFile(outside, 'secret')
  await symlink(outside, join(root, 'demo', 'linked.txt'))
  assert.equal(await resolveComponentFile(root, 'demo', 'linked.txt'), null)
  // 非法组件 id 与不存在的文件。
  assert.equal(await resolveComponentFile(root, '../x', 'component.js'), null)
  assert.equal(await resolveComponentFile(root, 'demo', 'missing.js'), null)
})

test('资源拒绝整个组件目录的符号链接逃逸及绝对路径', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-root-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new CustomUiService({ dataDir: directory })
  await createComponent(service.root, 'demo', NATIVE, NATIVE_FILES)
  const outside = join(directory, 'outside')
  await mkdir(outside)
  await writeFile(join(outside, 'secret.txt'), 'fixture')
  await symlink(outside, join(service.root, 'alias'))
  assert.equal(await resolveComponentFile(service.root, 'alias', 'secret.txt'), null)
  assert.equal(await resolveComponentFile(service.root, 'demo', '/component.js'), null)
  // 符号链接的组件目录不出现在列表里。
  assert.deepEqual(
    (await service.catalog()).components.map((row) => row.id),
    ['demo'],
  )
})

// 真实服务器 + 桌面 Cookie 鉴权 + createApiHandler（与旧视图凭证测试同一套脚手架）。
async function startServer(t, service) {
  const { createApiHandler } = await import('../http/api-handler.mjs')
  const { authorizeDesktopRequest } = await import('../desktop-sidecar-auth.mjs')
  const handler = createApiHandler({}, { customUi: service })
  const server = createServer(async (req, res) => {
    const origin = `http://127.0.0.1:${server.address().port}`
    const url = new URL(req.url, origin)
    if (authorizeDesktopRequest(req, res, url, { token: 'fixture', origin })) return
    if (!(await handler(req, res, url))) {
      res.writeHead(404, { 'Content-Type': 'application/json' })
      res.end('{"error":"not found"}')
    }
  })
  t.after(async () => {
    service.dispose()
    await new Promise((resolveClose, reject) =>
      server.close((error) => (error ? reject(error) : resolveClose())),
    )
  })
  await new Promise((resolveListen) => server.listen(0, '127.0.0.1', resolveListen))
  const origin = `http://127.0.0.1:${server.address().port}`
  const headers = { Cookie: '__pisper_desktop=fixture', Origin: origin }
  return { origin, headers }
}

test('HTTP: GET /components returns the v2 catalog; old iframe routes answer 410 and need auth', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-api-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new CustomUiService({ dataDir: directory })
  await createComponent(service.root, 'demo', NATIVE, NATIVE_FILES)
  await createComponent(service.root, 'old', { name: 'Old' }, { 'index.html': '<h1>old</h1>' })
  const { origin, headers } = await startServer(t, service)

  const list = await fetch(`${origin}/api/custom-ui/components`, { headers })
  assert.equal(list.status, 200)
  const payload = await list.json()
  assert.equal(payload.root, displayCustomUiPath(service.root))
  assert.deepEqual(payload.features, {
    import: false,
    versions: false,
    export: false,
    trash: 'move',
  })
  assert.deepEqual(
    payload.components.map((row) => [row.id, row.kind]),
    [
      ['demo', 'native'],
      ['old', 'legacy'],
    ],
  )

  const removed = [
    ['POST', '/api/custom-ui/components/old/views'],
    ['PUT', '/api/custom-ui/views/' + 'a'.repeat(64)],
    ['DELETE', '/api/custom-ui/views/' + 'a'.repeat(64)],
    ['GET', '/api/custom-ui/bridge.js'],
    ['GET', '/api/custom-ui/components/old/assets/index.html'],
    ['GET', '/api/custom-ui/components/old/assets/js/app.js'],
    ['GET', '/api/custom-ui/components/old/assets/a/b/c.js'],
  ]
  for (const [method, path] of removed) {
    // 兼容桩也在鉴权链之后：没有 Cookie 是 401，不是 410。
    assert.equal((await fetch(origin + path, { method })).status, 401, `${method} ${path}`)
    const response = await fetch(origin + path, { method, headers })
    assert.equal(response.status, 410, `${method} ${path}`)
    assert.deepEqual(await response.json(), {
      code: 'custom_ui_legacy_removed',
      error: 'custom_ui_legacy_removed',
    })
  }
  // 旧组件文件没有被兼容桩读取或改动。
  assert.equal(await readFile(join(service.root, 'old', 'index.html'), 'utf8'), '<h1>old</h1>')
})

test('HTTP: the pre-auth /api/custom-ui/render/* entry is gone (no bypass before authentication)', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-render-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new CustomUiService({ dataDir: directory })
  await createComponent(service.root, 'old', { name: 'Old' }, { 'index.html': '<h1>old</h1>' })
  const { origin, headers } = await startServer(t, service)
  const path = `/api/custom-ui/render/${'a'.repeat(64)}/assets/index.html`
  assert.equal((await fetch(origin + path)).status, 401)
  assert.equal((await fetch(origin + path, { headers })).status, 404)
  const { readFile: readSource } = await import('node:fs/promises')
  const runtimeSource = await readSource(new URL('../app-runtime.mjs', import.meta.url), 'utf8')
  assert.doesNotMatch(runtimeSource, /handleCustomUiResource/)
  const routes = await import('../http/routes/custom-ui.mjs')
  assert.equal('handleCustomUiResource' in routes, false)
})

async function writeState(directory, components) {
  await writeFile(
    join(directory, 'custom-ui-state.json'),
    JSON.stringify({ version: 1, components }),
  )
}

test('startup migration moves legacy and broken directories to the trash and leaves native components alone', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-migrate-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const root = join(directory, 'custom-ui')
  await createComponent(root, 'native-card', NATIVE, NATIVE_FILES)
  // manifest 有错的原生组件也是原生：不动（让代理或用户修它）。
  await createComponent(root, 'native-bad', { runtime: 'native', name: '' }, NATIVE_FILES)
  await createComponent(
    root,
    'old-board',
    { name: 'Old', permissions: ['notify'] },
    {
      'index.html': '<h1>old</h1>',
      'js/app.js': 'window.x = 1',
    },
  )
  await mkdir(join(root, 'no-manifest'), { recursive: true })
  await writeFile(join(root, 'no-manifest', 'notes.txt'), 'keep me')
  await createComponent(root, 'bad-json', '{not json')
  await writeFile(join(root, 'bad-json', 'manifest.json'), '{not json')
  // 不在列表里的目录不碰：非法名字、点开头的暂存目录。
  await mkdir(join(root, 'UPPER CASE!'), { recursive: true })
  await mkdir(join(root, '.component-old-1'), { recursive: true })
  // 旧组件残留的状态与存储文件（不应存在，但迁移要一并清掉）。
  await writeState(directory, {
    'old-board': { enabled: true, source: 'local', createdAt: 'x', updatedAt: 'x' },
    'native-card': { enabled: false, source: 'template', createdAt: 'x', updatedAt: 'x' },
  })
  await mkdir(join(directory, 'custom-ui-data'), { recursive: true })
  await writeFile(join(directory, 'custom-ui-data', 'old-board.json'), '{"version":1,"values":{}}')
  const nativeBefore = await readFile(join(root, 'native-card', 'component.js'), 'utf8')

  const events = []
  const service = new CustomUiService({ dataDir: directory, now: () => Date.UTC(2026, 9, 31, 8) })
  t.after(() => service.dispose())
  await service.start({ onChange: (id, detail) => events.push({ id, ...detail }) })

  assert.deepEqual((await readdir(root)).sort(), [
    '.component-old-1',
    'UPPER CASE!',
    'native-bad',
    'native-card',
  ])
  assert.equal(await readFile(join(root, 'native-card', 'component.js'), 'utf8'), nativeBefore)
  const trash = (await readdir(join(directory, 'custom-ui-trash'))).sort()
  assert.deepEqual(trash, [
    'bad-json-20261031T080000Z',
    'no-manifest-20261031T080000Z',
    'old-board-20261031T080000Z',
  ])
  // 文件原样在回收站里（回滚旧版本前可以手动移回）；存储文件随组件进回收站。
  const oldTrash = join(directory, 'custom-ui-trash', 'old-board-20261031T080000Z')
  assert.equal(await readFile(join(oldTrash, 'component', 'index.html'), 'utf8'), '<h1>old</h1>')
  assert.equal(await readFile(join(oldTrash, 'component', 'js', 'app.js'), 'utf8'), 'window.x = 1')
  assert.ok(await lstat(join(oldTrash, 'data.json')))
  assert.equal(
    await lstat(join(directory, 'custom-ui-data', 'old-board.json')).catch(() => null),
    null,
  )
  const meta = JSON.parse(await readFile(join(oldTrash, 'meta.json'), 'utf8'))
  assert.equal(meta.reason, 'legacy-retired')
  assert.equal(meta.kind, 'legacy')
  assert.equal(
    await readFile(
      join(directory, 'custom-ui-trash', 'no-manifest-20261031T080000Z', 'component', 'notes.txt'),
      'utf8',
    ),
    'keep me',
  )
  // 状态：旧组件的条目清掉，原生组件的保留。
  const state = JSON.parse(await readFile(join(directory, 'custom-ui-state.json'), 'utf8'))
  assert.deepEqual(Object.keys(state.components), ['native-card'])
  assert.equal(state.components['native-card'].enabled, false)
  // 记录文件只记 id、分类与回收站名字，不记完整路径。
  const record = JSON.parse(await readFile(join(directory, 'custom-ui-migrations.json'), 'utf8'))
  assert.equal(record.legacyRetiredAt, '2026-10-31T08:00:00.000Z')
  assert.deepEqual(record.moved.map((entry) => [entry.id, entry.kind]).sort(), [
    ['bad-json', 'broken'],
    ['no-manifest', 'broken'],
    ['old-board', 'legacy'],
  ])
  assert.doesNotMatch(JSON.stringify(record), new RegExp(directory.replaceAll('\\', '\\\\')))
  assert.deepEqual(events.map((event) => [event.id, event.reason]).sort(), [
    ['bad-json', 'deleted'],
    ['no-manifest', 'deleted'],
    ['old-board', 'deleted'],
  ])
  const catalog = await service.catalog()
  assert.deepEqual(
    catalog.components.map((row) => [row.id, row.kind, row.status]),
    [
      ['native-card', 'native', 'ready'],
      ['native-bad', 'native', 'invalid'],
    ],
  )
})

test('startup migration runs once: restarting does not fail and later broken directories stay visible', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-migrate-once-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const root = join(directory, 'custom-ui')
  await createComponent(root, 'native-card', NATIVE, NATIVE_FILES)
  await createComponent(root, 'old-board', { name: 'Old' }, { 'index.html': '<h1>old</h1>' })
  const first = new CustomUiService({ dataDir: directory })
  await first.start({})
  first.dispose()
  assert.deepEqual(await readdir(root), ['native-card'])
  // 迁移之后手放的坏目录：之后的启动不再自动移走（用户正在编辑的 manifest 可能暂时是坏的）。
  await mkdir(join(root, 'half-written'), { recursive: true })
  for (let round = 0; round < 2; round += 1) {
    const again = new CustomUiService({ dataDir: directory })
    await again.start({})
    assert.equal((await again.retireLegacyComponents()).status, 'skipped')
    assert.deepEqual(
      (await again.catalog()).components.map((row) => [row.id, row.kind]),
      [
        ['half-written', 'broken'],
        ['native-card', 'native'],
      ],
    )
    again.dispose()
  }
  assert.equal((await readdir(join(directory, 'custom-ui-trash'))).length, 1)
})

test('startup migration never overwrites an existing trash entry and retries after a failed move', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-migrate-trash-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const root = join(directory, 'custom-ui')
  const now = () => Date.UTC(2026, 9, 31, 8)
  await createComponent(root, 'old-board', { name: 'Old' }, { 'index.html': '<h1>new</h1>' })
  await createComponent(root, 'old-two', { name: 'Old 2' }, { 'index.html': '<h1>two</h1>' })
  // 回收站里已有同名同秒的条目（例如之前删过一次）：不能覆盖。
  const existing = join(directory, 'custom-ui-trash', 'old-board-20261031T080000Z')
  await mkdir(join(existing, 'component'), { recursive: true })
  await writeFile(join(existing, 'component', 'index.html'), '<h1>earlier</h1>')
  // 第一次启动：old-two 移动失败（被占用），old-board 照常移走；不写记录文件。
  const { rename } = await import('node:fs/promises')
  const flaky = new CustomUiService({
    dataDir: directory,
    now,
    renameDirectory: async (from, to) => {
      if (from.endsWith('old-two')) throw Object.assign(new Error('busy'), { code: 'EBUSY' })
      return rename(from, to)
    },
  })
  await flaky.start({})
  flaky.dispose()
  assert.equal(
    await readFile(join(existing, 'component', 'index.html'), 'utf8'),
    '<h1>earlier</h1>',
  )
  assert.equal(
    await readFile(
      join(directory, 'custom-ui-trash', 'old-board-20261031T080000Z-2', 'component', 'index.html'),
      'utf8',
    ),
    '<h1>new</h1>',
  )
  assert.deepEqual(await readdir(root), ['old-two'])
  assert.equal(await lstat(join(directory, 'custom-ui-migrations.json')).catch(() => null), null)
  // 第二次启动重试：只移剩下的旧组件。
  const retry = new CustomUiService({ dataDir: directory, now })
  await retry.start({})
  retry.dispose()
  assert.deepEqual(await readdir(root), [])
  assert.deepEqual((await readdir(join(directory, 'custom-ui-trash'))).sort(), [
    'old-board-20261031T080000Z',
    'old-board-20261031T080000Z-2',
    'old-two-20261031T080000Z',
  ])
  assert.ok(await lstat(join(directory, 'custom-ui-migrations.json')))
})
