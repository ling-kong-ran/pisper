import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

const PNG = Buffer.from(
  '89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082',
  'hex',
)

test('generated assets are archived independently from their workspace file', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-assets-data-'))
  const workspace = await mkdtemp(join(tmpdir(), 'pisper-assets-workspace-'))
  t.after(async () => {
    await Promise.all([
      rm(dataDir, { recursive: true, force: true }),
      rm(workspace, { recursive: true, force: true }),
    ])
  })

  const sourcePath = join(workspace, 'generated', 'visuals', 'image.png')
  await mkdir(dirname(sourcePath), { recursive: true })
  await mkdir(join(dataDir, 'pisper-assets'), { recursive: true })
  await writeFile(sourcePath, PNG)

  const runtime = new AgentRuntimeService({ cwd: workspace, dataDir })
  const created = await runtime.recordGeneratedFile(
    'session-1',
    { name: 'Generated image' },
    sourcePath,
  )
  const stored = runtime.findAsset(created.id)

  assert.ok(stored)
  assert.equal(stored.filePath, resolve(sourcePath))
  assert.notEqual(stored.storagePath, resolve(sourcePath))
  assert.deepEqual(await readFile(stored.storagePath), PNG)

  await rm(workspace, { recursive: true, force: true })
  const content = await runtime.getAssetContent(stored.id)
  const download = await runtime.getAssetDownload(stored.id)
  assert.equal(content.kind, 'image')
  assert.equal(content.data, PNG.toString('base64'))
  assert.deepEqual(download.buffer, PNG)
})
