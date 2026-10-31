// component_create：注册（目录、预设、能力、工厂）、执行模式与审批（跟随会话权限模式，不算 critical），
// 以及经真实 CustomUiService 的 create / update、保留 id、旧版占用、裸导入、manifest 错误、快照上限与事件。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { filterToolsForExecutionMode } from '../security/execution-mode.mjs'
import { permissionRequirement } from '../services/session-permission-service.mjs'
import { CustomUiService } from '../services/custom-ui-service.mjs'
import { createComponentCreateTool, manifest } from '../tools/app/component-create.mjs'
import { TOOL_CATALOG, TOOL_PRESETS, createAppTools } from '../tools/registry.mjs'

const MANIFEST = {
  runtime: 'native',
  name: 'CI',
  version: '1.0.0',
  placements: [{ surface: 'overview-card', sizes: ['S'] }],
}

async function fixture(t) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-component-create-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const service = new CustomUiService({ dataDir })
  const events = []
  await service.start({ onChange: (id, detail) => events.push({ id, ...detail }) })
  t.after(() => service.dispose())
  const calls = []
  const runtime = service.agentInterface()
  const tool = createComponentCreateTool({
    componentRuntime: {
      writeComponent: (input, options) => {
        calls.push(options)
        return runtime.writeComponent(input, options)
      },
      describe: runtime.describe,
    },
    sessionId: 'session-7',
  })
  return { dataDir, service, events, tool, calls }
}

const rejectsText = (promise, pattern) =>
  assert.rejects(promise, (error) => {
    assert.match(error.message, pattern)
    return true
  })

test('component_create 是登记过的高风险 app 工具：目录、预设、工厂与提示都在', () => {
  assert.equal(manifest.risk, 'high')
  assert.equal(manifest.source, 'app')
  assert.ok(TOOL_CATALOG.some((tool) => tool.id === 'component_create'))
  assert.equal(TOOL_PRESETS.full.includes('component_create'), true)
  const [tool] = createAppTools({ enabledTools: ['component_create'] })
  assert.equal(tool?.name, 'component_create')
  const guidelines = tool.promptGuidelines.join('\n')
  // 非工作室会话的精简契约：约 1.2 KB 以内，提到技能入口、禁止裸导入与网络规则。
  assert.ok(Buffer.byteLength(guidelines) <= 1600, `guidelines ${Buffer.byteLength(guidelines)} B`)
  assert.match(guidelines, /\/skill:pisper-component/)
  assert.match(guidelines, /No bare imports/)
  assert.match(guidelines, /https/)
})

test('执行模式：审批模式可见且每次审批；工作区写与完全访问直接放行；不按“工作区之外”阻断', () => {
  for (const mode of ['approval-required', 'workspace-write', 'full-access'])
    assert.deepEqual(filterToolsForExecutionMode(['component_create'], mode), ['component_create'])
  const base = { cwd: '/work/project', toolName: 'component_create', args: { id: 'ci-status' } }
  const ask = permissionRequirement({ ...base, mode: 'ask', executionMode: 'approval-required' })
  assert.equal(ask.block, undefined)
  assert.equal(ask.risk, 'high')
  assert.equal(
    ask.reason,
    'component_create 将写入组件 ci-status（数据目录/custom-ui/ci-status），需要确认后执行。',
  )
  assert.equal(
    permissionRequirement({ ...base, mode: 'auto', executionMode: 'workspace-write' }),
    null,
  )
  assert.equal(
    permissionRequirement({ ...base, mode: 'ignore', executionMode: 'full-access' }),
    null,
  )
  // id 不合法时理由里不回显原文。
  const odd = permissionRequirement({
    ...base,
    args: { id: '../../etc' },
    mode: 'ask',
    executionMode: 'approval-required',
  })
  assert.doesNotMatch(odd.reason, /etc/)
})

test('create 再 update：写入 agent 来源与会话，update 前快照，返回改动统计并发事件', async (t) => {
  const { dataDir, service, events, tool, calls } = await fixture(t)
  const created = await tool.execute('call-1', {
    mode: 'create',
    id: 'ci',
    manifest: MANIFEST,
    files: [{ path: 'component.js', content: 'export default () => null\n' }],
  })
  assert.match(
    created.content[0].text,
    /Created component "ci" v1\.0\.0 \(status ready, rev [a-f0-9]{24}\)/,
  )
  assert.match(created.content[0].text, /not placed anywhere/)
  assert.equal(created.details.mode, 'create')
  assert.equal(created.details.snapshot, null)
  assert.deepEqual(calls[0], { source: 'agent', sessionId: 'session-7' })
  const state = await service.state.get('ci')
  assert.equal(state.source, 'agent')
  assert.equal(state.enabled, true)
  assert.deepEqual(events.at(-1), { id: 'ci', rev: created.details.rev, reason: 'created' })
  // 没有默认放置。
  assert.deepEqual((await service.instances.list()).instances, [])

  const updated = await tool.execute('call-2', {
    mode: 'update',
    id: 'ci',
    files: [
      { path: 'component.js', content: "import { a } from './lib.js'\nexport default () => a\n" },
      { path: 'lib.js', content: 'export const a = 1\n' },
    ],
    summary: '拆出 lib',
  })
  assert.equal(updated.details.snapshot, 1)
  assert.deepEqual(
    updated.details.files.map((file) => file.path),
    ['component.js', 'lib.js'],
  )
  assert.match(updated.content[0].text, /Previous version saved as snapshot 1/)
  assert.deepEqual(events.at(-1), { id: 'ci', rev: updated.details.rev, reason: 'files' })
  const snapshot = join(dataDir, 'custom-ui-versions', 'ci', '1', 'component.js')
  assert.equal(await readFile(snapshot, 'utf8'), 'export default () => null\n')
})

