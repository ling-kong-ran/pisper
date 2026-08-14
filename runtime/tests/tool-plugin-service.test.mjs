import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { ToolPluginService } from '../services/tool-plugin-service.mjs'

async function createFixture(root, overrides = {}) {
  const source = join(root, 'source')
  await mkdir(source, { recursive: true })
  const manifest = {
    schemaVersion: 1,
    id: 'example.echo',
    name: 'Echo plugin',
    version: '1.0.0',
    description: 'Returns the supplied text.',
    entry: 'index.mjs',
    permissions: ['workspace-read'],
    tools: [
      {
        name: 'example_echo',
        label: 'Echo',
        description: 'Return the supplied text.',
        parameters: {
          type: 'object',
          properties: { text: { type: 'string' } },
          required: ['text'],
        },
      },
    ],
    ...overrides,
  }
  await writeFile(join(source, 'pisper-plugin.json'), JSON.stringify(manifest), 'utf8')
  await writeFile(
    join(source, 'index.mjs'),
    "export async function execute({ toolName, arguments: input, context }) { return { content: [{ type: 'text', text: `${toolName}:${input.text}:${Boolean(context.cwd)}:${process.env.PISPER_SECRET || 'clean'}` }], details: {} } }\n",
    'utf8',
  )
  return source
}

async function withService(run) {
  const root = await mkdtemp(join(os.tmpdir(), 'pisper-plugin-test-'))
  const dataDir = join(root, 'agent')
  const configPath = join(dataDir, 'pisper.json')
  await mkdir(dataDir, { recursive: true })
  await writeFile(configPath, JSON.stringify({ enabledTools: ['read'] }), 'utf8')
  const service = new ToolPluginService(configPath, { dataDir })
  await service.init()
  try {
    await run({ root, dataDir, configPath, service })
  } finally {
    service.dispose()
    await rm(root, { recursive: true, force: true })
  }
}

test('shipped project package info example installs and reads the active workspace', async () => {
  await withService(async ({ service }) => {
    const repositoryRoot = fileURLToPath(new URL('../..', import.meta.url))
    const source = join(repositoryRoot, 'examples', 'local-plugins', 'project-package-info')
    const inspection = await service.inspect(source)
    assert.equal(inspection.plugin.id, 'example.project-package-info')
    assert.deepEqual(
      inspection.plugin.capabilities.map((tool) => tool.name),
      ['project_package_info'],
    )

    await service.install(inspection.inspectionId)
    const [definition] = service.createToolDefinitions({
      cwd: repositoryRoot,
      sessionId: 'example-session',
      enabledTools: ['project_package_info'],
    })
    const result = await definition.execute('example-call', {})
    assert.equal(result.details.name, 'pisper')
    assert.match(result.details.version, /^\d+\.\d+\.\d+/)
    assert.equal(result.details.scripts.includes('dev'), true)
  })
})

test('plugin definitions create global source, install, and execute without overwriting', async () => {
  await withService(async ({ root, dataDir, service }) => {
    const created = await service.create({
      id: 'created.release',
      name: 'Release plugin',
      description: 'Provides release helpers.',
      permissions: ['workspace-read'],
      tools: [
        {
          name: 'created_version',
          description: 'Return a version string.',
          parameters: {
            type: 'object',
            properties: { prefix: { type: 'string' } },
          },
        },
      ],
      entryCode:
        "import { version } from './version.mjs'\nexport async function execute({ arguments: input, context }) { return `${input.prefix || ''}${version}:${context.cwd}` }\n",
      files: [{ path: 'version.mjs', content: "export const version = '1.2.3'\n" }],
    })

    assert.equal(created.sourcePath, join(dataDir, 'plugin-sources', 'created.release'))
    assert.deepEqual(created.tools, ['created_version'])
    const storedManifest = JSON.parse(
      await readFile(join(created.sourcePath, 'pisper-plugin.json'), 'utf8'),
    )
    assert.equal(storedManifest.entry, 'index.mjs')
    assert.equal(storedManifest.tools[0].name, 'created_version')
    assert.equal(
      (await service.getState()).plugins.some((plugin) => plugin.id === 'created.release'),
      true,
    )

    const otherProject = join(root, 'other-project')
    await mkdir(otherProject)
    const [definition] = service.createToolDefinitions({
      cwd: otherProject,
      sessionId: 'created-session',
      enabledTools: ['created_version'],
    })
    const result = await definition.execute('created-call', { prefix: 'v' })
    assert.equal(result.content[0].text, `v1.2.3:${otherProject}`)
    await assert.rejects(
      service.create({
        id: 'created.release',
        name: 'Replacement',
        tools: [{ name: 'replacement_tool', description: 'Replacement.' }],
        entryCode: 'export async function execute() { return true }',
      }),
      /已安装/,
    )
    assert.match(await readFile(join(created.sourcePath, 'index.mjs'), 'utf8'), /version\.mjs/)
  })
})

