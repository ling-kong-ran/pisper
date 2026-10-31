// 原生组件写入器（模板创建与 S2 的 component_create 共用）：暂存 + rename 交换、版本快照上限、
// 结构化校验清单（裸导入、HTML、entry 缺失、保留 id、旧版占用）、内容哈希与模板源码可解析。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { pathToFileURL } from 'node:url'
import { ComponentHasher } from '../services/custom-ui-files.mjs'
import { CustomUiService, findBareImports } from '../services/custom-ui-service.mjs'
import { buildComponentTemplate, COMPONENT_TEMPLATES } from '../services/custom-ui-templates.mjs'

async function fixture(t) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-writer-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const service = new CustomUiService({ dataDir })
  const events = []
  await service.start({ onChange: (id, detail) => events.push({ id, ...detail }) })
  return { dataDir, service, events, agent: service.agentInterface() }
}

const manifest = { runtime: 'native', name: 'CI', version: '1.0.0' }
const rejectsWith = (promise, code, errorCodes) =>
  assert.rejects(promise, (error) => {
    assert.equal(error.code, code)
    if (errorCodes)
      assert.deepEqual(
        error.details.errors.map((item) => item.code),
        errorCodes,
      )
    return true
  })

test('create 与 update：update 前快照，交换后没有暂存目录残留，返回改动统计', async (t) => {
  const { dataDir, service, events, agent } = await fixture(t)
  const created = await agent.writeComponent(
    {
      mode: 'create',
      id: 'ci',
      manifest,
      files: [
        {
          path: 'component.js',
          content: "import { a } from './lib/a.js'\nexport default () => a\n",
        },
        { path: 'lib/a.js', content: 'export const a = 1\n' },
      ],
    },
    { source: 'agent' },
  )
  assert.equal(created.mode, 'create')
  assert.equal(created.status, 'ready')
  assert.match(created.rev, /^[a-f0-9]{24}$/)
  assert.equal(created.snapshot, null)
  assert.deepEqual(events.at(-1), { id: 'ci', rev: created.rev, reason: 'created' })
  assert.equal((await service.state.get('ci')).source, 'agent')

  const updated = await agent.writeComponent({
    mode: 'update',
    id: 'ci',
    files: [{ path: 'component.js', content: 'export default () => 2\n' }],
    deleteFiles: ['lib/a.js'],
    summary: '简化',
  })
  assert.equal(updated.snapshot, 1)
  assert.notEqual(updated.rev, created.rev)
  assert.deepEqual(updated.files, [{ path: 'component.js', bytes: 23, added: 1, removed: 2 }])
  assert.deepEqual(updated.deleted, ['lib/a.js'])
  assert.deepEqual(events.at(-1), { id: 'ci', rev: updated.rev, reason: 'files' })
  assert.deepEqual((await readdir(join(service.root, 'ci'))).sort(), [
    'component.js',
    'manifest.json',
  ])
  assert.deepEqual(
    (await readdir(service.root)).filter((name) => name.startsWith('.')),
    [],
  )
  // 快照在 custom-ui/ 之外，含写入前的完整内容。
  const snapshot = join(dataDir, 'custom-ui-versions', 'ci', '1')
  assert.match(await readFile(join(snapshot, 'lib', 'a.js'), 'utf8'), /a = 1/)
  const index = JSON.parse(
    await readFile(join(dataDir, 'custom-ui-versions', 'ci', 'index.json'), 'utf8'),
  )
  assert.deepEqual(
    index.snapshots.map((item) => [item.n, item.summary]),
    [[1, '简化']],
  )
})

test('快照最多保留 20 份，删掉最旧的', async (t) => {
  const { dataDir, agent } = await fixture(t)
  await agent.writeComponent({
    mode: 'create',
    id: 'ci',
    manifest,
    files: [{ path: 'component.js', content: 'export default 0' }],
  })
  for (let index = 1; index <= 22; index += 1) {
    await agent.writeComponent({
      mode: 'update',
      id: 'ci',
      files: [{ path: 'component.js', content: `export default ${index}` }],
    })
  }
  const base = join(dataDir, 'custom-ui-versions', 'ci')
  const index = JSON.parse(await readFile(join(base, 'index.json'), 'utf8'))
  assert.deepEqual(
    index.snapshots.map((item) => item.n),
    Array.from({ length: 20 }, (_, position) => position + 3),
  )
  const dirs = (await readdir(base)).filter((name) => name !== 'index.json')
  assert.equal(dirs.length, 20)
  assert.equal(await readFile(join(base, '3', 'component.js'), 'utf8'), 'export default 2')
})

