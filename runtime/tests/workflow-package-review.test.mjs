import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdtemp, mkdir, readdir, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { WorkflowMediaService } from '../services/workflow-media-service.mjs'
import { SpriteEngineService } from '../services/sprite-engine-service.mjs'
import { WorkflowService } from '../services/workflow-service.mjs'
import { workflowScheduleRoutes } from '../http/routes/workflows-schedules.mjs'
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
const input = (name, media) => ({ name, label: name, type: 'image', defaultValue: media })
const definition = (media) => ({
  format: 'pisper-workflow',
  version: 1,
  workflow: {
    name: 'Portable',
    model: null,
    nodes: [{ id: 'node', kind: 'prompt', prompt: 'Inspect' }],
    edges: [],
    inputs: [input('image', media)],
    notifications: [],
  },
})

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-package-review-'))
  const source = new WorkflowMediaService({ dataDir: join(root, 'source') })
  const destination = new WorkflowMediaService({ dataDir: join(root, 'destination') })
  const services = []
  await destination.init()
  t.after(async () => {
    for (const service of services) await service.dispose()
    await source.dispose()
    await destination.dispose()
    await rm(root, { recursive: true, force: true })
  })
  const media = await source.upload({ name: 'image.png', mimeType: 'image/png', buffer: PNG })
  const files = decodeWorkflowBundle(await exportPortableWorkflowBundle(definition(media), source))
  return { root, source, destination, media, files, services }
}

async function routeFixture(t) {
  const fixtureData = await fixture(t)
  const workflows = new WorkflowService({
    path: join(fixtureData.root, 'workflows.json'),
    cwd: fixtureData.root,
    agent: { validateDirectory: async (value) => value },
    notifications: {},
  })
  fixtureData.services.push(workflows)
  await workflows.init()
  const route = workflowScheduleRoutes.find(
    ({ method, path }) => method === 'POST' && path === '/api/workflows/import-bundle',
  )
  assert.ok(route)
  const context = {
    runtime: {
      workflowMedia: fixtureData.destination,
      workflows,
      getWorkflows() {
        throw new Error('Directory projection is unavailable')
      },
      importWorkflow() {
        throw new Error('Import must use the domain commit, not the directory facade')
      },
    },
    bodyBuffer: async () => encodeWorkflowBundle(fixtureData.files),
  }
  return { ...fixtureData, workflows, route, context }
}

test('ZIP import commits its workflow and media without requiring a directory projection', async (t) => {
  const { workflows, destination, route, context } = await routeFixture(t)
  let response
  await route.handler({
    ...context,
    json(status, body) {
      response = { status, body }
    },
  })
  assert.equal(response.status, 201)
  const [workflow] = workflows.getState().workflows
  assert.equal(response.body.workflow.id, workflow.id)
  const importedMedia = workflow.inputs[0].defaultValue
  assert.deepEqual((await destination.load(importedMedia.id)).buffer, PNG)
  const persisted = JSON.parse(await readFile(workflows.path, 'utf8'))
  assert.equal(persisted.workflows[0].inputs[0].defaultValue.id, importedMedia.id)
})

test('ZIP response failure does not roll back media after the workflow commit succeeds', async (t) => {
  const { workflows, destination, route, context } = await routeFixture(t)
  await assert.rejects(
    route.handler({
      ...context,
      json() {
        throw new Error('fixture response failure')
      },
    }),
    /fixture response failure/,
  )
  const [workflow] = workflows.getState().workflows
  assert.deepEqual((await destination.load(workflow.inputs[0].defaultValue.id)).buffer, PNG)
  assert.equal(JSON.parse(await readFile(workflows.path, 'utf8')).workflows[0].id, workflow.id)
})

