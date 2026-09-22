import assert from 'node:assert/strict'
import { mkdtemp, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { isAppOwnedPath } from '../../scripts/app-paths.mjs'
import { releaseComponentsForPath } from '../../scripts/release-changes.mjs'
import {
  searchToolEntries,
  searchToolCriticalEntries,
  stageSearchTools,
} from '../../scripts/stage-search-tools.mjs'

test('offline packaging changes reach Runtime and desktop releases, with shared patches reaching mobile', () => {
  for (const path of [
    'scripts/search-tool-resources.json',
    'scripts/stage-search-tools.mjs',
    'scripts/stage-runtime-closure.mjs',
    'scripts/patch-pi-offline-compat.mjs',
  ]) {
    assert.deepEqual(releaseComponentsForPath(path), ['desktop', 'runtime'])
  }
  assert.equal(isAppOwnedPath('scripts/patch-pi-offline-compat.mjs'), true)
  assert.equal(isAppOwnedPath('scripts/stage-search-tools.mjs'), false)
})

test('every shipped desktop target pins both local search tools and their licenses', () => {
  for (const [platform, arch] of [
    ['win32', 'x64'],
    ['darwin', 'x64'],
    ['darwin', 'arm64'],
    ['linux', 'x64'],
  ]) {
    const target = { platform, arch }
    const tools = searchToolEntries(target)
    assert.deepEqual(
      tools.map((tool) => tool.name),
      ['rg', 'fd'],
    )
    for (const tool of tools) {
      assert.match(tool.asset.sha256, /^[a-f0-9]{64}$/)
      assert.ok(tool.asset.url.startsWith(`${tool.source}/releases/download/`))
      assert.equal(tool.asset.url.includes('/latest/'), false)
      assert.ok(tool.licenseFiles.length > 0)
      assert.ok(
        searchToolCriticalEntries(target).some((entry) => entry.path.endsWith(`/${tool.filename}`)),
      )
    }
  }
  assert.throws(() => searchToolEntries({ platform: 'win32', arch: 'unsupported' }), /Unsupported/)
})

test('mobile staging never downloads or ships desktop search binaries', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-mobile-search-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await stageSearchTools({
    root,
    runtimeDir: root,
    target: { platform: 'mobile', arch: 'arm64' },
    fetchImpl: () => {
      throw new Error('unexpected download')
    },
  })
  assert.deepEqual(await readdir(root), [])
  assert.deepEqual(searchToolCriticalEntries({ platform: 'mobile' }), [])
})

test('a tampered search archive fails the build before extraction', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-tampered-search-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await assert.rejects(
    stageSearchTools({
      root,
      runtimeDir: join(root, 'runtime'),
      target: { platform: 'win32', arch: 'x64' },
      fetchImpl: async () => new Response('tampered archive'),
    }),
    /checksum mismatch/,
  )
  assert.deepEqual(
    await readdir(
      join(root, 'runtime', 'node_modules', '@earendil-works', 'pi-coding-agent', 'vendor', 'bin'),
    ),
    [],
  )
})
