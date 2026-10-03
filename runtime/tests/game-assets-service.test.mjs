import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'
import { mkdtemp, readFile, readdir, rm, mkdir, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { PNG } from 'pngjs'
import { GameAssetsService } from '../services/game-assets-service.mjs'
import { WorkflowMediaService } from '../services/workflow-media-service.mjs'
import {
  parseGameAssetProjectInput,
  parseGameAssetsCatalog,
} from '../../shared/game/game-assets.mjs'

function png() {
  const image = new PNG({ width: 16, height: 16 })
  image.data.fill(255)
  return PNG.sync.write(image)
}
function deferred() {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}
function input(reference, name = 'Character') {
  return {
    name,
    prompt: 'Keep the reference style',
    reference,
    originalReference: reference,
    frameCount: 2,
    directions: ['S', 'N'],
    model: null,
    actions: [
      { id: 'idle', name: 'Idle', prompt: 'Breathing', enabled: true },
      { id: 'walk', name: 'Walk', prompt: 'Forward', enabled: true },
    ],
  }
}
function output(frames = []) {
  return { type: 'workflow-images', version: 1, frames }
}
async function fixture(t, execute) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-game-assets-'))
  const media = new WorkflowMediaService({ dataDir: root })
  const reference = await media.upload({
    name: 'reference.png',
    mimeType: 'image/png',
    buffer: png(),
  })
  const calls = []
  const operations = {
    async execute(request) {
      calls.push(request)
      if (execute) {
        const result = await execute(request)
        if (result) return result
      }
      let frames = request.images ?? [
        {
          media: request.source,
          width: 16,
          height: 16,
          durationMs: 125,
          action: '',
          direction: '',
          columns: 1,
          rows: 1,
          frameCount: 1,
        },
      ]
      if (request.operation === 'generate')
        frames = request.settings.directions.map((direction) => ({
          ...frames[0],
          action: request.settings.action,
          direction,
          frameCount: request.settings.frameCount,
          columns: request.settings.frameCount,
        }))
      if (request.operation === 'frames')
        frames = frames.flatMap((frame) =>
          Array.from({ length: frame.frameCount }, () => ({ ...frame, frameCount: 1, columns: 1 })),
        )
      if (request.operation === 'edit')
        frames = request.edits.frames.map((edit) => ({
          ...frames[edit.sourceIndex],
          durationMs: edit.durationMs,
        }))
      return { output: output(frames), summary: 'Completed' }
    },
  }
  const service = new GameAssetsService({ dataDir: root, media, operations })
  t.after(async () => {
    await service.dispose()
    await media.dispose()
    await rm(root, { recursive: true, force: true })
  })
  return { root, media, reference, service, operations, calls }
}
async function completed(service, job) {
  await service.running.get(job.id)?.promise
  return service.getJob(job.id)
}

test('standalone projects generate and persist without any workflow service or workflow records', async (t) => {
  const fixtureValue = await fixture(t)
  const { service, reference, root, calls } = fixtureValue
  const project = await service.save(input(reference))
  assert.equal((await service.catalog()).projects.length, 1)
  const done = await completed(service, await service.run(project.id))
  assert.equal(done.status, 'completed')
  assert.equal(done.completed, 2)
  assert.equal(done.output.frames.length, 8)
  assert.deepEqual(
    calls.map((call) => call.operation),
    [
      'input',
      'generate',
      'background',
      'frames',
      'transform',
      'generate',
      'background',
      'frames',
      'transform',
      'export',
    ],
  )
  assert.ok(
    calls
      .filter((call) => call.operation === 'background')
      .every((call) => call.settings.method === 'color'),
  )
  assert.deepEqual((await readdir(root)).sort(), ['game-assets.json', 'workflow-media'])
  const serialized = await readFile(join(root, 'game-assets.json'), 'utf8')
  assert.doesNotMatch(serialized, /workflowId|nodeId|"nodes"|"edges"|customized/)
  const restored = new GameAssetsService({
    dataDir: root,
    media: fixtureValue.media,
    operations: fixtureValue.operations,
  })
  await restored.init()
  assert.deepEqual(await restored.catalog(), await service.catalog())
  await restored.dispose()
  await service.remove(project.id)
  assert.deepEqual(await service.catalog(), { projects: [], jobs: [] })
})

