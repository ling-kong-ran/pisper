// 原生组件的放置实例与启用状态：数量规则、组件前提、并发写串行、损坏文件、修订冲突与严格 JSON。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { CustomUiInstanceStore } from '../services/custom-ui-instances.mjs'
import { CustomUiService } from '../services/custom-ui-service.mjs'
import { CustomUiStateStore } from '../services/custom-ui-state.mjs'

async function fixture(t) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-instances-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  let counter = 0
  const service = new CustomUiService({
    dataDir,
    randomId: () => `00000000-0000-4000-8000-${String(++counter).padStart(12, '0')}`,
  })
  const events = []
  await service.start({
    onChange: (id, detail) => events.push({ id, ...detail }),
    hostVersion: '0.6.5',
  })
  return { dataDir, service, events }
}

async function writeComponent(
  root,
  id,
  manifest,
  files = { 'component.js': 'export default () => null' },
) {
  const dir = join(root, id)
  await mkdir(dir, { recursive: true })
  await writeFile(
    join(dir, 'manifest.json'),
    typeof manifest === 'string' ? manifest : JSON.stringify(manifest),
  )
  for (const [path, content] of Object.entries(files)) await writeFile(join(dir, path), content)
}

const rejects = (promise, code) =>
  assert.rejects(promise, (error) => {
    assert.equal(error.code, code)
    return true
  })

test('overview-card 每个组件可以多个实例，其余放置每个组件各 1 个', async (t) => {
  const { service, events } = await fixture(t)
  await service.createFromTemplate({ template: 'notes' })
  const first = await service.instances.create({ componentId: 'notes', surface: 'overview-card' })
  assert.equal(first.revision, 1)
  assert.equal(first.instance.size, 'M')
  assert.equal(first.instance.scope, null)
  assert.equal(first.instance.enabled, true)
  const second = await service.instances.create({
    componentId: 'notes',
    surface: 'overview-card',
    title: '第二张',
  })
  assert.equal(second.instance.order, 1)
  const panel = await service.instances.create({ componentId: 'notes', surface: 'session-panel' })
  assert.deepEqual(panel.instance.scope, { type: 'all' })
  assert.equal(panel.instance.size, null)
  await rejects(
    service.instances.create({ componentId: 'notes', surface: 'session-panel' }),
    'component_instance_exists',
  )
  await service.instances.create({ componentId: 'notes', surface: 'feature-page' })
  await rejects(
    service.instances.create({ componentId: 'notes', surface: 'feature-page' }),
    'component_instance_exists',
  )
  const listed = await service.instances.list('overview-card')
  assert.equal(listed.revision, 4)
  assert.deepEqual(
    listed.instances.map((instance) => instance.title),
    ['', '第二张'],
  )
  assert.equal((await service.instances.list()).instances.length, 4)
  assert.ok(
    events.filter((event) => event.reason === 'instances').every((event) => event.id === 'notes'),
  )
  // 模板创建不写实例：没有默认放置。
  await service.createFromTemplate({ template: 'usage-today' })
  assert.equal((await service.instances.list()).instances.length, 4)
})

