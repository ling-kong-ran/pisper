import assert from 'node:assert/strict'
import { test } from 'node:test'
import { ImageToolsPlugin } from '../services/image-tools-plugin.mjs'
import { createImageAssetsTool, manifest } from '../tools/app/image-assets.mjs'
import { APP_TOOL_CATALOG, createAppToolDefinitions } from '../tools/app/index.mjs'
import { TOOL_PRESETS, toolsFromConfig } from '../tools/registry.mjs'

const source = { id: 'sample', name: 'sample.png', mimeType: 'image/png', size: 100 }
const output = { type: 'workflow-images', version: 1, frames: [] }

test('internal image operations are available to workbench and workflows while agent use defaults off', async () => {
  const plugin = new ImageToolsPlugin()
  const calls = []
  const operations = {
    execute: async (request) => {
      calls.push(request)
      return { output, summary: 'done' }
    },
  }
  await plugin.bind(operations, 'workflow').execute({ operation: 'frames' })
  await plugin.bind(operations, 'workbench').execute({ operation: 'edit' })
  await assert.rejects(plugin.bind(operations, 'agent').execute({ operation: 'frames' }), {
    code: 'image_tools_agent_disabled',
  })
  assert.equal(calls.length, 2)
  assert.ok(APP_TOOL_CATALOG.some((tool) => tool.id === manifest.id))
  assert.equal(toolsFromConfig().includes(manifest.id), false)
  for (const preset of Object.values(TOOL_PRESETS))
    assert.equal(preset.includes(manifest.id), false)
})

test('a cached agent capability checks the latest switch and cannot impersonate a UI consumer', async () => {
  let enabled = true,
    calls = 0
  const plugin = new ImageToolsPlugin({ isAgentEnabled: () => enabled })
  const operations = {
    execute: async () => {
      calls++
      return { output }
    },
  }
  const agent = plugin.bind(operations, 'agent')
  await agent.execute({ operation: 'preview' })
  enabled = false
  await assert.rejects(agent.execute({ operation: 'preview', consumer: 'workflow' }), {
    code: 'image_tools_agent_disabled',
  })
  await plugin.bind(operations, 'workbench').execute({ operation: 'preview' })
  assert.equal(calls, 2)
  await assert.rejects(plugin.bind(operations, 'unexpected').execute({}), {
    code: 'image_tools_invalid_consumer',
  })
})

test('plugin propagates cancellation while checking asynchronous authorization', async () => {
  let release
  const waiting = new Promise((resolve) => {
    release = resolve
  })
  const plugin = new ImageToolsPlugin({ isAgentEnabled: () => waiting })
  let called = false
  const agent = plugin.bind(
    {
      execute: async () => {
        called = true
      },
    },
    'agent',
  )
  const controller = new AbortController()
  const task = agent.execute({}, { signal: controller.signal })
  controller.abort()
  release(true)
  await assert.rejects(task, { name: 'AbortError' })
  assert.equal(called, false)
})

test('image asset tool imports a workspace source and forwards controlled references and edits', async () => {
  const received = [],
    imports = []
  const imageAssets = {
    importImage: async (cwd, path, options) => {
      imports.push({ cwd, path, options })
      return source
    },
    execute: async (request, options) => {
      received.push({ request, options })
      return { output, summary: 'done' }
    },
  }
  const tool = createAppToolDefinitions({
    enabledTools: [manifest.id],
    cwd: '/workspace',
    imageAssets,
  })[0]
  const controller = new AbortController()
  const result = await tool.execute(
    'first',
    { operation: 'input', sourceImage: 'character.png' },
    controller.signal,
  )
  assert.deepEqual(result.details.output, output)
  assert.deepEqual(JSON.parse(result.content[0].text), result.details)
  assert.deepEqual(received[0].request.source, source)
  assert.equal(imports[0].cwd, '/workspace')
  assert.equal(imports[0].options.signal, controller.signal)
  assert.equal(received[0].options.signal, controller.signal)
  await tool.execute('edit', {
    operation: 'edit',
    source,
    edits: { frames: [{ sourceIndex: 0, x: 12, durationMs: 300 }] },
  })
  assert.equal(received[1].request.edits.frames[0].x, 12)
  assert.equal(received[1].request.edits.frames[0].scale, 1)
  await tool.execute('generate', {
    operation: 'generate',
    source,
    model: 'provider/vendor/image-model',
  })
  assert.deepEqual(received[2].request.model, {
    provider: 'provider',
    model: 'vendor/image-model',
  })
})

test('asset export returns workspace files and notifies each generated file without losing successful output', async () => {
  const files = [
    { path: '/workspace/generated/image-assets/atlas.png', mimeType: 'image/png' },
    { path: '/workspace/generated/image-assets/atlas.json', mimeType: 'application/json' },
  ]
  const notified = []
  const controller = new AbortController()
  const tool = createImageAssetsTool({
    cwd: '/workspace',
    imageAssets: {
      execute: async () => ({ output, summary: 'exported' }),
      exportImages: async (cwd, result, options) => {
        assert.equal(cwd, '/workspace')
        assert.equal(result, output)
        assert.equal(options.signal, controller.signal)
        return { files }
      },
    },
    onGeneratedFile: async (file) => {
      notified.push(file)
      if (notified.length === 1) throw new Error('Index unavailable')
    },
  })
  const result = await tool.execute('export', { operation: 'export', source }, controller.signal)
  assert.deepEqual(result.details.files, files)
  assert.deepEqual(JSON.parse(result.content[0].text).files, files)
  assert.deepEqual(notified, files)
})

test('tool rejects arbitrary URLs, unrecognized fields and malformed edits before capability execution', async () => {
  let executed = false
  const tool = createImageAssetsTool({
    imageAssets: {
      execute: async () => {
        executed = true
      },
    },
  })
  for (const request of [
    { operation: 'preview', source: 'https://example.com/private.png' },
    { operation: 'preview', source: { ...source, url: 'https://example.com/private.png' } },
    { operation: 'preview', consumer: 'workflow' },
    { operation: 'generate', model: '/model' },
    { operation: 'generate', model: 'provider/' },
    { operation: 'generate', model: 'missing-provider' },
    { operation: 'edit', source, edits: { frames: [{ sourceIndex: 0, scale: Infinity }] } },
    {
      operation: 'edit',
      source,
      edits: {
        frames: [{ sourceIndex: 0, eraseStrokes: [{ radius: 0.1, points: [{ x: -1, y: 0 }] }] }],
      },
    },
    { operation: 'generate', prompt: 'x'.repeat(16001) },
  ])
    await assert.rejects(tool.execute('invalid', request), { code: 'workflow_image_invalid' })
  assert.equal(executed, false)
})