test('ZIP commit failure removes new media and the failed in-memory workflow', async (t) => {
  const { root, workflows, destination, route, context } = await routeFixture(t)
  const storedPath = workflows.path
  const blockedPath = join(root, 'blocked-destination')
  await mkdir(blockedPath)
  workflows.path = blockedPath
  try {
    await assert.rejects(
      route.handler({
        ...context,
        json() {
          assert.fail('A failed commit must not send a success response')
        },
      }),
    )
    assert.deepEqual(workflows.getState().workflows, [])
    assert.deepEqual(await readdir(destination.root), [])
    assert.deepEqual(JSON.parse(await readFile(storedPath, 'utf8')).workflows, [])
  } finally {
    workflows.path = storedPath
  }
})

for (const firstFails of [true, false]) {
  test(`concurrent creates persist only the committed workflow when the ${firstFails ? 'first' : 'second'} write fails`, async (t) => {
    const { root, workflows } = await routeFixture(t)
    const storedPath = workflows.path
    const blockedPath = join(root, 'blocked-concurrent-destination')
    await mkdir(blockedPath)
    workflows.path = firstFails ? blockedPath : storedPath
    let releaseWrites
    workflows.writeQueue = new Promise((resolve) => {
      releaseWrites = resolve
    })
    let queued = 0
    let queuedBoth
    const ready = new Promise((resolve) => {
      queuedBoth = resolve
    })
    const save = workflows.save.bind(workflows)
    workflows.save = (...args) => {
      const pending = save(...args)
      if (++queued === 2) queuedBoth()
      return pending
    }
    const first = workflows.create({ name: 'First import' }).finally(() => {
      workflows.path = firstFails ? storedPath : blockedPath
    })
    const second = workflows.create({ name: 'Second import' })
    const settled = Promise.allSettled([first, second])
    try {
      await ready
      releaseWrites()
      const results = await settled
      assert.equal(results[firstFails ? 0 : 1].status, 'rejected')
      const success = results[firstFails ? 1 : 0]
      assert.equal(success.status, 'fulfilled')
      assert.deepEqual(
        workflows.getState().workflows.map(({ id }) => id),
        [success.value.id],
      )
      assert.deepEqual(
        JSON.parse(await readFile(storedPath, 'utf8')).workflows.map(({ id }) => id),
        [success.value.id],
      )
    } finally {
      releaseWrites()
      await settled
      workflows.path = storedPath
      workflows.save = save
    }
  })
}

test('invalid model dependency fails before any imported media is installed', async (t) => {
  const { destination, files } = await fixture(t)
  const workflow = bundleJson(files, 'workflow.json')
  workflow.workflow.model = { provider: 'invalid-without-model' }
  files['workflow.json'] = jsonBundleFile(workflow)
  await assert.rejects(importPortableWorkflowBundle(encodeWorkflowBundle(files), destination), {
    code: 'workflow_bundle_invalid',
  })
  assert.deepEqual(await readdir(destination.root), [])
})

test('every default reference is checked even when multiple inputs share the same media id', async (t) => {
  const { destination, media, files } = await fixture(t)
  const workflow = bundleJson(files, 'workflow.json')
  workflow.workflow.inputs = [
    input('forged', { ...media, name: 'different.png' }),
    input('correct', media),
  ]
  files['workflow.json'] = jsonBundleFile(workflow)
  await assert.rejects(importPortableWorkflowBundle(encodeWorkflowBundle(files), destination), {
    code: 'workflow_bundle_invalid',
  })
  assert.deepEqual(await readdir(destination.root), [])
})

test('media import removes earlier writes when a later asset cannot be written', async (t) => {
  const { source, destination, media } = await fixture(t)
  const second = await source.upload({ name: 'second.png', mimeType: 'image/png', buffer: PNG })
  const files = await source.exportFilesForWorkflow({
    inputs: [input('first', media), input('second', second)],
  })
  const write = destination.write.bind(destination)
  let writes = 0
  destination.write = async (...args) => {
    if (++writes === 2) throw new Error('fixture storage failure')
    return write(...args)
  }
  await assert.rejects(destination.importBundleFiles(files), /fixture storage failure/)
  assert.deepEqual(await readdir(destination.root), [])
})