test('校验清单：裸导入、HTML、entry 缺失、非法路径与 manifest 错误逐项返回', async (t) => {
  const { service, agent } = await fixture(t)
  await rejectsWith(
    agent.writeComponent({
      mode: 'create',
      id: 'ci',
      manifest,
      files: [
        {
          path: 'component.js',
          content: "import React from 'react'\nconst x = await import('lodash')",
        },
        { path: '../escape.js', content: '' },
        { path: 'page.html', content: '<p>' },
        { path: 'manifest.json', content: '{}' },
      ],
    }),
    'component_files_invalid',
    [
      'component_bare_import',
      'component_bare_import',
      'component_file_path_invalid',
      'component_file_path_invalid',
      'component_file_path_invalid',
    ],
  )
  await rejectsWith(
    agent.writeComponent({
      mode: 'create',
      id: 'ci',
      manifest,
      files: [{ path: 'other.js', content: '' }],
    }),
    'component_files_invalid',
    ['entry_missing'],
  )
  await rejectsWith(
    agent.writeComponent({
      mode: 'create',
      id: 'ci',
      manifest: { ...manifest, placements: ['board-card'] },
      files: [{ path: 'component.js', content: '' }],
    }),
    'component_manifest_invalid',
    ['placement_surface_invalid'],
  )
  await rejectsWith(
    agent.writeComponent({ mode: 'create', id: 'ci', files: [] }),
    'component_manifest_invalid',
  )
  await rejectsWith(
    agent.writeComponent({ mode: 'create', id: 'pisper-island', manifest, files: [] }),
    'component_id_reserved',
  )
  await rejectsWith(
    agent.writeComponent({ mode: 'create', id: 'Bad', manifest, files: [] }),
    'component_id_invalid',
  )
  await rejectsWith(
    agent.writeComponent({ mode: 'update', id: 'missing', files: [] }),
    'component_not_found',
  )
  // 旧版目录占着这个 id：不覆盖用户文件。
  await mkdir(join(service.root, 'old'), { recursive: true })
  await writeFile(join(service.root, 'old', 'manifest.json'), JSON.stringify({ name: 'Old' }))
  await rejectsWith(
    agent.writeComponent({
      mode: 'create',
      id: 'old',
      manifest,
      files: [{ path: 'component.js', content: '' }],
    }),
    'component_id_taken_by_legacy',
  )
  await rejectsWith(
    agent.writeComponent({
      mode: 'update',
      id: 'old',
      manifest,
      files: [{ path: 'component.js', content: '' }],
    }),
    'component_id_taken_by_legacy',
  )
  assert.deepEqual(await readdir(join(service.root, 'old')), ['manifest.json'])
  await agent.writeComponent({
    mode: 'create',
    id: 'ci',
    manifest,
    files: [{ path: 'component.js', content: '' }],
  })
  await rejectsWith(
    agent.writeComponent({
      mode: 'create',
      id: 'ci',
      manifest,
      files: [{ path: 'component.js', content: '' }],
    }),
    'component_already_installed',
  )
  // 手放的 HTML 留在目录里时，update 也不能让它成为原生组件的一部分。
  await writeFile(join(service.root, 'ci', 'index.html'), '<p>')
  await rejectsWith(
    agent.writeComponent({
      mode: 'update',
      id: 'ci',
      files: [{ path: 'component.js', content: 'export default 1' }],
    }),
    'component_files_invalid',
    ['component_html_unsupported'],
  )
})

test('裸导入检测只放行 ./、../ 与 / 开头的说明符', () => {
  assert.deepEqual(
    findBareImports(
      [
        "import a from './a.js'",
        "import b from '../b.js'",
        "import '/api/x.js'",
        "import { c } from 'react'",
        "export * from 'lodash/fp'",
        "export { d } from './d.js'",
        "const e = await import('https://cdn.example.com/e.js')",
        "const f = await import('./f.js')",
        'const text = \'import x from "y"\'',
        'export default function Component() {}',
      ].join('\n'),
    ),
    ['react', 'lodash/fp', 'https://cdn.example.com/e.js'],
  )
})

test('内容哈希按文件缓存，未变化时 rev 稳定，点开头的条目不参与', async (t) => {
  const { service } = await fixture(t)
  const dir = join(service.root, 'demo')
  await mkdir(join(dir, '.git'), { recursive: true })
  await writeFile(join(dir, 'manifest.json'), JSON.stringify(manifest))
  await writeFile(join(dir, 'component.js'), 'export default 1')
  const hasher = new ComponentHasher()
  const first = await hasher.compute(dir)
  assert.equal((await hasher.compute(dir)).rev, first.rev)
  await writeFile(join(dir, '.git', 'HEAD'), 'ref')
  await writeFile(join(dir, '.DS_Store'), 'x')
  assert.equal((await hasher.compute(dir)).rev, first.rev)
  await writeFile(join(dir, 'manifest.json'), JSON.stringify({ ...manifest, version: '1.0.1' }))
  assert.notEqual((await hasher.compute(dir)).rev, first.rev)
  await mkdir(join(dir, 'a', 'b', 'c', 'd', 'e'), { recursive: true })
  await writeFile(join(dir, 'a', 'b', 'c', 'd', 'e', 'deep.js'), '')
  assert.equal((await hasher.compute(dir)).error, 'component_too_large')
})

test('三个模板生成可被 ESM 解析的源码与合法 manifest，名称不会被当成代码', async (t) => {
  const { dataDir, service } = await fixture(t)
  for (const template of COMPONENT_TEMPLATES) {
    const { files } = buildComponentTemplate(template, {
      name: template === 'blank' ? 'x" ${y} `z`' : '',
    })
    const file = join(dataDir, `${template}.mjs`)
    await writeFile(file, files[0].content)
    const module = await import(pathToFileURL(file).href)
    assert.equal(typeof module.default, 'function', template)
    const { component } = await service.createFromTemplate({ template })
    assert.equal(component.status, 'ready', template)
    assert.deepEqual(component.errors, [])
  }
})
