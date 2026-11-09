// 本机发布（scripts/release-local.mjs）：GitHub 接口的重试、覆盖上传与下载跳转，
// 以及它依赖的 release.mjs 计划模式和 fetch-bundled-components 的本地归档来源。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { load as parseYaml } from 'js-yaml'
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
  // publish 推送后中途失败可以重跑：标签都已在远端时跳过提交与推送，计划文件最后才删。
  assert.match(local, /findPlan\(api, \{ resume: true \}\)/)
  assert.match(local, /if \(!pushed\)\s+git\(\[\s+'push'/)
  assert.ok(
    local.indexOf('await publishNpm(current') < local.lastIndexOf('api.deleteAsset(planAsset.id)'),
  )
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

test('self-hosted Windows releases inject secrets and can resume existing drafts', async () => {
  const source = await readFile('.github/workflows/release-self-hosted.yml', 'utf8')
  const workflow = parseYaml(source)
  assert.deepEqual(Object.keys(workflow.on), ['workflow_dispatch'])
  assert.deepEqual(workflow.permissions, { contents: 'write' })

  const inputs = workflow.on.workflow_dispatch.inputs
  assert.deepEqual(inputs.stage.options, ['all', 'plan', 'build', 'status', 'publish'])
  assert.equal(inputs.stage.default, 'all')
  assert.equal(inputs.version.default, 'patch')

  const jobs = Object.values(workflow.jobs)
  assert.equal(jobs.length, 1)
  const [job] = jobs
  // 必须同时匹配 self-hosted 和仓库自定义标签，不能回退到 GitHub 托管 Runner。
  assert.deepEqual(job['runs-on'], ['self-hosted', 'pisper-windows'])
  assert.equal(job.env.GITHUB_TOKEN, '${{ github.token }}')
  assert.equal(job.env.GH_TOKEN, '${{ github.token }}')

  const step = (name) => {
    const found = job.steps.find((entry) => entry.name === name)
    assert.ok(found, `missing workflow step: ${name}`)
    return found
  }
  assert.equal(step('Require Windows x64').shell, 'powershell')
  assert.match(step('Require Windows x64').run, /RUNNER_OS.*Windows.*RUNNER_ARCH.*X64/s)
  assert.equal(
    job.steps.find((entry) => entry.uses === 'actions/setup-node@v5').with['node-version'],
    24,
  )
  assert.ok(job.steps.some((entry) => entry.uses === 'dtolnay/rust-toolchain@stable'))
  assert.equal(step('Install Node dependencies').run, 'npm ci')

  const plan = step('Plan Windows-only release')
  assert.equal(plan.if, "inputs.stage == 'all' || inputs.stage == 'plan'")
  assert.equal(plan.shell, 'powershell')
  assert.match(plan.run, /npm run release:local -- plan \$env:RELEASE_VERSION --platforms=windows/)
  // 手动输入只经环境变量进入脚本，先校验再使用。
  assert.equal(plan.env.RELEASE_VERSION, '${{ inputs.version }}')
  assert.doesNotMatch(plan.run, /\$\{\{/)
  assert.match(plan.run, /-cnotmatch '\^\(patch\|minor\|major\|/)

  const build = step('Build and upload Windows artifacts')
  assert.equal(build.if, "inputs.stage == 'all' || inputs.stage == 'build'")
  // plan 可能刚升级依赖，build 必须自己重新 npm ci。
  assert.equal(build.run, 'npm run release:local -- build')
  assert.equal(build.env.TAURI_SIGNING_PRIVATE_KEY, '${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}')
  assert.equal(
    build.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD,
    '${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}',
  )
  assert.match(step('Inspect draft release assets').if, /inputs\.stage == 'publish'/)

  const publish = step('Publish validated draft releases')
  assert.equal(publish.if, "inputs.stage == 'all' || inputs.stage == 'publish'")
  assert.equal(publish.run, 'npm run release:local -- publish')

  assert.doesNotMatch(source, /run:.*\$\{\{\s*inputs\./)

  // Windows-only 计划没有 npm；入口只能复用 release-local，不能走 release.mjs 的 Actions 派发。
  assert.doesNotMatch(source, /\bNPM_TOKEN\b|--skip-checks|gh workflow run|scripts\/release\.mjs/)
  assert.doesNotMatch(source, /npm run release(?!:local)/)
})