test('创建实例的组件前提：存在、原生、可运行、已启用、声明了该放置', async (t) => {
  const { service } = await fixture(t)
  await service.createFromTemplate({ template: 'usage-today' })
  await writeComponent(
    service.root,
    'old',
    { name: 'Old', entry: 'index.html' },
    { 'index.html': '<p>' },
  )
  await writeComponent(service.root, 'bad', { runtime: 'native', name: '' })
  await rejects(
    service.instances.create({ componentId: 'missing', surface: 'floating' }),
    'component_not_found',
  )
  await rejects(
    service.instances.create({ componentId: 'old', surface: 'floating' }),
    'component_legacy_unsupported',
  )
  await rejects(
    service.instances.create({ componentId: 'bad', surface: 'floating' }),
    'component_invalid',
  )
  // usage-today 没有声明 session-panel。
  await rejects(
    service.instances.create({ componentId: 'usage-today', surface: 'session-panel' }),
    'component_surface_unsupported',
  )
  await rejects(
    service.instances.create({ componentId: 'usage-today', surface: 'board-card' }),
    'component_surface_invalid',
  )
  await rejects(
    service.instances.create({ componentId: 'usage-today', surface: 'overview-card', size: 'L' }),
    'component_size_invalid',
  )
  await rejects(
    service.instances.create({ componentId: 'usage-today', surface: 'floating', size: 'S' }),
    'component_size_invalid',
  )
  await rejects(
    service.instances.create({
      componentId: 'usage-today',
      surface: 'floating',
      scope: { type: 'all' },
    }),
    'component_scope_invalid',
  )
  await rejects(
    service.instances.create({
      componentId: 'usage-today',
      surface: 'floating',
      title: 'x'.repeat(81),
    }),
    'component_title_invalid',
  )
  await rejects(
    service.instances.create({ componentId: 'usage-today', surface: 'floating', extra: true }),
    'component_request_invalid',
  )
  await service.setEnabled('usage-today', { enabled: false })
  await rejects(
    service.instances.create({ componentId: 'usage-today', surface: 'floating' }),
    'component_disabled',
  )
})

test('属性按 manifest schema 校验，错误定位到字段；修改实例不改放置', async (t) => {
  const { service } = await fixture(t)
  await writeComponent(service.root, 'ci', {
    runtime: 'native',
    name: 'CI',
    props: {
      properties: {
        repo: { type: 'string', maxLength: 40 },
        limit: { type: 'integer', minimum: 1, maximum: 10 },
      },
    },
    placements: [{ surface: 'overview-card', sizes: ['S', 'M'] }, 'session-panel'],
  })
  await assert.rejects(
    service.instances.create({
      componentId: 'ci',
      surface: 'overview-card',
      props: { repo: 1, x: 2 },
    }),
    (error) => {
      assert.equal(error.code, 'component_props_invalid')
      assert.deepEqual(
        error.details.errors.map((item) => `${item.path}:${item.code}`),
        ['props.repo:props_type_invalid', 'props.x:props_unknown'],
      )
      return true
    },
  )
  const { instance } = await service.instances.create({
    componentId: 'ci',
    surface: 'overview-card',
    props: { repo: 'pisper' },
  })
  assert.equal(instance.size, 'S')
  const patched = await service.instances.update(instance.instanceId, {
    size: 'M',
    props: { repo: 'other', limit: 3 },
    title: '  CI · other ',
  })
  assert.equal(patched.instance.size, 'M')
  assert.equal(patched.instance.title, 'CI · other')
  assert.deepEqual(patched.instance.props, { repo: 'other', limit: 3 })
  assert.equal(patched.instance.surface, 'overview-card')
  await rejects(
    service.instances.update(instance.instanceId, { props: { limit: 11 } }),
    'component_props_invalid',
  )
  await rejects(
    service.instances.update(instance.instanceId, { surface: 'floating' }),
    'component_request_invalid',
  )
  await rejects(service.instances.update(instance.instanceId, {}), 'component_request_invalid')
  await rejects(
    service.instances.update('nope', { enabled: false }),
    'component_instance_not_found',
  )
  // 停用组件后仍可改实例属性与启用状态（工作室里停用后调参数）。
  await service.setEnabled('ci', { enabled: false })
  assert.equal(
    (await service.instances.update(instance.instanceId, { props: { repo: 'x' } })).instance.props
      .repo,
    'x',
  )
  assert.equal(
    (await service.instances.update(instance.instanceId, { enabled: false })).instance.enabled,
    false,
  )

  const panel = await service.instances
    .create({ componentId: 'ci', surface: 'session-panel' })
    .catch((error) => error)
  assert.equal(panel.code, 'component_disabled')
  await service.setEnabled('ci', { enabled: true })
  const scoped = await service.instances.create({
    componentId: 'ci',
    surface: 'session-panel',
    scope: { type: 'project', cwd: '/work/pisper' },
  })
  assert.deepEqual(scoped.instance.scope, { type: 'project', cwd: '/work/pisper' })
  await rejects(
    service.instances.update(scoped.instance.instanceId, {
      scope: { type: 'project', cwd: 'relative' },
    }),
    'component_scope_invalid',
  )
  assert.deepEqual(
    (await service.instances.update(scoped.instance.instanceId, { scope: null })).instance.scope,
    { type: 'all' },
  )
  const removed = await service.instances.remove(scoped.instance.instanceId)
  assert.equal(typeof removed.revision, 'number')
  await rejects(
    service.instances.remove(scoped.instance.instanceId),
    'component_instance_not_found',
  )
})

