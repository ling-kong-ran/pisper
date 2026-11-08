// 本机发布（scripts/release-local.mjs）：GitHub 接口的重试、覆盖上传与下载跳转，
// 以及它依赖的 release.mjs 计划模式和 fetch-bundled-components 的本地归档来源。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createGitHubReleases } from '../../scripts/github-release-api.mjs'

function fakeFetch(handler) {
  const calls = []
  const fetchImpl = async (url, init = {}) => {
    calls.push({ url, method: init.method || 'GET', headers: init.headers || {}, body: init.body })
    return handler(url, init, calls.length)
  }
  return { calls, fetchImpl }
}

test('a re-uploaded asset replaces the earlier one with the same name', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-release-local-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const file = join(directory, 'Pisper_TUI_1.0.0_linux_x86_64.tar.gz')
  await writeFile(file, 'archive')
  const { calls, fetchImpl } = fakeFetch(async (url) =>
    url.includes('uploads.github.com')
      ? new Response(JSON.stringify({ id: 9 }), { status: 201 })
      : new Response(null, { status: 204 }),
  )
  const api = createGitHubReleases({ token: 'secret', fetchImpl })
  await api.uploadAsset(42, file, [{ id: 7, name: 'Pisper_TUI_1.0.0_linux_x86_64.tar.gz' }])
  assert.equal(calls[0].method, 'DELETE')
  assert.match(calls[0].url, /releases\/assets\/7$/)
  assert.match(calls[1].url, /^https:\/\/uploads\.github\.com\/.*releases\/42\/assets\?name=/)
  assert.equal(calls[1].headers.Authorization, 'Bearer secret')
})

test('server errors are retried and asset downloads drop the token after the redirect', async () => {
  const { calls, fetchImpl } = fakeFetch(async (url, _init, count) => {
    if (url.includes('/releases?')) {
      return count === 1
        ? new Response('busy', { status: 502 })
        : new Response(JSON.stringify([{ id: 1, draft: true }]), { status: 200 })
    }
    if (url.includes('/releases/assets/5'))
      return new Response(null, { status: 302, headers: { location: 'https://objects.example/x' } })
    return new Response('bytes', { status: 200 })
  })
  const api = createGitHubReleases({ token: 'secret', fetchImpl })
  assert.deepEqual(await api.listReleases(), [{ id: 1, draft: true }])
  assert.equal((await api.downloadAsset(5)).toString(), 'bytes')
  const redirected = calls.at(-1)
  assert.equal(redirected.url, 'https://objects.example/x')
  assert.equal(redirected.headers.Authorization, undefined)
})

test('release.mjs can stop after planning, and desktop builds can bundle local archives', async () => {
  const release = await readFile('scripts/release.mjs', 'utf8')
  // 写出计划就退出，排在派发 workflow 之前。
  assert.ok(release.indexOf('if (planOutput) {') < release.indexOf('function dispatchWorkflow('))
  assert.match(release, /if \(!skipDependencyUpdate\) updateUpstreamPiDependency\(\)/)
  const bundler = await readFile('scripts/fetch-bundled-components.mjs', 'utf8')
  // 本地归档同样验签。
  assert.match(
    bundler,
    /localFound[\s\S]*await verifyArchive\(component, archiveBytes, signatureBytes\)/,
  )
  const local = await readFile('scripts/release-local.mjs', 'utf8')
  assert.match(local, /'--local=release\/component-artifacts'/)
  assert.match(local, /scripts\/verify-release-head\.mjs/)
  assert.doesNotMatch(local, /console\.log\([^)]*(TOKEN|PRIVATE_KEY|PASSWORD)/)
})

test('a release that skips Linux validates only the planned platforms', async (t) => {
  const { execFileSync } = await import('node:child_process')
  const directory = await mkdtemp(join(tmpdir(), 'pisper-release-platforms-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const platforms = ['darwin_aarch64', 'windows_x86_64']
  for (const platform of platforms)
    for (const name of [
      `Pisper_TUI_9.9.9_${platform}.tar.gz`,
      `Pisper_TUI_Component_9.9.9_${platform}.tar.gz`,
    ]) {
      await writeFile(join(directory, name), 'x')
      await writeFile(join(directory, `${name}.sig`), 'x')
    }
  const validate = (...extra) =>
    execFileSync(
      process.execPath,
      ['scripts/validate-component-release-assets.mjs', 'tui', 'tui-v9.9.9', directory, ...extra],
      { stdio: 'pipe' },
    )
  // 不带 --platforms 时仍要求三个平台齐全（CI 的行为不变）。
  assert.throws(() => validate(), /Missing release assets: .*linux_x86_64/)
  validate(`--platforms=${platforms.join(',')}`)
})
