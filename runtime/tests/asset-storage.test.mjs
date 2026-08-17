import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { generatedAssetsForSession, reconcileAssetIndex } from '../services/asset-storage.mjs'

const PNG = Buffer.from(
  '89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082',
  'hex',
)

async function createFixture(t) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-assets-data-'))
  const workspace = await mkdtemp(join(tmpdir(), 'pisper-assets-workspace-'))
  const assetsDir = join(dataDir, 'pisper-assets')
  await mkdir(assetsDir, { recursive: true })
  t.after(async () => {
    await Promise.all([
      rm(dataDir, { recursive: true, force: true }),
      rm(workspace, { recursive: true, force: true }),
    ])
  })
  return {
    assetsDir,
    dataDir,
    workspace,
    runtime: new AgentRuntimeService({ cwd: workspace, dataDir }),
  }
}

test('generated assets are archived once and remain available without their workspace', async (t) => {
  const { assetsDir, dataDir, workspace, runtime } = await createFixture(t)
  const sourcePath = join(workspace, 'generated', 'visuals', 'image.png')
  const duplicatePath = join(workspace, 'generated', 'visuals', 'renamed.png')
  await mkdir(dirname(sourcePath), { recursive: true })
  await writeFile(sourcePath, PNG)
  await writeFile(duplicatePath, PNG)

  const created = await runtime.recordGeneratedFile(
    'session-1',
    { name: 'Generated image' },
    sourcePath,
  )
  const duplicate = await runtime.recordGeneratedFile(
    'session-2',
    { name: 'Another session' },
    duplicatePath,
  )
  const stored = runtime.findAsset(created.id)

  assert.ok(stored)
  assert.equal(duplicate.id, created.id)
  assert.equal(runtime.assetIndex.assets.length, 1)
  assert.equal((await readdir(assetsDir)).length, 1)
  assert.equal(stored.filePath, resolve(sourcePath))
  assert.notEqual(stored.storagePath, resolve(sourcePath))
  assert.deepEqual(await readFile(stored.storagePath), PNG)
  assert.equal(stored.references.length, 1)
  assert.equal(stored.references[0].filePath, resolve(duplicatePath))
  const persisted = JSON.parse(await readFile(join(dataDir, 'pisper-assets.json'), 'utf8'))
  assert.equal(persisted.assets.length, 1)
  assert.equal(persisted.assets[0].references.length, 1)
  assert.equal(runtime.streamProjection.generatedAssets('session-1').length, 1)
  assert.equal(runtime.streamProjection.generatedAssets('session-2')[0].name, 'renamed.png')

  await rm(workspace, { recursive: true, force: true })
  const content = await runtime.getAssetContent(stored.id)
  const download = await runtime.getAssetDownload(stored.id)
  assert.equal(content.kind, 'image')
  assert.equal(content.data, PNG.toString('base64'))
  assert.deepEqual(download.buffer, PNG)
})

test('uploaded files deduplicate by content across names and sources', async (t) => {
  const { assetsDir, runtime } = await createFixture(t)
  const bytes = Buffer.from('the same file content', 'utf8')
  const uploaded = await runtime.createAsset({
    name: 'notes.txt',
    text: bytes.toString('utf8'),
    source: 'upload',
    sessionId: 'session-1',
    sessionName: 'First session',
  })
  const attached = await runtime.createAsset({
    name: 'renamed.json',
    data: bytes.toString('base64'),
    mimeType: 'application/json',
    source: 'attachment',
    sessionId: 'session-2',
    sessionName: 'Second session',
  })

  assert.equal(attached.id, uploaded.id)
  assert.equal(runtime.assetIndex.assets.length, 1)
  assert.equal((await readdir(assetsDir)).length, 1)
  assert.equal('references' in attached, false)
  assert.deepEqual(
    (await runtime.listAssets({ sessionId: 'session-1' })).map((asset) => asset.id),
    [uploaded.id],
  )
  const currentAssets = await runtime.listAssets({ sessionId: 'session-2' })
  assert.deepEqual(
    currentAssets.map((asset) => asset.id),
    [uploaded.id],
  )
  assert.equal(currentAssets[0].name, 'notes.txt')
  assert.equal(currentAssets[0].source, 'attachment')
  assert.equal(currentAssets[0].sessionName, 'Second session')
})