test('input validation bounds work, allows drafts, and rejects forged or foreign media references', async (t) => {
  const { service, reference, root } = await fixture(t)
  const foreign = new WorkflowMediaService({ dataDir: join(root, 'foreign') })
  t.after(() => foreign.dispose())
  const otherReference = await foreign.upload({
    name: 'other.png',
    mimeType: 'image/png',
    buffer: png(),
  })
  await assert.rejects(service.save(input(otherReference)), { code: 'game_assets_media_invalid' })
  await assert.rejects(service.save(input({ ...reference, name: 'forged.png' })), {
    code: 'game_assets_media_invalid',
  })
  const draft = await service.save(input(null))
  await assert.rejects(service.run(draft.id), { code: 'game_assets_source_required' })
  for (const patch of [
    { frameCount: 1.5 },
    { frameCount: 17 },
    { directions: ['S', 'S'] },
    { directions: ['INVALID'] },
    { prompt: 'a'.repeat(8001) },
    { model: { provider: '', model: 'm' } },
    { actions: [{ id: '../x', name: 'x', prompt: '', enabled: true }] },
    {
      directions: ['S', 'SW', 'W', 'NW', 'N', 'NE', 'E', 'SE'],
      frameCount: 16,
      actions: Array.from({ length: 8 }, (_, i) => ({
        id: `${i}`,
        name: 'action',
        prompt: '',
        enabled: true,
      })),
    },
    { workflowId: 'coupled' },
  ])
    assert.throws(() => parseGameAssetProjectInput({ ...input(reference), ...patch }), {
      code: 'game_assets_invalid',
    })
  assert.deepEqual((await service.catalog()).projects, [draft])
})

test('per-project and global reservations prevent concurrent overwrites; cancelling releases the slot', async (t) => {
  const entered = deferred()
  const { service, reference } = await fixture(t, (request) => {
    if (request.operation !== 'generate') return
    entered.resolve()
    return new Promise((_, reject) =>
      request.signal.addEventListener(
        'abort',
        () => reject(new Error('sensitive upstream error')),
        { once: true },
      ),
    )
  })
  const first = await service.save(input(reference, 'first'))
  const second = await service.save(input(reference, 'second'))
  const third = await service.save(input(reference, 'third'))
  const job = await service.run(first.id)
  await entered.promise
  await assert.rejects(service.run(first.id), { code: 'game_assets_busy' })
  await assert.rejects(service.save({ ...input(reference), id: first.id }), {
    code: 'game_assets_busy',
  })
  await assert.rejects(service.remove(first.id), { code: 'game_assets_busy' })
  await service.run(second.id)
  await assert.rejects(service.run(third.id), { code: 'game_assets_busy' })
  const cancelled = await service.stop(job.id)
  assert.equal(cancelled.status, 'cancelled')
  assert.equal(cancelled.error, 'game_assets_cancelled')
  await service.run(third.id)
  await service.dispose()
  assert.ok((await service.catalog()).jobs.every((entry) => entry.status === 'cancelled'))
  await assert.rejects(service.run(first.id), { code: 'game_assets_closed' })
})

test('failed generation keeps prior actions and validates partial results without exposing upstream messages', async (t) => {
  let reference
  const { service, ...rest } = await fixture(t, (request) => {
    if (request.operation === 'generate' && request.settings.action === 'Walk') {
      throw Object.assign(new Error('token and personal path'), {
        code: 'untrusted_error',
        partialOutput: output([
          {
            media: reference,
            width: 16,
            height: 16,
            action: 'Walk',
            direction: 'S',
            durationMs: 125,
            columns: 2,
            rows: 1,
            frameCount: 2,
          },
        ]),
      })
    }
  })
  reference = rest.reference
  const project = await service.save(input(reference))
  const job = await completed(service, await service.run(project.id))
  assert.equal(job.status, 'failed')
  assert.equal(job.completed, 1)
  assert.equal(job.output.frames.length, 5)
  assert.equal(job.error, 'game_assets_processing_failed')
  assert.doesNotMatch(JSON.stringify(job), /token|personal|untrusted/)
  assert.ok(job.output.frames.some((frame) => frame.action === 'Idle'))
})

test('restart marks incomplete jobs interrupted without invoking image operations', async (t) => {
  const { root, service, media, operations, reference, calls } = await fixture(t)
  const project = await service.save(input(reference))
  const job = {
    id: randomUUID(),
    projectId: project.id,
    status: 'running',
    startedAt: new Date().toISOString(),
    finishedAt: null,
    completed: 0,
    total: 2,
    error: null,
    output: output(),
    originalOutput: output(),
    revision: 0,
  }
  await writeFile(
    join(root, 'game-assets.json'),
    JSON.stringify({ version: 1, projects: [project], jobs: [job] }),
  )
  const restored = new GameAssetsService({ dataDir: root, media, operations })
  await restored.init()
  assert.equal((await restored.getJob(job.id)).status, 'interrupted')
  assert.equal(calls.length, 0)
  assert.equal(
    JSON.parse(await readFile(join(root, 'game-assets.json'), 'utf8')).jobs[0].status,
    'interrupted',
  )
  assert.doesNotThrow(() =>
    parseGameAssetsCatalog({
      projects: [project],
      jobs: [{ ...job, status: 'interrupted', finishedAt: new Date().toISOString() }],
    }),
  )
  await restored.dispose()
})