test('local plugins can be inspected, installed, executed, toggled, and removed', async () => {
  await withService(async ({ root, dataDir, configPath, service }) => {
    const initialWebSearch = (await service.getState()).tools.find(
      (tool) => tool.id === 'web_search',
    )
    assert.equal(initialWebSearch.name, 'Web Search')
    assert.equal(initialWebSearch.source, 'app')

    const source = await createFixture(root)
    const inspection = await service.inspect(source)
    assert.equal(inspection.plugin.id, 'example.echo')
    assert.equal(inspection.plugin.systemAccess, true)
    assert.equal(inspection.fileCount, 2)
    assert.match(inspection.warnings.join('\n'), /完全访问/)

    const installed = await service.install(inspection.inspectionId)
    assert.equal(installed.enabled, true)
    assert.deepEqual(service.enabledTools({ enabledTools: ['read'] }, 'workspace-write'), ['read'])
    assert.deepEqual(service.enabledTools({ enabledTools: ['read'] }, 'full-access'), [
      'read',
      'example_echo',
    ])

    const definitions = service.createToolDefinitions({
      cwd: root,
      sessionId: 'session-1',
      enabledTools: ['example_echo'],
    })
    assert.equal(definitions.length, 1)
    process.env.PISPER_SECRET = 'must-not-leak'
    const result = await definitions[0].execute('tool-call-1', { text: 'hello' })
    delete process.env.PISPER_SECRET
    assert.equal(result.content[0].text, 'example_echo:hello:true:clean')

    await service.setCapabilityEnabled('example.echo', 'example_echo', false)
    assert.equal((await service.getState()).enabledTools.includes('example_echo'), false)
    await service.setPluginEnabled('example.echo', true)
    assert.equal((await service.getState()).enabledTools.includes('example_echo'), true)

    const restored = new ToolPluginService(configPath, { dataDir })
    await restored.init()
    assert.equal(
      (await restored.getState()).plugins.some((plugin) => plugin.id === 'example.echo'),
      true,
    )
    restored.dispose()

    await service.uninstall('example.echo')
    assert.equal(
      (await service.getState()).plugins.some((plugin) => plugin.id === 'example.echo'),
      false,
    )
    const state = JSON.parse(await readFile(join(dataDir, 'pisper-plugins.json'), 'utf8'))
    assert.deepEqual(state.plugins, {})
  })
})

test('uninstall removes every plugin tool and is blocked while one is running', async () => {
  await withService(async ({ root, service }) => {
    const source = await createFixture(root, {
      tools: [
        { name: 'example_first', description: 'First local tool.' },
        { name: 'example_second', description: 'Second local tool.' },
      ],
    })
    await writeFile(
      join(source, 'index.mjs'),
      'export async function execute({ toolName }) { await new Promise((resolve) => setTimeout(resolve, 40)); return toolName }\n',
      'utf8',
    )
    const inspection = await service.inspect(source)
    await service.install(inspection.inspectionId)
    const definitions = service.createToolDefinitions({
      cwd: root,
      sessionId: 'session-1',
      enabledTools: ['example_first', 'example_second'],
    })
    assert.equal(definitions.length, 2)

    const running = definitions[0].execute('tool-call-active', {})
    await assert.rejects(service.uninstall('example.echo'), /正在执行/)
    await running
    const state = await service.uninstall('example.echo')
    assert.equal(
      state.plugins.some((plugin) => plugin.id === 'example.echo'),
      false,
    )
    assert.equal(
      state.tools.some((tool) => tool.name.startsWith('example_')),
      false,
    )
  })
})

test('concurrent installation cannot race the same plugin id', async () => {
  await withService(async ({ root, service }) => {
    const source = await createFixture(root)
    const first = await service.inspect(source)
    const second = await service.inspect(source)
    const results = await Promise.allSettled([
      service.install(first.inspectionId),
      service.install(second.inspectionId),
    ])
    assert.equal(results.filter((result) => result.status === 'fulfilled').length, 1)
    assert.equal(results.filter((result) => result.status === 'rejected').length, 1)
  })
})

test('installation rejects a source changed after inspection', async () => {
  await withService(async ({ root, service }) => {
    const source = await createFixture(root)
    const inspection = await service.inspect(source)
    await writeFile(join(source, 'index.mjs'), 'export const execute = () => "changed"\n', 'utf8')
    await assert.rejects(service.install(inspection.inspectionId), /发生了变化/)
  })
})

test('inspection rejects built-in tool conflicts and invalid manifests', async () => {
  await withService(async ({ root, service }) => {
    await assert.rejects(service.uninstall('builtin.filesystem'), /本地插件/)
    const source = await createFixture(root, {
      tools: [{ name: 'read', description: 'Conflicts with the built-in read tool.' }],
    })
    await assert.rejects(service.inspect(source), /与内置工具冲突/)

    const invalidSchema = await createFixture(root, {
      tools: [
        {
          name: 'invalid_schema',
          description: 'Invalid schema.',
          parameters: { type: 'object', required: 'text' },
        },
      ],
    })
    await assert.rejects(service.inspect(invalidSchema), /required 必须为字符串数组/)

    const invalid = join(root, 'invalid')
    await mkdir(invalid)
    await writeFile(join(invalid, 'pisper-plugin.json'), '{', 'utf8')
    await assert.rejects(service.inspect(invalid), /不是有效的 JSON/)

    const oversized = join(root, 'oversized')
    await mkdir(oversized)
    await writeFile(join(oversized, 'pisper-plugin.json'), ' '.repeat(256 * 1024 + 1), 'utf8')
    await assert.rejects(service.inspect(oversized), /超过 256 KB/)
  })
})