test('拒绝并返回结构化清单：保留 id、旧版占用、已存在、不存在、裸导入、manifest 错误、HTML、entry 缺失', async (t) => {
  const { service, tool } = await fixture(t)
  const file = { path: 'component.js', content: 'export default () => null\n' }
  await rejectsText(
    tool.execute('c', { mode: 'create', id: 'pisper-island', manifest: MANIFEST, files: [file] }),
    /component_id_reserved/,
  )
  await mkdir(join(service.root, 'legacy'), { recursive: true })
  await writeFile(
    join(service.root, 'legacy', 'manifest.json'),
    '{"name":"Old","entry":"index.html"}',
  )
  await rejectsText(
    tool.execute('c', { mode: 'create', id: 'legacy', manifest: MANIFEST, files: [file] }),
    /component_id_taken_by_legacy/,
  )
  await rejectsText(
    tool.execute('c', { mode: 'update', id: 'legacy', files: [file] }),
    /component_id_taken_by_legacy/,
  )
  await rejectsText(
    tool.execute('c', { mode: 'update', id: 'ghost', files: [file] }),
    /component_not_found/,
  )
  await tool.execute('c', { mode: 'create', id: 'ci', manifest: MANIFEST, files: [file] })
  await rejectsText(
    tool.execute('c', { mode: 'create', id: 'ci', manifest: MANIFEST, files: [file] }),
    /component_already_installed/,
  )
  await assert.rejects(
    tool.execute('c', {
      mode: 'update',
      id: 'ci',
      files: [
        {
          path: 'component.js',
          content:
            "import React from 'react'\nimport x from 'https://cdn.example.com/x.js'\nexport default () => import('lodash')\n",
        },
      ],
    }),
    (error) => {
      assert.match(error.message, /^component_create failed: component_files_invalid/)
      const listed = JSON.parse(error.message.slice(error.message.indexOf('\n') + 1))
      assert.deepEqual(
        listed.map((item) => [item.code, item.params?.specifier]),
        [
          ['component_bare_import', 'react'],
          ['component_bare_import', 'https://cdn.example.com/x.js'],
          ['component_bare_import', 'lodash'],
        ],
      )
      return true
    },
  )
  await assert.rejects(
    tool.execute('c', {
      mode: 'create',
      id: 'bad-manifest',
      manifest: { runtime: 'native', name: '', placements: [{ surface: 'board-card' }] },
      files: [file],
    }),
    (error) => {
      assert.match(error.message, /component_manifest_invalid/)
      assert.match(error.message, /placement_surface_invalid/)
      return true
    },
  )
  await rejectsText(
    tool.execute('c', { mode: 'create', id: 'no-manifest', files: [file] }),
    /manifest_missing/,
  )
  await rejectsText(
    tool.execute('c', {
      mode: 'create',
      id: 'page',
      manifest: MANIFEST,
      files: [file, { path: 'index.html', content: '<p>' }],
    }),
    /component_file_path_invalid/,
  )
  await rejectsText(
    tool.execute('c', { mode: 'create', id: 'no-entry', manifest: MANIFEST, files: [] }),
    /entry_missing/,
  )
  // 被拒的写入不会留下目录。
  assert.deepEqual((await readdir(service.root)).sort(), ['ci', 'legacy'])
})

test('快照只保留 20 份', async (t) => {
  const { dataDir, tool } = await fixture(t)
  await tool.execute('c', {
    mode: 'create',
    id: 'ci',
    manifest: MANIFEST,
    files: [{ path: 'component.js', content: 'export default 0' }],
  })
  for (let index = 1; index <= 21; index += 1)
    await tool.execute('c', {
      mode: 'update',
      id: 'ci',
      files: [{ path: 'component.js', content: `export default ${index}` }],
    })
  const base = join(dataDir, 'custom-ui-versions', 'ci')
  assert.equal((await readdir(base)).filter((name) => name !== 'index.json').length, 20)
})

test('没有注入组件运行时：明确报未初始化，不伪装成功', async () => {
  const tool = createComponentCreateTool({})
  await rejectsText(
    tool.execute('c', { mode: 'create', id: 'ci', manifest: MANIFEST, files: [] }),
    /not initialized/,
  )
})