test('failed atomic saves leave the published project unchanged', async (t) => {
  const { service, reference, root } = await fixture(t)
  const project = await service.save(input(reference))
  await rm(join(root, 'game-assets.json'))
  await mkdir(join(root, 'game-assets.json'))
  await assert.rejects(service.save({ ...input(reference, 'Modified'), id: project.id }), {
    code: 'game_assets_storage_failed',
  })
  assert.deepEqual((await service.catalog()).projects, [project])
})

test('manual frame editing replays original frames, supports copy/delete/reorder, and persists without generation', async (t) => {
  const { service, reference, root, media, operations, calls } = await fixture(t)
  const project = await service.save(input(reference))
  const generated = await completed(service, await service.run(project.id))
  const original = structuredClone(generated.output)
  const edits = {
    frames: [
      {
        sourceIndex: 6,
        durationMs: 200,
        x: 2,
        eraseStrokes: [{ points: [{ x: 0.2, y: 0.2 }], radius: 0.1 }],
      },
      { sourceIndex: 1, rotation: 45, opacity: 0.5 },
      { sourceIndex: 1, scale: 1.5 },
    ],
  }
  calls.length = 0
  const edited = await service.edit(generated.id, edits)
  assert.deepEqual(
    calls.map((call) => call.operation),
    ['edit', 'export'],
  )
  assert.deepEqual(calls[0].images, original.frames)
  assert.equal(edited.output.frames.length, 3)
  assert.equal(edited.output.frames[0].durationMs, 200)
  assert.equal(edited.revision, 1)
  assert.deepEqual(edited.originalOutput, original)
  const second = await service.edit(generated.id, { frames: [{ sourceIndex: 7 }] })
  assert.equal(second.revision, 2)
  assert.deepEqual(second.originalOutput, original)
  assert.deepEqual(calls[2].images, original.frames)
  await assert.rejects(service.edit(generated.id, { frames: [{ sourceIndex: 8 }] }), {
    code: 'game_assets_invalid',
  })
  assert.equal((await service.getJob(generated.id)).revision, 2)
  const restored = new GameAssetsService({ dataDir: root, media, operations })
  await restored.init()
  assert.deepEqual(await restored.getJob(generated.id), second)
  await restored.dispose()
  assert.deepEqual((await readdir(root)).sort(), ['game-assets.json', 'workflow-media'])
})

test('cancelled edits and failed export preserve previous results and immutable originals', async (t) => {
  let editing = false
  let failing = false
  const entered = deferred()
  const { service, reference } = await fixture(t, (request) => {
    if (editing && request.operation === 'edit') {
      entered.resolve()
      return new Promise((_, reject) =>
        request.signal.addEventListener('abort', () => reject(new Error('cancelled')), {
          once: true,
        }),
      )
    }
    if (failing && request.operation === 'export') throw new Error('private details')
  })
  const project = await service.save(input(reference))
  const original = await completed(service, await service.run(project.id))
  editing = true
  const pending = service.edit(original.id, { frames: [{ sourceIndex: 0 }] })
  const rejected = assert.rejects(pending, { code: 'game_assets_cancelled' })
  await entered.promise
  await assert.rejects(service.edit(original.id, { frames: [{ sourceIndex: 1 }] }), {
    code: 'game_assets_busy',
  })
  await assert.rejects(service.run(project.id), { code: 'game_assets_busy' })
  await service.stop(original.id)
  await rejected
  assert.deepEqual(await service.getJob(original.id), original)
  editing = false
  failing = true
  await assert.rejects(service.edit(original.id, { frames: [{ sourceIndex: 2 }] }), {
    code: 'game_assets_processing_failed',
  })
  assert.deepEqual(await service.getJob(original.id), original)
})

test('invalid stored data is rejected instead of being overwritten with an empty catalog', async (t) => {
  const { root, media, operations } = await fixture(t)
  for (const invalid of ['null', '{"version":2,"projects":[],"jobs":[]}', '{ private-data']) {
    await writeFile(join(root, 'game-assets.json'), invalid)
    const restored = new GameAssetsService({ dataDir: root, media, operations })
    await assert.rejects(restored.init(), {
      code: 'game_assets_storage_invalid',
      message: 'game_assets_storage_invalid',
    })
    await assert.doesNotReject(restored.dispose())
    assert.equal(await readFile(join(root, 'game-assets.json'), 'utf8'), invalid)
  }
})
