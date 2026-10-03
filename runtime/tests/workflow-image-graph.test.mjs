import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { WorkflowService } from '../services/workflow-service.mjs'
import {
  normalizeWorkflowImageSettings,
  parseWorkflowImageOutput,
} from '../../shared/workflow/workflow-image-nodes.mjs'

const output = {
  type: 'workflow-images',
  version: 1,
  frames: [
    {
      media: { id: 'fixture', name: 'frame.png', mimeType: 'image/png', size: 20 },
      width: 1,
      height: 1,
      durationMs: 125,
      action: 'idle',
      direction: 'S',
      columns: 1,
      rows: 1,
      frameCount: 1,
    },
  ],
}
async function fixture(t, executeImageNode) {
  const path = await mkdtemp(join(tmpdir(), 'pisper-image-graph-'))
  const service = new WorkflowService({
    path: join(path, 'workflows.json'),
    cwd: path,
    agent: {
      validateDirectory: async (cwd) => cwd,
      prompt: () => assert.fail('Image nodes must not invoke a chat agent'),
      abort: async () => {},
    },
    notifications: { notify: async () => {} },
    executeImageNode,
  })
  await service.init()
  t.after(async () => {
    await service.dispose()
    await rm(path, { recursive: true, force: true })
  })
  return service
}

test('image nodes compose in the ordinary graph and preserve their configuration through save and reload shape', async (t) => {
  const calls = []
  const service = await fixture(t, (request) => {
    calls.push(request)
    return Promise.resolve({ output, summary: '1 frame' })
  })
  const workflow = await service.create({
    name: 'Reusable images',
    inputs: [{ name: 'task', type: 'text', required: true }],
    nodes: [
      { id: 'source', kind: 'media-input', image: { inputName: 'reference' } },
      { id: 'background', kind: 'media-background', image: { colors: ['#ff00ff'], softness: 12 } },
      { id: 'preview', kind: 'media-preview', prompt: '{{inputs.task}}' },
    ],
    edges: [
      { source: 'source', target: 'background' },
      { source: 'background', target: 'preview' },
    ],
  })
  assert.equal(workflow.nodes[1].kind, 'media-background')
  assert.equal(workflow.nodes[1].image.softness, 12)
  const run = await service.runNow(workflow.id, { inputs: { task: 'new subject' } })
  await service.active.get(run.id)?.done
  const finished = service.getRun(run.id)
  assert.equal(finished.status, 'completed')
  assert.deepEqual(
    calls.map(({ node }) => node.kind),
    ['media-input', 'media-background', 'media-preview'],
  )
  assert.deepEqual(calls[1].predecessors[0].output, output)
  assert.equal(calls[2].node.prompt, 'new subject')
  assert.deepEqual(finished.nodes[2].output, output)
  const imported = await service.importWorkflow(service.exportWorkflow(workflow.id))
  assert.deepEqual(imported.nodes[1].image, workflow.nodes[1].image)
})

test('cancelling or disposing an image workflow aborts the node owner and waits for execution cleanup', async (t) => {
  const entered = Promise.withResolvers()
  let cleaned = false
  const service = await fixture(
    t,
    ({ signal }) =>
      new Promise((resolve, reject) => {
        entered.resolve()
        signal.addEventListener(
          'abort',
          () => {
            cleaned = true
            reject(signal.reason)
          },
          { once: true },
        )
      }),
  )
  const workflow = await service.create({
    name: 'Cancel image',
    nodes: [{ id: 'source', kind: 'media-input' }],
  })
  const run = await service.runNow(workflow.id)
  await entered.promise
  await service.dispose()
  assert.equal(cleaned, true)
  assert.equal(service.active.size, 0)
  assert.equal(service.getRun(run.id).status, 'cancelled')
})

test('single-node reruns retain valid upstream results, invalidate descendants and reject stale sources', async (t) => {
  const calls = []
  const service = await fixture(t, ({ node, predecessors }) => {
    calls.push({ id: node.id, predecessors })
    return Promise.resolve({ output, summary: node.id })
  })
  const workflow = await service.create({
    name: 'Edit frames',
    nodes: [
      { id: 'source', kind: 'media-input' },
      { id: 'edit', kind: 'media-transform' },
      { id: 'preview', kind: 'media-preview' },
      { id: 'export', kind: 'media-export' },
    ],
    edges: [
      { source: 'source', target: 'edit' },
      { source: 'edit', target: 'preview' },
      { source: 'preview', target: 'export' },
    ],
  })
  const initial = await service.runNow(workflow.id)
  await service.active.get(initial.id)?.done
  const updated = await service.update(workflow.id, {
    nodes: workflow.nodes.map((node) =>
      node.id === 'edit' ? { ...node, image: { ...node.image, padding: 12 } } : node,
    ),
  })
  calls.length = 0
  const edited = await service.runNow(workflow.id, { nodeId: 'edit', sourceRunId: initial.id })
  await service.active.get(edited.id)?.done
  assert.deepEqual(
    calls.map(({ id }) => id),
    ['edit'],
  )
  assert.deepEqual(
    service.getRun(edited.id).nodes.map(({ id }) => id),
    ['source', 'edit'],
  )
  await assert.rejects(service.runNow(workflow.id, { nodeId: 'export', sourceRunId: initial.id }), {
    code: 'workflow_image_source_stale',
  })
  const preview = await service.runNow(workflow.id, { nodeId: 'preview', sourceRunId: edited.id })
  await service.active.get(preview.id)?.done
  const exported = await service.runNow(workflow.id, { nodeId: 'export', sourceRunId: preview.id })
  await service.active.get(exported.id)?.done
  assert.equal(service.getRun(exported.id).status, 'completed')
  assert.deepEqual(
    calls.map(({ id }) => id),
    ['edit', 'preview', 'export'],
  )
  await service.update(workflow.id, {
    nodes: updated.nodes.map((node) =>
      node.id === 'source' ? { ...node, image: { ...node.image, inputName: 'different' } } : node,
    ),
  })
  await assert.rejects(service.runNow(workflow.id, { nodeId: 'edit', sourceRunId: exported.id }), {
    code: 'workflow_image_source_stale',
  })
})

test('failed image generation retains successful direction frames without automatic paid retry', async (t) => {
  let calls = 0
  const service = await fixture(t, () => {
    calls++
    return Promise.reject(
      Object.assign(new Error('workflow_image_generation_failed'), { partialOutput: output }),
    )
  })
  const workflow = await service.create({
    name: 'Partial generation',
    nodes: [{ id: 'generate', kind: 'media-generate', retries: 3, prompt: 'Retain the reference' }],
  })
  const run = await service.runNow(workflow.id)
  await service.active.get(run.id)?.done
  assert.equal(service.getRun(run.id).status, 'failed')
  assert.deepEqual(service.getRun(run.id).nodes[0].output, output)
  assert.equal(calls, 1)
})

test('image node settings reject invalid transforms, duplicate directions and unsafe input names', () => {
  for (const value of [
    { directions: ['S', 'S'] },
    { inputName: '__proto__' },
    { transforms: [{ scale: Infinity }] },
    { colors: ['javascript:alert(1)'] },
    { frameOrder: [0, 0] },
  ])
    assert.throws(() => normalizeWorkflowImageSettings(value), { code: 'workflow_image_invalid' })
  assert.deepEqual(parseWorkflowImageOutput(output), output)
  assert.throws(
    () =>
      parseWorkflowImageOutput({
        ...output,
        frames: [
          { ...output.frames[0], media: { ...output.frames[0].media, mimeType: 'video/mp4' } },
        ],
      }),
    { code: 'workflow_image_invalid' },
  )
})
