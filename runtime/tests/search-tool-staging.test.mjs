import assert from 'node:assert/strict'
import { mkdtemp, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { isAppOwnedPath } from '../../scripts/app-paths.mjs'
import { releaseComponentsForPath } from '../../scripts/release-changes.mjs'
import {
  searchArchiveCommand,
  searchToolEntries,
  searchToolCriticalEntries,
  stageSearchTools,
} from '../../scripts/stage-search-tools.mjs'

test('Windows archive extraction uses system bsdtar even when Git tar precedes it on PATH', () => {
  assert.equal(
    searchArchiveCommand('win32', { SystemRoot: 'D:\\Windows', PATH: 'C:\\Git\\usr\\bin' }),
    'D:\\Windows\\System32\\tar.exe',
  )
  assert.equal(
    searchArchiveCommand('win32', { WINDIR: 'E:\\Windows' }),
    'E:\\Windows\\System32\\tar.exe',
  )
  assert.equal(searchArchiveCommand('win32', {}), 'C:\\Windows\\System32\\tar.exe')
  assert.equal(searchArchiveCommand('darwin', {}), 'tar')
})

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

// 下载源偶尔连续回 5xx：每轮先走原地址、再经 GitHub API 下同一个文件，都是暂时性失败才重试；4xx 不重试。
test('search tool downloads fall back to the GitHub API and retry server errors', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-retry-search-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const stage = (fetchImpl) =>
    stageSearchTools({
      root,
      runtimeDir: join(root, 'runtime'),
      target: { platform: 'win32', arch: 'x64' },
      fetchImpl,
      retryDelayMs: 1,
    })
  const urls = []
  const releaseLookup = (url) =>
    url.startsWith('https://api.github.com/repos/') && url.includes('/releases/tags/')
  // 原地址一直 504：经 API 查到资源 id 并下载（内容不对，摘要不符失败，说明真的走到了 API 下载）。
  await assert.rejects(
    stage(async (url) => {
      urls.push(url)
      if (releaseLookup(url))
        return Response.json({
          assets: [{ id: 42, name: 'ripgrep-15.2.0-x86_64-pc-windows-msvc.zip' }],
        })
      if (url.endsWith('/releases/assets/42')) return new Response('tampered archive')
      return new Response('busy', { status: 504 })
    }),
    /checksum mismatch/,
  )
  assert.deepEqual(urls, [
    'https://github.com/BurntSushi/ripgrep/releases/download/15.2.0/ripgrep-15.2.0-x86_64-pc-windows-msvc.zip',
    'https://api.github.com/repos/BurntSushi/ripgrep/releases/tags/15.2.0',
    'https://api.github.com/repos/BurntSushi/ripgrep/releases/assets/42',
  ])
  // 两条路都断：重试到 5 轮为止，报原地址的错。
  let calls = 0
  await assert.rejects(
    stage(async () => {
      calls += 1
      throw new TypeError('fetch failed')
    }),
    /download failed: fetch failed/,
  )
  assert.equal(calls, 10)
  calls = 0
  await assert.rejects(
    stage(async () => {
      calls += 1
      return new Response('missing', { status: 404 })
    }),
    /HTTP 404/,
  )
  assert.equal(calls, 1)
})
