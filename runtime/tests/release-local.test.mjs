// 本机发布（scripts/release-local.mjs）：GitHub 接口的重试、覆盖上传与下载跳转，
// 以及它依赖的 release.mjs 计划模式和 fetch-bundled-components 的本地归档来源。
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { load as parseYaml } from 'js-yaml'
import { createGitHubReleases } from '../../scripts/github-release-api.mjs'

function fakeFetch(handler) {
  const calls = []
  const fetchImpl = async (url, init = {}) => {
    calls.push({
      url,
      method: init.method || 'GET',
      headers: init.headers || {},
      body: init.body,
      dispatcher: init.dispatcher,
    })
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

test('an asset completed before an upload timeout is reused instead of uploaded twice', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-release-local-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const name = 'Pisper_TUI_1.0.0_linux_x86_64.tar.gz'
  const file = join(directory, name)
  await writeFile(file, 'archive')
  const digest = `sha256:${createHash('sha256').update('archive').digest('hex')}`
  const { calls, fetchImpl } = fakeFetch(async (url) => {
    if (url.includes('uploads.github.com')) {
      throw Object.assign(new TypeError('fetch failed'), {
        cause: { code: 'UND_ERR_HEADERS_TIMEOUT' },
      })
    }
    return new Response(JSON.stringify([{ id: 7, name, state: 'uploaded', size: 7, digest }]), {
      status: 200,
    })
  })
  const api = createGitHubReleases({ token: 'secret', fetchImpl, sleepImpl: async () => {} })
  const uploaded = await api.uploadAsset(42, file)
  assert.equal(uploaded.id, 7)
  assert.equal(calls.length, 2)
  assert.equal(calls.filter((call) => call.method === 'POST').length, 1)
})

test('an upload network failure removes an incomplete asset and retries', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-release-local-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const name = 'Pisper_TUI_1.0.0_linux_x86_64.tar.gz'
  const file = join(directory, name)
  await writeFile(file, 'archive')
  let uploads = 0
  const { calls, fetchImpl } = fakeFetch(async (url, init) => {
    if (url.includes('uploads.github.com')) {
      uploads += 1
      if (uploads === 1) throw new TypeError('fetch failed')
      return new Response(JSON.stringify({ id: 9, name, state: 'uploaded' }), { status: 201 })
    }
    if (init.method === 'DELETE') return new Response(null, { status: 204 })
    return new Response(JSON.stringify([{ id: 8, name, state: 'starter', size: 0 }]), {
      status: 200,
    })
  })
  const api = createGitHubReleases({ token: 'secret', fetchImpl, sleepImpl: async () => {} })
  const uploaded = await api.uploadAsset(42, file)
  assert.equal(uploaded.id, 9)
  assert.deepEqual(
    calls.map((call) => call.method),
    ['POST', 'GET', 'DELETE', 'POST'],
  )
})

test('a duplicate response after an upload timeout is reconciled before failing', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-release-local-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const name = 'Pisper_TUI_1.0.0_linux_x86_64.tar.gz'
  const file = join(directory, name)
  await writeFile(file, 'archive')
  let uploads = 0
  let listings = 0
  const { calls, fetchImpl } = fakeFetch(async (url) => {
    if (url.includes('uploads.github.com')) {
      uploads += 1
      if (uploads === 1) throw new TypeError('fetch failed')
      return new Response(JSON.stringify({ message: 'already_exists' }), { status: 422 })
    }
    listings += 1
    return new Response(
      JSON.stringify(listings === 1 ? [] : [{ id: 10, name, state: 'uploaded', size: 7 }]),
      { status: 200 },
    )
  })
  const api = createGitHubReleases({ token: 'secret', fetchImpl, sleepImpl: async () => {} })
  const uploaded = await api.uploadAsset(42, file)
  assert.equal(uploaded.id, 10)
  assert.equal(uploads, 2)
  assert.equal(calls.filter((call) => call.method === 'DELETE').length, 0)
})

