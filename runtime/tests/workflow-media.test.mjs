import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { mkdtemp, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { WorkflowMediaService } from '../services/workflow-media-service.mjs'
import { WorkflowService } from '../services/workflow-service.mjs'
import { createApiHandler } from '../http/api-handler.mjs'
import { parseWorkflowMedia } from '../../shared/workflow/workflow-inputs.mjs'

const PNG = Buffer.from(
  '89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082',
  'hex',
)
const MP4 = Buffer.from('000000206674797069736f6d00000000', 'hex')

async function fixture(t) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-workflow-media-'))
  const service = new WorkflowMediaService({ dataDir })
  t.after(async () => {
    await service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  return { service, dataDir }
}

test('media inputs resolve verified images as attachments and videos as local file references', async (t) => {
  const { service } = await fixture(t)
  const image = await service.upload({ name: 'source.png', mimeType: 'image/png', buffer: PNG })
  const video = await service.upload({ name: 'clip.mp4', mimeType: 'video/mp4', buffer: MP4 })
  const resolved = await service.resolveInputs({ image, video, task: 'Analyze' })
  assert.equal(resolved.attachments.length, 1)
  assert.equal(resolved.attachments[0].kind, 'image')
  assert.deepEqual(Buffer.from(resolved.attachments[0].data, 'base64'), PNG)
  assert.match(resolved.context, /not a native video model attachment/)
  assert.ok(!JSON.stringify(image).includes('path'))
  assert.ok(!JSON.stringify(video).includes('path'))
  await assert.rejects(service.resolveInputs({ image: { ...image, size: image.size + 1 } }), {
    code: 'workflow_media_invalid',
  })
  assert.throws(() => service.upload({ name: 'bad.mp4', mimeType: 'video/mp4', buffer: PNG }), {
    code: 'workflow_media_invalid',
  })
  assert.throws(
    () =>
      service.upload({
        name: 'huge.png',
        mimeType: 'image/png',
        buffer: Buffer.alloc(8 * 1024 * 1024 + 1),
      }),
    { code: 'workflow_media_too_large' },
  )
})

test('portable media verifies complete references, remaps ids and can discard only its own imported copy', async (t) => {
  const { service } = await fixture(t)
  const image = await service.upload({ name: 'source.png', mimeType: 'image/png', buffer: PNG })
  const files = await service.exportFilesForWorkflow({
    inputs: [{ name: 'image', type: 'image', defaultValue: image }],
  })
  const mapping = await service.importBundleFiles(files)
  assert.notEqual(mapping[image.id].id, image.id)
  assert.deepEqual((await service.read(mapping[image.id].id)).buffer, PNG)
  await assert.rejects(service.discardImported({ forged: image }), {
    code: 'workflow_media_invalid',
  })
  const importedId = mapping[image.id].id
  mapping[image.id] = image
  await service.discardImported(mapping)
  await assert.rejects(service.read(importedId), { code: 'workflow_media_missing' })
  assert.deepEqual((await service.read(image.id)).buffer, PNG)
  const missing = { ...files }
  delete missing[`media/${image.id}/data.bin`]
  assert.throws(() => service.validateBundleFiles(missing), { code: 'workflow_media_missing' })
  assert.throws(() => service.validateBundleFiles({ ...files, 'media/../data.bin': PNG }), {
    code: 'workflow_media_invalid',
  })
})

test('workflow execution forwards real image attachments while durable run inputs retain only media references', async (t) => {
  const { service: media, dataDir } = await fixture(t)
  const image = await media.upload({ name: 'source.png', mimeType: 'image/png', buffer: PNG })
  const video = await media.upload({ name: 'clip.mp4', mimeType: 'video/mp4', buffer: MP4 })
  const completed = Promise.withResolvers()
  const prompts = []
  const workflows = new WorkflowService({
    path: join(dataDir, 'workflows.json'),
    cwd: dataDir,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => {},
      prompt: async (input) => {
        prompts.push(input)
        return { text: 'Analyzed', sessionId: 'fixture', assets: [] }
      },
    },
    notifications: { notify: async () => completed.resolve() },
    resolveMediaInputs: (inputs) => media.resolveInputs(inputs),
  })
  await workflows.init()
  try {
    const workflow = await workflows.create({
      name: 'Media input',
      status: 'published',
      notifications: ['browser'],
      inputs: [
        { name: 'image', label: 'Image', type: 'image', defaultValue: image },
        { name: 'video', label: 'Video', type: 'video', defaultValue: video },
      ],
      nodes: [
        { id: 'inspect', kind: 'prompt', prompt: 'Inspect {{inputs.image}} and {{inputs.video}}' },
      ],
    })
    const run = await workflows.runNow(workflow.id)
    await completed.promise
    assert.equal(prompts.length, 1)
    assert.deepEqual(Buffer.from(prompts[0].attachments[0].data, 'base64'), PNG)
    assert.match(prompts[0].message, /not a native video model attachment/)
    assert.deepEqual(workflows.getRun(run.id).inputs, { image, video })
    assert.ok(!JSON.stringify(workflows.getRun(run.id).inputs).includes('base64'))
    assert.ok(!JSON.stringify(workflows.getRun(run.id).inputs).includes(dataDir))
  } finally {
    await workflows.dispose()
  }
})

test('media serving rejects traversal and tampered symlink content', async (t) => {
  const { service, dataDir } = await fixture(t)
  const image = await service.upload({ name: 'source.png', mimeType: 'image/png', buffer: PNG })
  await assert.rejects(service.read('../outside'), { code: 'workflow_media_invalid' })
  const data = join(dataDir, 'workflow-media', image.id, 'data.bin')
  const outside = join(dataDir, 'outside.png')
  await writeFile(outside, PNG)
  await rm(data)
  await symlink(outside, data)
  await assert.rejects(service.read(image.id), { code: 'workflow_media_invalid' })
})

test('workflow media HTTP uploads raw bytes and returns reference metadata without base64', async (t) => {
  const { service } = await fixture(t)
  const handle = createApiHandler({ workflowMedia: service })
  const server = createServer(
    (req, res) => void handle(req, res, new URL(req.url, 'http://localhost')),
  )
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  t.after(
    () =>
      new Promise((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      ),
  )
  const root = `http://127.0.0.1:${server.address().port}/api/workflow-media`
  const uploaded = await fetch(`${root}?name=source.png`, {
    method: 'POST',
    headers: { 'Content-Type': 'image/png' },
    body: PNG,
  })
  assert.equal(uploaded.status, 201)
  const reference = parseWorkflowMedia(await uploaded.json())
  const loaded = await fetch(`${root}/${reference.id}/content`)
  assert.equal(loaded.headers.get('content-type'), 'image/png')
  assert.deepEqual(Buffer.from(await loaded.arrayBuffer()), PNG)
  const invalid = await fetch(`${root}?name=image.png`, {
    method: 'POST',
    headers: { 'Content-Type': 'image/png' },
    body: 'https://example.test/not-image',
  })
  assert.equal(invalid.status, 400)
  assert.equal((await invalid.json()).code, 'workflow_media_invalid')
  const invalidBundle = await fetch(root.replace('/workflow-media', '/workflows/import-bundle'), {
    method: 'POST',
    headers: { 'Content-Type': 'application/zip' },
    body: 'broken zip',
  })
  assert.equal(invalidBundle.status, 400)
  assert.deepEqual(await invalidBundle.json(), {
    code: 'workflow_bundle_invalid',
    error: '工作流压缩包无效或内容不完整。',
  })
})
