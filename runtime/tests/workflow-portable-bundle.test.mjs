import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { WorkflowMediaService } from '../services/workflow-media-service.mjs'
import {
  exportPortableWorkflowBundle,
  importPortableWorkflowBundle,
} from '../services/workflow-portable-bundle.mjs'
import {
  encodeWorkflowBundle,
  decodeWorkflowBundle,
  bundleJson,
  jsonBundleFile,
} from '../services/workflow-bundle-archive.mjs'

const PNG = Buffer.from(
  '89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082',
  'hex',
)
const definition = (media) => ({
  format: 'pisper-workflow',
  version: 1,
  workflow: {
    name: 'Reusable',
    cwd: '/old-machine-private-workspace',
    model: { provider: 'local', model: 'vision' },
    nodes: [
      {
        id: 'a',
        prompt: 'Review {{inputs.reference}}',
        requestedToolNames: ['read'],
        skillName: 'review',
      },
    ],
    edges: [],
    inputs: [
      {
        id: 'source',
        name: 'reference',
        label: 'Reference',
        description: '',
        type: 'image',
        required: true,
        defaultValue: media,
      },
    ],
    notifications: [],
  },
})

test('workflow ZIP includes default media, remaps references offline and strips the old workspace', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-workflow-bundle-'))
  const source = new WorkflowMediaService({ dataDir: join(root, 'source') })
  const destination = new WorkflowMediaService({ dataDir: join(root, 'destination') })
  t.after(async () => {
    await source.dispose()
    await destination.dispose()
    await rm(root, { recursive: true, force: true })
  })
  const media = await source.upload({ name: 'reference.png', mimeType: 'image/png', buffer: PNG })
  const original = definition(media)
  const zip = await exportPortableWorkflowBundle(original, source)
  const files = decodeWorkflowBundle(zip)
  assert.deepEqual(Buffer.from(files[`media/${media.id}/data.bin`]), PNG)
  assert.equal(bundleJson(files, 'workflow.json').workflow.cwd, '')
  assert.equal(original.workflow.cwd, '/old-machine-private-workspace')
  const restored = await importPortableWorkflowBundle(zip, destination)
  const imported = restored.definition.workflow.inputs[0].defaultValue
  assert.notEqual(imported.id, media.id)
  assert.equal(imported.name, media.name)
  assert.deepEqual(restored.requirements.models, ['local/vision'])
  assert.deepEqual(restored.requirements.skills, ['review'])
  assert.deepEqual(restored.requirements.tools, ['read'])
  const verified = await destination.exportFilesForWorkflow(restored.definition.workflow)
  assert.deepEqual(Buffer.from(verified[`media/${imported.id}/data.bin`]), PNG)
  await destination.discardImported(restored.mediaMapping)
  await assert.rejects(destination.exportFilesForWorkflow(restored.definition.workflow), {
    code: 'workflow_media_missing',
  })
})

test('workflow ZIP rejects missing/tampered/unreferenced media before installing anything', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-workflow-package-'))
  const mediaService = new WorkflowMediaService({ dataDir: root })
  t.after(async () => {
    await mediaService.dispose()
    await rm(root, { recursive: true, force: true })
  })
  const media = await mediaService.upload({ name: 'input.png', mimeType: 'image/png', buffer: PNG })
  const files = decodeWorkflowBundle(
    await exportPortableWorkflowBundle(definition(media), mediaService),
  )
  const missing = { ...files }
  delete missing[`media/${media.id}/data.bin`]
  await assert.rejects(importPortableWorkflowBundle(encodeWorkflowBundle(missing), mediaService), {
    code: 'workflow_media_missing',
  })
  const tampered = { ...files, [`media/${media.id}/data.bin`]: Uint8Array.of(0) }
  await assert.rejects(importPortableWorkflowBundle(encodeWorkflowBundle(tampered), mediaService), {
    code: 'workflow_media_invalid',
  })
  const unreferenced = definition(null)
  await assert.rejects(
    importPortableWorkflowBundle(
      encodeWorkflowBundle({ ...files, 'workflow.json': jsonBundleFile(unreferenced) }),
      mediaService,
    ),
    { code: 'workflow_bundle_invalid' },
  )
})