test('asset operations wait for background legacy reconciliation', async (t) => {
  const { runtime, workspace } = await createFixture(t)
  const sourcePath = join(workspace, 'legacy.txt')
  await writeFile(sourcePath, 'legacy content')
  runtime.assetIndex = {
    assets: [
      {
        id: 'legacy',
        kind: 'file',
        name: 'legacy.txt',
        mimeType: 'text/plain',
        size: 0,
        filePath: sourcePath,
        source: 'agent',
        sessionId: 'session-1',
        sessionName: 'Legacy',
        created: '2026-01-01T00:00:00.000Z',
        modified: '2026-01-01T00:00:00.000Z',
      },
    ],
  }

  let releaseSave = () => {}
  let markSaveStarted = () => {}
  const saveGate = new Promise((resolve) => {
    releaseSave = resolve
  })
  const saveStarted = new Promise((resolve) => {
    markSaveStarted = resolve
  })
  runtime.saveAssetIndex = async () => {
    markSaveStarted()
    await saveGate
  }

  const reconciliation = runtime.startAssetReconciliation()
  assert.equal(runtime.findAsset('legacy').storagePath, undefined)
  await saveStarted
  let listSettled = false
  const listed = runtime.listAssets().then((assets) => {
    listSettled = true
    return assets
  })
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(listSettled, false)

  releaseSave()
  assert.equal(await reconciliation, true)
  assert.equal((await listed)[0].id, 'legacy')
  assert.match(runtime.findAsset('legacy').hash, /^[a-f0-9]{64}$/)
  assert.ok(runtime.findAsset('legacy').storagePath)
})

