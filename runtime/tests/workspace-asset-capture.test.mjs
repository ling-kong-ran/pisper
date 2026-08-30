import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  captureWorkspaceAssetBaseline,
  listNewWorkspaceAssets,
} from '../services/workspace-asset-capture.mjs'

async function fixture(t) {
  const workspace = await mkdtemp(join(tmpdir(), 'pisper-workspace-assets-'))
  t.after(() => rm(workspace, { recursive: true, force: true }))
  return workspace
}

test('workspace asset capture returns only new user files', async (t) => {
  const workspace = await fixture(t)
  await Promise.all([
    writeFile(join(workspace, 'existing.md'), '# Existing'),
    mkdir(join(workspace, 'node_modules', 'fixture'), { recursive: true }),
    mkdir(join(workspace, 'dist'), { recursive: true }),
  ])
  await writeFile(join(workspace, 'node_modules', 'fixture', 'package.js'), 'ignored')
  const baseline = await captureWorkspaceAssetBaseline(workspace)

  await Promise.all([
    writeFile(join(workspace, 'existing.md'), '# Updated'),
    writeFile(join(workspace, 'report.csv'), 'name,value\nPisper,1\n'),
    writeFile(join(workspace, 'demo.mp4'), Buffer.from('video fixture')),
    writeFile(join(workspace, 'package-lock.json'), '{}'),
    writeFile(join(workspace, 'dist', 'bundle.js'), 'ignored'),
  ])

  const assets = await listNewWorkspaceAssets(baseline)
  assert.deepEqual(
    assets.map((asset) => asset.path).sort(),
    [join(workspace, 'demo.mp4'), join(workspace, 'report.csv')].sort(),
  )
})

test('workspace asset capture ignores hidden, temporary, and generated directories', async (t) => {
  const workspace = await fixture(t)
  const baseline = await captureWorkspaceAssetBaseline(workspace)
  await Promise.all([
    mkdir(join(workspace, '.cache'), { recursive: true }),
    mkdir(join(workspace, 'target'), { recursive: true }),
    mkdir(join(workspace, 'notes'), { recursive: true }),
  ])
  await Promise.all([
    writeFile(join(workspace, '.cache', 'cache.txt'), 'ignored'),
    writeFile(join(workspace, 'target', 'program.log'), 'ignored'),
    writeFile(join(workspace, '.secret'), 'ignored'),
    writeFile(join(workspace, 'notes', 'meeting.md'), '# Meeting'),
  ])

  const assets = await listNewWorkspaceAssets(baseline)
  assert.deepEqual(
    assets.map((asset) => asset.path),
    [join(workspace, 'notes', 'meeting.md')],
  )
})