test('重排：baseRevision 不同就是冲突，order 必须是该放置全部实例的排列', async (t) => {
  const { service } = await fixture(t)
  await service.createFromTemplate({ template: 'usage-today' })
  const a = await service.instances.create({
    componentId: 'usage-today',
    surface: 'overview-card',
    title: 'A',
  })
  const b = await service.instances.create({
    componentId: 'usage-today',
    surface: 'overview-card',
    title: 'B',
  })
  await service.instances.create({ componentId: 'usage-today', surface: 'floating' })
  const { revision } = await service.instances.list()
  await rejects(
    service.instances.reorder({
      baseRevision: revision - 1,
      surface: 'overview-card',
      order: [b.instance.instanceId, a.instance.instanceId],
    }),
    'component_instances_conflict',
  )
  await rejects(
    service.instances.reorder({
      baseRevision: revision,
      surface: 'overview-card',
      order: [b.instance.instanceId],
    }),
    'component_instances_order_invalid',
  )
  const reordered = await service.instances.reorder({
    baseRevision: revision,
    surface: 'overview-card',
    order: [b.instance.instanceId, a.instance.instanceId],
  })
  assert.equal(reordered.revision, revision + 1)
  assert.deepEqual(
    reordered.instances.map((instance) => instance.title),
    ['B', 'A'],
  )
  await rejects(
    service.instances.reorder({ baseRevision: 'x', surface: 'overview-card', order: [] }),
    'component_request_invalid',
  )
})

test('并发写按文件串行：同一毫秒的多次创建全部落盘，修订号连续', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-serial-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const path = join(dataDir, 'custom-ui-instances.json')
  let counter = 0
  // 固定时钟：writeJsonAtomic 的临时文件名只精确到毫秒，不串行就会撞名。
  const store = new CustomUiInstanceStore({
    path,
    now: () => 1_000,
    randomId: () => `id-${++counter}`,
  })
  const results = await Promise.all(
    Array.from({ length: 20 }, (_, index) =>
      store.create({
        componentId: `c${index}`,
        surface: 'floating',
        size: null,
        scope: null,
        title: '',
        props: {},
      }),
    ),
  )
  assert.deepEqual(
    results.map((result) => result.revision).sort((left, right) => left - right),
    Array.from({ length: 20 }, (_, index) => index + 1),
  )
  const disk = JSON.parse(await readFile(path, 'utf8'))
  assert.equal(disk.revision, 20)
  assert.equal(disk.instances.length, 20)
  assert.deepEqual(
    (await readdir(dataDir)).filter((name) => name.endsWith('.tmp')),
    [],
  )
  const state = new CustomUiStateStore({
    path: join(dataDir, 'custom-ui-state.json'),
    now: () => 1_000,
  })
  await Promise.all(
    Array.from({ length: 10 }, (_, index) =>
      state.update(`c${index}`, { enabled: index % 2 === 0 }),
    ),
  )
  const saved = JSON.parse(await readFile(join(dataDir, 'custom-ui-state.json'), 'utf8'))
  assert.equal(Object.keys(saved.components).length, 10)
  assert.equal(saved.components.c1.enabled, false)
  assert.equal(saved.components.c1.source, 'local')
})

