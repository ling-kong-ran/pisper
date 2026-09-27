import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { mkdtemp, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { randomUUID } from 'node:crypto'
import { GameAssetsService } from '../services/game-assets-service.mjs'
import { WorkflowMediaService } from '../services/workflow-media-service.mjs'
import { createApiHandler } from '../http/api-handler.mjs'
import { ImageToolsPlugin } from '../services/image-tools-plugin.mjs'

const PNG = Buffer.from(
  '89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082',
  'hex',
)
const emptyEngines = { engines: [] }
function project(reference, name = 'Game assets') {
  return {
    name,
    prompt: 'Reference style',
    reference,
    originalReference: reference,
    frameCount: 1,
    directions: ['S'],
    model: null,
    actions: [{ id: 'walk', name: 'Walk', prompt: '', enabled: true }],
  }
}

async function fixture(t, override) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-game-assets-http-'))
  const media = new WorkflowMediaService({ dataDir: join(root, 'game') })
  const workflowMedia = new WorkflowMediaService({ dataDir: join(root, 'workflows') })
  const calls = []
  const operations = {
    async execute(input) {
      calls.push(input)
      if (override) {
        const result = await override(input)
        if (result) return result
      }
      let frames = input.images ?? []
      if (input.operation === 'input') {
        const { media: stored } = await media.read(input.source.id)
        if (JSON.stringify(stored) !== JSON.stringify(input.source))
          throw Object.assign(new Error('Invalid media'), { code: 'workflow_media_invalid' })
        frames = [
          {
            media: stored,
            width: 1,
            height: 1,
            durationMs: 125,
            action: '',
            direction: '',
            columns: 1,
            rows: 1,
            frameCount: 1,
          },
        ]
      }
      if (input.operation === 'generate')
        frames = frames.map((frame) => ({ ...frame, action: 'Walk', direction: 'S' }))
      if (input.operation === 'edit')
        frames = input.edits.frames.map((edit) => ({
          ...frames[edit.sourceIndex],
          durationMs: edit.durationMs,
        }))
      return { output: { type: 'workflow-images', version: 1, frames }, summary: 'Complete' }
    },
  }
  const imageTools = new ImageToolsPlugin()
  const bounded = imageTools.bind(operations, 'workbench')
  const gameAssets = new GameAssetsService({
    dataDir: join(root, 'game'),
    media,
    operations: bounded,
  })
  const engineCalls = []
  const runtime = {
    capabilities: { features: { workflows: false, plugins: false } },
    gameAssets,
    gameAssetMedia: media,
    gameAssetOperations: bounded,
    get workflows() {
      throw new Error('workbench must not touch workflows')
    },
    workflowMedia,
    visualGeneration: {
      async getModelStatus(kind) {
        assert.equal(kind, 'image')
        return {
          models: [
            {
              id: 'paint',
              name: 'Paint',
              providerId: 'fixture',
              providerName: 'Fixture',
              apiKey: 'must-not-expose',
            },
          ],
        }
      },
    },
    spriteEngines: {
      catalog: () => Promise.resolve(emptyEngines),
      download: (id) => {
        engineCalls.push(['download', id])
        return Promise.resolve(emptyEngines)
      },
      cancel: (id) => {
        engineCalls.push(['cancel', id])
        return Promise.resolve(emptyEngines)
      },
    },
  }
  const handler = createApiHandler(runtime)
  const server = createServer(
    (req, res) => void handler(req, res, new URL(req.url, 'http://localhost')),
  )
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  t.after(async () => {
    await gameAssets.dispose()
    const closing = new Promise((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    )
    server.closeAllConnections()
    await closing
    await media.dispose()
    await workflowMedia.dispose()
    await rm(root, { recursive: true, force: true })
  })
  const base = `http://127.0.0.1:${server.address().port}/api/game-assets`
  const json = (path, value, method = 'POST') =>
    fetch(`${base}${path}`, {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(value),
    })
  const upload = async () => {
    const response = await fetch(`${base}/media?name=reference.png`, {
      method: 'POST',
      headers: { 'Content-Type': 'image/png' },
      body: PNG,
    })
    assert.equal(response.status, 201)
    return response.json()
  }
  return { root, runtime, base, json, upload, calls, engineCalls, gameAssets, workflowMedia }
}

test('independent HTTP routes work with workflows disabled and Agent image tools disabled', async (t) => {
  const { root, base, json, upload, gameAssets, engineCalls } = await fixture(t)
  const reference = await upload()
  const catalog = await fetch(base)
  assert.equal(catalog.status, 200)
  const state = await catalog.json()
  assert.equal(state.models[0].id, 'fixture/paint')
  assert.doesNotMatch(JSON.stringify(state), /must-not-expose|apiKey/)
  const created = await json('/projects', project(reference))
  assert.equal(created.status, 201)
  const value = await created.json()
  const edited = await json(
    `/projects/${value.id}`,
    { ...project(reference), name: 'Updated' },
    'PATCH',
  )
  assert.equal(edited.status, 200)
  assert.equal((await edited.json()).name, 'Updated')
  const response = await json(`/projects/${value.id}/run`, {})
  assert.equal(response.status, 202)
  const { job } = await response.json()
  await gameAssets.running.get(job.id)?.promise
  const completed = await fetch(`${base}/jobs/${job.id}`)
  assert.equal((await completed.json()).status, 'completed')
  const frames = await json(`/jobs/${job.id}/frames`, {
    frames: [{ sourceIndex: 0, durationMs: 300 }, { sourceIndex: 0 }],
  })
  assert.equal(frames.status, 200)
  const updated = await frames.json()
  assert.equal(updated.revision, 1)
  assert.equal(updated.originalOutput.frames.length, 1)
  assert.equal(updated.output.frames.length, 2)
  const downloaded = await json('/engines/background/download', {})
  assert.equal(downloaded.status, 202)
  assert.deepEqual(await downloaded.json(), emptyEngines)
  assert.equal((await json('/engines/background/cancel', {})).status, 200)
  assert.deepEqual(engineCalls, [
    ['download', 'background'],
    ['cancel', 'background'],
  ])
  const removed = await fetch(`${base}/projects/${value.id}`, { method: 'DELETE' })
  assert.equal(removed.status, 200)
  assert.deepEqual(await removed.json(), { deleted: true })
  assert.equal((await fetch(`${base}/jobs/${job.id}`)).status, 404)
  assert.deepEqual((await readdir(join(root, 'game'))).sort(), [
    'game-assets.json',
    'workflow-media',
  ])
})

test('media read and processing stay inside the workbench namespace', async (t) => {
  const { base, json, upload, workflowMedia, calls } = await fixture(t)
  const reference = await upload()
  const loaded = await fetch(`${base}/media/${reference.id}/content`)
  assert.equal(loaded.headers.get('content-type'), 'image/png')
  assert.equal(loaded.headers.get('x-content-type-options'), 'nosniff')
  assert.deepEqual(Buffer.from(await loaded.arrayBuffer()), PNG)
  const processed = await json('/process', {
    reference,
    operation: 'background',
    image: { method: 'color' },
  })
  assert.equal(processed.status, 200)
  assert.deepEqual(await processed.json(), reference)
  assert.deepEqual(
    calls.map((call) => call.operation),
    ['input', 'background'],
  )
  const foreign = await workflowMedia.upload({
    name: 'foreign.png',
    mimeType: 'image/png',
    buffer: PNG,
  })
  const notFound = await fetch(`${base}/media/${foreign.id}/content`)
  assert.equal(notFound.status, 404)
  assert.equal((await notFound.json()).code, 'workflow_media_missing')
  const forged = await json('/projects', project(foreign))
  assert.equal(forged.status, 400)
  assert.equal((await forged.json()).code, 'game_assets_media_invalid')
  const unsupported = await fetch(`${base}/media`, {
    method: 'POST',
    headers: { 'Content-Type': 'video/mp4' },
    body: PNG,
  })
  assert.equal(unsupported.status, 415)
})

test('HTTP boundaries reject malformed bodies and identity spoofing, and never echo upstream messages', async (t) => {
  const { base, json, upload, runtime } = await fixture(t)
  const reference = await upload()
  const value = await (await json('/projects', project(reference))).json()
  const mismatched = await json(
    `/projects/${value.id}`,
    { ...project(reference), id: randomUUID() },
    'PATCH',
  )
  assert.equal(mismatched.status, 400)
  for (const [path, body] of [
    ['/projects', { ...project(reference), workflowId: 'forged' }],
    ['/projects', { ...project(reference), frameCount: Infinity }],
    ['/process', { reference, operation: 'generate' }],
    ['/process', { reference, operation: 'background', consumer: 'agent' }],
    [`/jobs/${randomUUID()}/frames`, { frames: [{ sourceIndex: -1 }] }],
    ['/engines/unknown/download', {}],
  ])
    assert.equal((await json(path, body)).status, 400)
  const broken = await fetch(`${base}/projects`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: '{ broken secret',
  })
  assert.equal(broken.status, 400)
  assert.deepEqual(await broken.json(), {
    code: 'game_assets_invalid',
    error: 'game_assets_invalid',
  })
  runtime.gameAssetOperations.execute = () =>
    Promise.reject(new Error('private token and filesystem path'))
  const failed = await json('/process', { reference, operation: 'inpaint' })
  assert.equal(failed.status, 500)
  assert.deepEqual(await failed.json(), { error: 'game_assets_failed', code: 'game_assets_failed' })
})