test('media export checks its aggregate limit before reading another oversized contribution', async (t) => {
  const { source } = await fixture(t)
  let reads = 0
  const media = (id) => ({ id, name: `${id}.mp4`, mimeType: 'video/mp4', size: 64 * 1024 * 1024 })
  source.load = async (id) => {
    reads++
    return {
      metadata: { version: 1, media: media(id), sha256: '0'.repeat(64) },
      buffer: Buffer.alloc(0),
      path: '',
    }
  }
  const inputs = ['first', 'second', 'third'].map((id) => ({
    name: id,
    label: id,
    type: 'video',
    defaultValue: media(id),
  }))
  await assert.rejects(source.exportFilesForWorkflow({ inputs }), {
    code: 'workflow_media_too_large',
  })
  assert.equal(reads, 2)
})

test('portable export includes only model identifiers from the persisted workflow, not provider credentials', async (t) => {
  const { root, source } = await fixture(t)
  const workflows = new WorkflowService({
    path: join(root, 'workflows.json'),
    cwd: root,
    agent: { validateDirectory: async (value) => value },
    notifications: {},
  })
  await workflows.init()
  try {
    const workflow = await workflows.create({
      name: 'Credentials boundary',
      apiKey: 'fixture-secret',
      model: { provider: 'local', model: 'vision', apiKey: 'fixture-secret' },
      nodes: [
        {
          id: 'first',
          kind: 'prompt',
          prompt: 'Preserve this task',
          credentials: { token: 'fixture-secret' },
          model: { provider: 'local', model: 'vision', apiKey: 'fixture-secret' },
        },
      ],
    })
    const files = decodeWorkflowBundle(
      await exportPortableWorkflowBundle(workflows.exportWorkflow(workflow.id), source),
    )
    assert.ok(!JSON.stringify(bundleJson(files, 'workflow.json')).includes('fixture-secret'))
    assert.equal(bundleJson(files, 'workflow.json').workflow.cwd, '')
    assert.equal(bundleJson(files, 'workflow.json').workflow.nodes[0].prompt, 'Preserve this task')
  } finally {
    await workflows.dispose()
  }
})

test('offline engine transaction rolls back new suites after a publish failure and preserves existing suites', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-engine-transaction-'))
  const bytes = Buffer.from('pinned fixture bytes')
  const definitions = ['background', 'inpaint'].map((id) => ({
    id,
    name: id,
    version: '1',
    licenses: [],
    files: [
      {
        name: 'runtime.js',
        url: 'https://example.invalid/runtime.js',
        bytes: bytes.length,
        sha256: createHash('sha256').update(bytes).digest('hex'),
        mimeType: 'text/javascript',
      },
    ],
  }))
  const service = new SpriteEngineService({ dataDir: root, definitions })
  await service.init()
  t.after(async () => {
    await service.dispose()
    await rm(root, { recursive: true, force: true })
  })
  const files = Object.fromEntries(definitions.map(({ id }) => [`engines/${id}/runtime.js`, bytes]))
  const blocked = join(root, 'sprite-engines', 'inpaint', 'installed.json')
  await mkdir(blocked)
  await assert.rejects(service.installBundleFiles(files), { code: 'sprite_engine_storage_unsafe' })
  assert.ok((await service.catalog()).engines.every(({ status }) => status === 'missing'))
  assert.deepEqual(await readdir(join(root, 'sprite-engines', 'background')), [])
  assert.deepEqual(await readdir(join(root, 'sprite-engines', 'inpaint')), ['installed.json'])
  await service.installBundleFiles({ 'engines/background/runtime.js': bytes })
  const existing = await readFile(
    join(root, 'sprite-engines', 'background', 'installed.json'),
    'utf8',
  )
  await assert.rejects(service.installBundleFiles(files), { code: 'sprite_engine_storage_unsafe' })
  assert.equal(
    await readFile(join(root, 'sprite-engines', 'background', 'installed.json'), 'utf8'),
    existing,
  )
  assert.deepEqual((await service.file('background', 'runtime.js')).buffer, bytes)
})