test('network and server errors are retried and asset downloads drop the token after the redirect', async () => {
  const { calls, fetchImpl } = fakeFetch(async (url, _init, count) => {
    if (url.includes('/releases?')) {
      if (count === 1) throw new TypeError('fetch failed')
      return count === 2
        ? new Response('busy', { status: 502 })
        : new Response(JSON.stringify([{ id: 1, draft: true }]), { status: 200 })
    }
    if (url.includes('/releases/assets/5'))
      return new Response(null, { status: 302, headers: { location: 'https://objects.example/x' } })
    return new Response('bytes', { status: 200 })
  })
  const dispatcher = { dispatch() {} }
  const api = createGitHubReleases({
    token: 'secret',
    fetchImpl,
    sleepImpl: async () => {},
    dispatcher,
  })
  assert.deepEqual(await api.listReleases(), [{ id: 1, draft: true }])
  assert.equal((await api.downloadAsset(5)).toString(), 'bytes')
  assert.ok(calls.every((call) => call.dispatcher === dispatcher))
  const redirected = calls.at(-1)
  assert.equal(redirected.url, 'https://objects.example/x')
  assert.equal(redirected.headers.Authorization, undefined)
})

test('a write that never left the machine is retried, one that may have reached GitHub is not', async () => {
  const unsent = (code) => Object.assign(new TypeError('fetch failed'), { cause: { code } })
  const { calls, fetchImpl } = fakeFetch(async (_url, _init, count) => {
    if (count === 1) throw unsent('EAI_AGAIN')
    if (count === 2) return new Response(JSON.stringify({ id: 7 }), { status: 201 })
    throw unsent('ECONNRESET')
  })
  const api = createGitHubReleases({ token: 'secret', fetchImpl, sleepImpl: async () => {} })
  assert.deepEqual(await api.createDraft({ tag: 'v1.0.0', target: 'abc', title: 't', body: 'b' }), {
    id: 7,
  })
  assert.equal(calls.length, 2)
  await assert.rejects(api.publish(7, { tag: 'v1.0.0', title: 't', body: 'b' }), /fetch failed/)
  assert.equal(calls.length, 3)
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
  // 包内 publishConfig 仍要求 provenance；NPM_TOKEN 入口必须显式关闭，包括平台包和主包。
  const npmPublishCalls = [...local.matchAll(/npm\(\[\s*'publish',([\s\S]*?)\]\)/g)]
  assert.equal(npmPublishCalls.length, 2)
  for (const call of npmPublishCalls) assert.match(call[1], /'--no-provenance'/)
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
  // 手动运行，或由 self-release-* 标签流程调用（只用 build）。
  assert.deepEqual(Object.keys(workflow.on), ['workflow_dispatch', 'workflow_call'])
  assert.deepEqual(Object.keys(workflow.on.workflow_call.inputs), ['stage', 'version'])
  assert.deepEqual(workflow.permissions, { contents: 'write' })
  assert.match(workflow.concurrency.group, /windows-/)
  assert.match(workflow.concurrency.group, /inputs\.stage == 'publish'/)

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
  assert.equal(
    job.env.GIT_CONFIG_GLOBAL,
    '${{ github.workspace }}/../pisper-release-gitconfig-${{ github.run_id }}',
  )
  assert.equal(job.env.GIT_CONFIG_NOSYSTEM, '1')

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

test('self-hosted Linux releases inject signing secrets and only resume existing drafts', async () => {
  const source = await readFile('.github/workflows/release-linux-self-hosted.yml', 'utf8')
  const workflow = parseYaml(source)
  assert.deepEqual(Object.keys(workflow.on), ['workflow_dispatch', 'workflow_call'])
  assert.deepEqual(Object.keys(workflow.on.workflow_call.inputs), ['stage'])
  assert.deepEqual(workflow.permissions, { contents: 'write' })
  assert.equal(
    workflow.concurrency.group,
    "release-self-hosted-${{ github.repository }}-${{ inputs.stage == 'publish' && 'publish' || format('linux-{0}', inputs.stage) }}",
  )
  assert.equal(workflow.concurrency['cancel-in-progress'], false)

  const inputs = workflow.on.workflow_dispatch.inputs
  assert.deepEqual(inputs.stage.options, ['build', 'status', 'publish'])
  assert.equal(inputs.stage.default, 'build')
  assert.deepEqual(Object.keys(inputs), ['stage'])

  const jobs = Object.values(workflow.jobs)
  assert.equal(jobs.length, 1)
  const [job] = jobs
  // 必须同时匹配 self-hosted 和 Linux 自定义标签，不能回退到 GitHub 托管 Runner。
  assert.deepEqual(job['runs-on'], ['self-hosted', 'pisper-linux'])
  assert.equal(job.env.GITHUB_TOKEN, '${{ github.token }}')
  assert.equal(job.env.GH_TOKEN, '${{ github.token }}')
  assert.equal(
    job.env.GIT_CONFIG_GLOBAL,
    '${{ github.workspace }}/../pisper-release-gitconfig-${{ github.run_id }}',
  )
  assert.equal(job.env.GIT_CONFIG_NOSYSTEM, '1')

  const step = (name) => {
    const found = job.steps.find((entry) => entry.name === name)
    assert.ok(found, `missing workflow step: ${name}`)
    return found
  }
  assert.equal(step('Require Linux x64').shell, 'bash')
  assert.match(step('Require Linux x64').run, /RUNNER_OS.*Linux.*RUNNER_ARCH.*X64/s)

  const checkout = job.steps.find((entry) => entry.uses === 'actions/checkout@v5')
  assert.deepEqual(checkout.with, { ref: 'release', 'fetch-depth': 0 })
  assert.equal(
    job.steps.find((entry) => entry.uses === 'actions/setup-node@v5').with['node-version'],
    24,
  )
  const rust = job.steps.find((entry) => entry.uses === 'dtolnay/rust-toolchain@stable')
  assert.equal(rust.with.components, 'rustfmt, clippy')
  assert.equal(step('Install Node dependencies').run, 'npm ci')
  assert.match(
    step('Configure release commit identity').run,
    /git config user\.name[\s\S]*git config user\.email/,
  )

  const build = step('Build and upload Linux artifacts')
  assert.equal(build.if, "inputs.stage == 'build'")
  assert.equal(build.run, 'npm run release:local -- build')
  assert.equal(build.env.TAURI_SIGNING_PRIVATE_KEY, '${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}')
  assert.equal(
    build.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD,
    '${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}',
  )
  assert.deepEqual(Object.keys(build.env).sort(), [
    'TAURI_SIGNING_PRIVATE_KEY',
    'TAURI_SIGNING_PRIVATE_KEY_PASSWORD',
  ])

  const status = step('Inspect draft release assets')
  assert.equal(
    status.if,
    "inputs.stage == 'build' || inputs.stage == 'status' || inputs.stage == 'publish'",
  )
  assert.equal(status.run, 'npm run release:local -- status')
  const publish = step('Publish validated draft releases')
  assert.equal(publish.if, "inputs.stage == 'publish'")
  assert.equal(publish.run, 'npm run release:local -- publish')
  // 三个平台齐全的计划在这里发 npm：自托管 Runner 没有 npm 的可信发布，只能用令牌，且只给 publish 这一步。
  assert.deepEqual(publish.env, { NPM_TOKEN: '${{ secrets.NPM_TOKEN }}' })
  assert.equal(source.split('\n').filter((line) => /\bNPM_TOKEN\b/.test(line)).length, 1)

  // Linux 只能复用既有计划，且不得绕开依赖安装、检查或发布入口。
  assert.doesNotMatch(
    source,
    /release:local -- plan|--no-install|--allow-unsigned-mac|--skip-checks|gh workflow run|scripts\/release\.mjs/,
  )
  assert.doesNotMatch(source, /run:.*\$\{\{\s*inputs\./)
  assert.doesNotMatch(source, /npm run release(?!:local)/)
})

test('self-hosted macOS releases inject signing secrets and only resume existing drafts', async () => {
  const source = await readFile('.github/workflows/release-mac-self-hosted.yml', 'utf8')
  const workflow = parseYaml(source)
  assert.deepEqual(Object.keys(workflow.on), ['workflow_dispatch', 'workflow_call'])
  assert.deepEqual(Object.keys(workflow.on.workflow_call.inputs), ['stage'])
  assert.deepEqual(workflow.permissions, { contents: 'write' })
  assert.match(workflow.concurrency.group, /macos-/)
  assert.match(workflow.concurrency.group, /inputs\.stage == 'publish'/)

  const inputs = workflow.on.workflow_dispatch.inputs
  assert.deepEqual(inputs.stage.options, ['build', 'status', 'publish'])
  assert.equal(inputs.stage.default, 'build')
  assert.deepEqual(Object.keys(inputs), ['stage'])

  const jobs = Object.values(workflow.jobs)
  assert.equal(jobs.length, 1)
  const [job] = jobs
  assert.deepEqual(job['runs-on'], ['self-hosted', 'pisper-macos'])
  assert.equal(job.env.GITHUB_TOKEN, '${{ github.token }}')
  assert.equal(job.env.GH_TOKEN, '${{ github.token }}')
  assert.equal(
    job.env.GIT_CONFIG_GLOBAL,
    '${{ github.workspace }}/../pisper-release-gitconfig-${{ github.run_id }}',
  )
  assert.equal(job.env.GIT_CONFIG_NOSYSTEM, '1')

  const step = (name) => {
    const found = job.steps.find((entry) => entry.name === name)
    assert.ok(found, `missing workflow step: ${name}`)
    return found
  }
  assert.equal(step('Require macOS arm64').shell, 'bash')
  assert.match(step('Require macOS arm64').run, /RUNNER_OS.*macOS.*RUNNER_ARCH.*ARM64/s)
  assert.equal(
    job.steps.find((entry) => entry.uses === 'actions/setup-node@v5').with['node-version'],
    24,
  )
  assert.ok(job.steps.some((entry) => entry.uses === 'dtolnay/rust-toolchain@stable'))
  assert.equal(step('Install Node dependencies').run, 'npm ci')

  const build = step('Build and upload macOS artifacts')
  assert.equal(build.if, "inputs.stage == 'build'")
  assert.equal(build.run, 'npm run release:local -- build --allow-unsigned-mac')
  assert.equal(build.env.TAURI_SIGNING_PRIVATE_KEY, '${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}')
  assert.equal(
    build.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD,
    '${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}',
  )
  assert.deepEqual(Object.keys(build.env).sort(), [
    'TAURI_SIGNING_PRIVATE_KEY',
    'TAURI_SIGNING_PRIVATE_KEY_PASSWORD',
  ])

  assert.equal(
    step('Inspect draft release assets').if,
    "inputs.stage == 'build' || inputs.stage == 'status' || inputs.stage == 'publish'",
  )
  assert.equal(step('Publish validated draft releases').if, "inputs.stage == 'publish'")
  assert.equal(step('Publish validated draft releases').run, 'npm run release:local -- publish')

  // Mac 只能复用既有计划；沿用现有发布方式，显式构建不做 Apple 签名/公证的应用。
  assert.doesNotMatch(
    source,
    /release:local -- plan|\bAPPLE_[A-Z_]+\b|\bNPM_TOKEN\b|--skip-checks|gh workflow run|scripts\/release\.mjs/,
  )
  assert.doesNotMatch(source, /run:.*\$\{\{\s*inputs\./)
  assert.doesNotMatch(source, /npm run release(?!:local)/)
})

test('a self-release tag runs plan, three platform builds, publish and the app on self-hosted runners', async () => {
  const source = await readFile('.github/workflows/release-self-hosted-tag.yml', 'utf8')
  const workflow = parseYaml(source)
  // 与托管路径的 release-* 互不重叠：self-release-* 不会同时触发 release-tag.yml。
  assert.deepEqual(workflow.on, { push: { tags: ['self-release-*'] } })
  assert.deepEqual(workflow.permissions, { contents: 'write', actions: 'write' })
  // 和 release-tag.yml 同一分组，两条标签发布不会同时跑。
  assert.equal(workflow.concurrency.group, 'release-trigger-${{ github.repository }}')
  assert.equal(workflow.concurrency['cancel-in-progress'], false)

  const { plan, ...rest } = workflow.jobs
  // 每个 job 都只落在自托管 Runner 上：plan 自己声明，其余都调用自托管 workflow。
  assert.deepEqual(plan['runs-on'], ['self-hosted', 'pisper-linux'])
  assert.equal(plan.env.GIT_CONFIG_NOSYSTEM, '1')
  const step = (name) => {
    const found = plan.steps.find((entry) => entry.name === name)
    assert.ok(found, `missing workflow step: ${name}`)
    return found
  }
  const order = plan.steps.map((entry) => entry.name || entry.uses)
  assert.ok(
    order.indexOf('Consume trigger tag') <
      order.indexOf('Resolve release input and verify the source'),
  )
  assert.ok(
    order.indexOf('Resolve release input and verify the source') <
      order.indexOf('Plan release drafts'),
  )
  assert.match(step('Consume trigger tag').run, /git push origin ":refs\/tags\/\$TRIGGER_TAG"/)
  const resolveStep = step('Resolve release input and verify the source').run
  assert.match(resolveStep, /node scripts\/release-trigger\.mjs "\$TRIGGER_TAG" --self-hosted/)
  assert.match(resolveStep, /"\$head" != "\$TRIGGER_SHA"/)
  const planStep = step('Plan release drafts')
  assert.match(planStep.run, /npm run release:local -- plan "\$RELEASE_INPUT" --github-output/)
  // 没有 NPM_TOKEN 时这次不发 npm，而不是等到 publish 才因缺凭据失败；令牌本身不交给 plan。
  assert.equal(planStep.env.HAS_NPM_TOKEN, "${{ secrets.NPM_TOKEN != '' }}")
  assert.match(planStep.run, /--no-npm/)
  assert.doesNotMatch(source, /secrets\.NPM_TOKEN \}\}/)
  assert.doesNotMatch(source, /run:.*\$\{\{/)

  const calls = Object.fromEntries(
    Object.entries(rest).map(([name, job]) => [name, [job.uses, job.with]]),
  )
  assert.deepEqual(calls, {
    'build-windows': ['./.github/workflows/release-self-hosted.yml', { stage: 'build' }],
    'build-macos': ['./.github/workflows/release-mac-self-hosted.yml', { stage: 'build' }],
    'build-linux': ['./.github/workflows/release-linux-self-hosted.yml', { stage: 'build' }],
    publish: ['./.github/workflows/release-linux-self-hosted.yml', { stage: 'publish' }],
    app: [
      './.github/workflows/release-app.yml',
      {
        version: '${{ needs.plan.outputs.app_version }}',
        source_sha: '${{ needs.plan.outputs.source }}',
      },
    ],
  })
  for (const job of Object.values(rest)) assert.equal(job.secrets, 'inherit')
  assert.deepEqual(rest.publish.needs, ['plan', 'build-windows', 'build-macos', 'build-linux'])
  // App 在组件发布成功之后（或只有 App 要发时）才发，两边不会同时往 release 推版本提交。
  assert.deepEqual(rest.app.needs, ['plan', 'publish'])
  assert.match(
    rest.app.if,
    /needs\.plan\.outputs\.components == '' \|\| needs\.publish\.result == 'success'/,
  )

  const app = parseYaml(await readFile('.github/workflows/release-app.yml', 'utf8'))
  assert.deepEqual(Object.keys(app.on), ['workflow_dispatch', 'workflow_call'])
  assert.deepEqual(Object.keys(app.on.workflow_call.inputs), ['version', 'source_sha'])
})

test('plan can skip npm and hand its result to the tag workflow', async () => {
  const local = await readFile('scripts/release-local.mjs', 'utf8')
  assert.match(local, /flags\.has\('--no-npm'\)[\s\S]*computed\.npm = null/)
  // 输出只在建完草稿后写（只有 App 要发时不建草稿也写），下一步按它决定构建什么。
  assert.ok(
    local.indexOf('if (githubOutput) await writeGithubOutput') > local.indexOf('api.createDraft('),
  )
  assert.match(local, /`source=\$\{source\}`/)
  assert.match(local, /`app_version=\$\{app\?\.version \|\| ''\}`/)
})