test('closing a process response aborts local computation and does not leave work running', async (t) => {
  const entered = Promise.withResolvers()
  const cancelled = Promise.withResolvers()
  const { base, upload } = await fixture(t, (request) => {
    if (request.operation !== 'background') return
    entered.resolve()
    return new Promise((_, reject) =>
      request.signal.addEventListener(
        'abort',
        () => {
          cancelled.resolve()
          reject(Object.assign(new Error('cancelled'), { code: 'workflow_image_cancelled' }))
        },
        { once: true },
      ),
    )
  })
  const reference = await upload()
  const controller = new AbortController()
  const task = fetch(`${base}/process`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ reference, operation: 'background' }),
    signal: controller.signal,
  })
  const rejected = assert.rejects(task, { name: 'AbortError' })
  await entered.promise
  controller.abort()
  await rejected
  await cancelled.promise
})

test('stop endpoint cancels generation and retains a readable standalone job', async (t) => {
  const entered = Promise.withResolvers()
  const { base, json, upload } = await fixture(t, (request) => {
    if (request.operation !== 'generate') return
    entered.resolve()
    return new Promise((_, reject) =>
      request.signal.addEventListener('abort', () => reject(new Error('upstream aborted')), {
        once: true,
      }),
    )
  })
  const reference = await upload()
  const value = await (await json('/projects', project(reference))).json()
  const { job } = await (await json(`/projects/${value.id}/run`, {})).json()
  await entered.promise
  const stopped = await json(`/jobs/${job.id}/stop`, {})
  assert.equal(stopped.status, 200)
  assert.equal((await stopped.json()).job.status, 'cancelled')
  assert.equal(
    (await (await fetch(`${base}/jobs/${job.id}`)).json()).error,
    'game_assets_cancelled',
  )
})