test('损坏的实例与状态文件先改名为 .corrupt-<时间> 再从空开始，不静默覆盖', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-corrupt-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  await writeFile(join(dataDir, 'custom-ui-instances.json'), '{"version":1,"instances":[')
  await writeFile(join(dataDir, 'custom-ui-state.json'), '{"version":9}')
  const service = new CustomUiService({ dataDir, now: () => Date.UTC(2026, 9, 31, 1, 2, 3) })
  assert.deepEqual(await service.instances.list(), { revision: 0, instances: [] })
  assert.equal(await service.state.get('x'), null)
  const names = (await readdir(dataDir)).sort()
  assert.ok(names.includes('custom-ui-instances.json.corrupt-20261031T010203Z'), names.join(','))
  assert.ok(names.includes('custom-ui-state.json.corrupt-20261031T010203Z'), names.join(','))
  assert.equal(
    await readFile(join(dataDir, 'custom-ui-instances.json.corrupt-20261031T010203Z'), 'utf8'),
    '{"version":1,"instances":[',
  )
  // 单条坏记录只丢这一条，不把整份文件当成损坏。
  await writeFile(
    join(dataDir, 'custom-ui-instances.json'),
    JSON.stringify({
      version: 1,
      revision: 7,
      instances: [
        { instanceId: 'ok-1', componentId: 'notes', surface: 'floating' },
        { instanceId: 'bad id', componentId: 'notes', surface: 'floating' },
        { instanceId: 'ok-2', componentId: 'notes', surface: 'board-card' },
      ],
    }),
  )
  const fresh = new CustomUiInstanceStore({ path: join(dataDir, 'custom-ui-instances.json') })
  const listed = await fresh.list()
  assert.equal(listed.revision, 7)
  assert.deepEqual(
    listed.instances.map((instance) => instance.instanceId),
    ['ok-1'],
  )
})

test('实例标题、范围与属性落盘前替换孤立代理项，文件是严格 JSON', async (t) => {
  const { dataDir, service } = await fixture(t)
  await writeComponent(service.root, 'text', {
    runtime: 'native',
    name: 'Text',
    props: { properties: { body: { type: 'string' } } },
  })
  const { instance } = await service.instances.create({
    componentId: 'text',
    surface: 'session-panel',
    title: 'a\ud800',
    scope: { type: 'project', cwd: '/w/\udc00' },
    props: { body: 'x\ud83d' },
  })
  assert.equal(instance.title, 'a�')
  assert.equal(instance.scope.cwd, '/w/�')
  assert.equal(instance.props.body, 'x�')
  const text = await readFile(join(dataDir, 'custom-ui-instances.json'), 'utf8')
  assert.doesNotMatch(text, /\\ud[89a-f][0-9a-f]{2}/i)
  assert.doesNotThrow(() => JSON.parse(text))
})

test('启用状态：手放组件默认启用；停用写入 source:local；旧版与损坏目录不能切换', async (t) => {
  const { service, events } = await fixture(t)
  await writeComponent(service.root, 'hand', { runtime: 'native', name: 'Hand' })
  await writeComponent(service.root, 'old', { name: 'Old' })
  await mkdir(join(service.root, 'empty'))
  const row = (await service.catalog()).components.find((component) => component.id === 'hand')
  assert.equal(row.enabled, true)
  assert.equal(row.source, 'local')
  const { component } = await service.setEnabled('hand', { enabled: false })
  assert.equal(component.enabled, false)
  assert.equal((await service.state.get('hand')).source, 'local')
  assert.deepEqual(events.at(-1), { id: 'hand', rev: component.rev, reason: 'state' })
  await rejects(service.setEnabled('old', { enabled: true }), 'component_legacy_unsupported')
  await rejects(service.setEnabled('empty', { enabled: true }), 'component_invalid')
  await rejects(service.setEnabled('missing', { enabled: true }), 'component_not_found')
  await rejects(service.setEnabled('hand', { enabled: 'yes' }), 'component_request_invalid')
  await rejects(
    service.setEnabled('hand', { enabled: true, extra: 1 }),
    'component_request_invalid',
  )
})