test('legacy reconciliation merges readable duplicates and removes unreadable records', async (t) => {
  const { assetsDir, workspace } = await createFixture(t)
  const currentStorage = join(assetsDir, 'current.png')
  const oldStorage = join(assetsDir, 'old.png')
  const oldSource = join(workspace, 'old-source.png')
  const uniqueSource = join(workspace, 'unique.txt')
  const referenceSource = join(workspace, 'reference-source.bin')
  const missingSource = join(workspace, 'missing.bin')
  const recoverable = Buffer.from('recoverable reference content')
  await Promise.all([
    writeFile(currentStorage, PNG),
    writeFile(oldStorage, PNG),
    writeFile(oldSource, PNG),
    writeFile(uniqueSource, 'unique legacy content'),
    writeFile(referenceSource, recoverable),
  ])

  const assets = [
    {
      id: 'current',
      kind: 'image',
      name: 'current.png',
      mimeType: 'image/png',
      size: 0,
      storagePath: currentStorage,
      source: 'upload',
      sessionId: 'session-1',
      sessionName: 'Current',
      created: '2026-03-01T00:00:00.000Z',
      modified: '2026-03-01T00:00:00.000Z',
    },
    {
      id: 'old-managed',
      kind: 'image',
      name: 'old.png',
      mimeType: 'image/png',
      size: PNG.length,
      storagePath: oldStorage,
      source: 'attachment',
      sessionId: 'session-2',
      sessionName: 'Old managed',
      created: '2026-02-01T00:00:00.000Z',
      modified: '2026-02-01T00:00:00.000Z',
    },
    {
      id: 'old-workspace',
      kind: 'image',
      name: 'old-source.png',
      mimeType: 'image/png',
      size: PNG.length,
      filePath: oldSource,
      source: 'agent',
      sessionId: 'session-3',
      sessionName: 'Old workspace',
      created: '2026-01-01T00:00:00.000Z',
      modified: '2026-01-01T00:00:00.000Z',
    },
    {
      id: 'unique',
      kind: 'file',
      name: 'unique.txt',
      mimeType: 'text/plain',
      size: 0,
      filePath: uniqueSource,
      source: 'agent',
      sessionId: 'session-4',
      sessionName: 'Unique',
      created: '2025-12-01T00:00:00.000Z',
      modified: '2025-12-01T00:00:00.000Z',
    },
    {
      id: 'recovered',
      kind: 'file',
      name: 'recovered.bin',
      mimeType: 'application/octet-stream',
      size: 0,
      filePath: missingSource,
      source: 'agent',
      sessionId: 'session-5',
      sessionName: 'Recovered',
      created: '2025-11-01T00:00:00.000Z',
      modified: '2025-11-01T00:00:00.000Z',
      references: [
        {
          source: 'agent',
          sessionId: 'session-6',
          sessionName: 'Reference',
          name: 'reference-source.bin',
          kind: 'file',
          mimeType: 'application/octet-stream',
          size: recoverable.length,
          filePath: referenceSource,
          created: '2025-10-01T00:00:00.000Z',
        },
      ],
    },
    {
      id: 'missing',
      kind: 'file',
      name: 'missing.bin',
      mimeType: 'application/octet-stream',
      size: 1,
      hash: 'f'.repeat(64),
      filePath: missingSource,
      source: 'agent',
      sessionId: 'session-7',
      sessionName: 'Missing',
      created: '2025-09-01T00:00:00.000Z',
      modified: '2025-09-01T00:00:00.000Z',
    },
    {
      id: 'link',
      kind: 'link',
      name: 'Project site',
      url: 'https://ling-kong-ran.github.io/pisper/',
      mimeType: 'text/uri-list',
      size: 0,
      source: 'upload',
      sessionId: '',
      sessionName: '',
      created: '2025-08-01T00:00:00.000Z',
      modified: '2025-08-01T00:00:00.000Z',
    },
  ]
  let saves = 0
  const reconcile = () =>
    reconcileAssetIndex({
      assets,
      assetsDir,
      save: async () => {
        saves += 1
      },
    })

  assert.equal(await reconcile(), true)
  assert.equal(saves, 1)
  assert.deepEqual(
    assets.map((asset) => asset.id),
    ['current', 'unique', 'recovered', 'link'],
  )
  assert.match(assets[0].hash, /^[a-f0-9]{64}$/)
  assert.equal(assets[0].size, PNG.length)
  assert.equal(assets[0].storagePath, resolve(currentStorage))
  assert.deepEqual(
    assets[0].references.map((reference) => reference.sessionId),
    ['session-2', 'session-3'],
  )
  assert.equal(generatedAssetsForSession(assets, 'session-3')[0].name, 'old-source.png')
  assert.deepEqual(await readFile(oldSource), PNG)
  assert.equal((await readdir(assetsDir)).includes('old.png'), false)

  const unique = assets.find((asset) => asset.id === 'unique')
  const recovered = assets.find((asset) => asset.id === 'recovered')
  assert.match(unique.hash, /^[a-f0-9]{64}$/)
  assert.notEqual(unique.storagePath, resolve(uniqueSource))
  assert.deepEqual(await readFile(unique.storagePath), Buffer.from('unique legacy content'))
  assert.match(recovered.hash, /^[a-f0-9]{64}$/)
  assert.notEqual(recovered.storagePath, resolve(referenceSource))
  assert.deepEqual(await readFile(recovered.storagePath), recoverable)
  assert.equal(
    assets.some((asset) => asset.id === 'missing'),
    false,
  )
  assert.equal(
    assets.find((asset) => asset.id === 'link').url,
    'https://ling-kong-ran.github.io/pisper/',
  )
  assert.equal(await reconcile(), false)
  assert.equal(saves, 1)
})
