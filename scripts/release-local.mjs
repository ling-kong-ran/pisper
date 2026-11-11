// 本机发布：GitHub Actions 额度用完时，在自己的 Windows、macOS 和 Linux 电脑上发布 Desktop、TUI、Runtime 与 npm。
// 和 CI 同一套脚本（release.mjs 算计划，stage-*/archive-*/validate-* 构建与校验），只是把三个平台的构建
// 分到三台机器上，用 GitHub 草稿 Release 汇合：
//
//   plan [patch|minor|major|X.Y.Z] [--platforms=windows,darwin] [--no-npm] [--github-output]
//                                   任一台：检测通道、算版本、跑发布前检查，为每个版本建草稿 Release；
//                                   --platforms 可以跳过某个平台（跳过时这次不发 npm）；--no-npm 这次不发 npm
//                                   （自托管流程里没有 NPM_TOKEN 时）；--github-output 把源提交和 App 版本
//                                   写进 $GITHUB_OUTPUT，供 release-self-hosted-tag.yml 接着构建、发布与发 App
//   build                           每台各跑一次：按草稿里的计划构建本平台产物、签名并上传到草稿
//   status                          看各草稿还缺哪些平台的产物
//   publish                         三个平台都传完后任一台：校验资产、提交版本号并打标签、推送、
//                                   发布草稿，最后发布 npm
//
// 从 plan 到 publish 期间 release 分支冻结（不推别的提交），verify-release-head 只放行版本提交。
// 移动 App 仍走 CI（release-app.yml），计划里有 app 时这里跳过并提示；标签触发的自托管流程在 publish 后接着发 App。
//
// 凭据放在不入库的文件里：默认 ~/.config/pisper-release/env（或用 PISPER_RELEASE_ENV 指定），
// 每行 KEY=VALUE：GITHUB_TOKEN、TAURI_SIGNING_PRIVATE_KEY、TAURI_SIGNING_PRIVATE_KEY_PASSWORD、
// NPM_TOKEN（publish 发 npm 时），macOS 打桌面包时还有 APPLE_CERTIFICATE 等签名与公证变量。
import { execFileSync } from 'node:child_process'
import { existsSync, statSync } from 'node:fs'
import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createGitHubReleases } from './github-release-api.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const PLAN_ASSET = 'pisper-release-plan.json'
const RELEASE_BRANCH = 'release'
const COMPONENT_ORDER = ['tui', 'runtime', 'desktop']
const VERSION_FILES = {
  desktop: ['src-tauri/desktop-package.json'],
  tui: ['src-tui/Cargo.toml', 'src-tui/Cargo.lock'],
  runtime: ['package.json', 'package-lock.json'],
  npm: ['packages/pisper/package.json'],
}
// 发布过程中生成、不入库的文件：构建结束后清掉，免得留在使用者的工作区里。
const GENERATED_FILES = ['public/release-notes.json', 'release-body.md']
const PLATFORM =
  process.platform === 'win32' ? 'windows' : process.platform === 'darwin' ? 'darwin' : 'linux'
const ARCH = process.arch === 'x64' ? 'x86_64' : process.arch === 'arm64' ? 'aarch64' : process.arch
const TARGET = `${PLATFORM}_${ARCH}`
const SUPPORTED_TARGETS = ['windows_x86_64', 'darwin_aarch64', 'linux_x86_64']

// 这次发布哪些平台：plan --platforms=windows,darwin 可以跳过某个平台（例如 Linux 构建机暂时不可用）。
// 跳过的平台在组件更新里停在上一版（更新器只认带本平台产物的 Release），npm 这次不发。
function parsePlatforms(value) {
  if (!value) return SUPPORTED_TARGETS
  const targets = value
    .split(',')
    .map((name) =>
      SUPPORTED_TARGETS.find(
        (target) => target.startsWith(`${name.trim()}_`) || target === name.trim(),
      ),
    )
  if (targets.some((target) => !target) || !targets.length)
    throw new Error(`--platforms 只能是 windows、darwin、linux 的组合：${value}`)
  return [...new Set(targets)]
}

function planPlatforms(current) {
  return Array.isArray(current.platforms) && current.platforms.length
    ? current.platforms
    : SUPPORTED_TARGETS
}

const PLATFORM_NAMES = { windows: 'Windows', darwin: 'macOS', linux: 'Linux' }
function platformNames(targets) {
  return targets.map((target) => PLATFORM_NAMES[target.split('_')[0]] || target).join('、')
}

const [command = 'help', ...rest] = process.argv.slice(2)
const flags = new Set(rest.filter((value) => value.startsWith('--')))
const positional = rest.filter((value) => !value.startsWith('--'))

function run(file, args, { capture = false, env } = {}) {
  const result = execFileSync(file, args, {
    cwd: root,
    encoding: 'utf8',
    stdio: capture ? 'pipe' : 'inherit',
    env: env ? { ...process.env, ...env } : process.env,
  })
  return typeof result === 'string' ? result.trim() : ''
}
const git = (args) => run('git', args, { capture: true })
const node = (args, options) => run(process.execPath, args, options)
// 经 npm run 启动时 npm_execpath 指向 npm 的 JS 入口：用 node 直接跑，Windows 上不必经 shell
// 启动 npm.cmd（路径里有空格时会被拆开）。
const npmCli = String(process.env.npm_execpath || '').trim()
const npm = (args, options) =>
  npmCli
    ? node([npmCli, ...args], options)
    : run(process.platform === 'win32' ? 'npm.cmd' : 'npm', args, options)
const tauriCli = join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js')

// 凭据文件：KEY=VALUE，# 开头是注释；已在环境变量里的不覆盖。不打印任何值。
async function loadSecrets() {
  const path = process.env.PISPER_RELEASE_ENV || join(homedir(), '.config', 'pisper-release', 'env')
  if (!existsSync(path)) {
    console.warn(`未找到凭据文件 ${path}，只使用当前环境变量。`)
    return
  }
  if (process.platform !== 'win32' && (statSync(path).mode & 0o077) !== 0)
    console.warn(`凭据文件 ${path} 对其他用户可读，建议 chmod 600。`)
  for (const line of (await readFile(path, 'utf8')).split(/\r?\n/)) {
    const match = line.match(/^\s*([A-Z0-9_]+)\s*=\s*(.*?)\s*$/)
    if (!match || line.trim().startsWith('#')) continue
    const value = match[2].replace(/^(['"])(.*)\1$/, '$2')
    if (!process.env[match[1]]) process.env[match[1]] = value
  }
}

function requireEnv(...names) {
  const missing = names.filter((name) => !String(process.env[name] || '').trim())
  if (missing.length) throw new Error(`缺少凭据：${missing.join('、')}（写进凭据文件或环境变量）。`)
}

function github() {
  requireEnv('GITHUB_TOKEN')
  return createGitHubReleases({ token: process.env.GITHUB_TOKEN })
}

function assertCleanTree() {
  if (git(['status', '--porcelain', '--untracked-files=no']))
    throw new Error('工作区有未提交的修改，请先提交或处理。')
}

function releaseTitleFor(component, version, tag) {
  return node(['scripts/validate-release-notes.mjs', 'release-body.md', component, version, tag], {
    capture: true,
  })
}

async function restoreWorkingTree(components) {
  const paths = components.flatMap((component) => VERSION_FILES[component] || [])
  if (paths.length) git(['checkout', '--', ...paths])
  await Promise.all(GENERATED_FILES.map((file) => rm(join(root, file), { force: true })))
}

// 草稿里的计划：每个草稿都带一份 pisper-release-plan.json；HEAD 等于计划的源提交才算本次发布。
// publish 中途失败时部分 Release 可能已经发布：resume 时也从已发布的 Release 里找计划，并标出哪些已发布。
async function findPlan(api, { source, resume = false } = {}) {
  const releases = await api.listReleases()
  const drafts = releases.filter((release) => release.draft)
  const plans = []
  for (const release of resume ? releases : drafts) {
    const asset = release.assets?.find((item) => item.name === PLAN_ASSET)
    if (!asset) continue
    const plan = JSON.parse((await api.downloadAsset(asset.id)).toString('utf8'))
    if (!plans.some((item) => item.source === plan.source)) plans.push(plan)
  }
  const matching = source ? plans.filter((plan) => plan.source === source) : plans
  if (matching.length === 0)
    throw new Error(
      plans.length
        ? `草稿里的发布计划来自 ${plans.map((plan) => plan.source.slice(0, 10)).join('、')}，与当前提交不一致；先 git fetch 并切到那个提交。`
        : '没有找到待构建的发布计划，请先在任一台机器上运行 plan。',
    )
  if (matching.length > 1) throw new Error('找到多份发布计划，请先删除过期的草稿 Release。')
  const plan = matching[0]
  for (const item of plan.components) {
    const release = (resume ? releases : drafts).find((entry) => entry.id === item.releaseId)
    if (!release || (!release.draft && release.tag_name !== item.tag))
      throw new Error(`计划里的草稿 ${item.tag} 不见了，请重新运行 plan。`)
    item.draft = release
    item.published = !release.draft
  }
  return plan
}

// 自托管标签流程的下一步要用的计划结果：源提交、本机发布的组件、要另外发的 App 版本。
async function writeGithubOutput({ source, components, app }) {
  const path = process.env.GITHUB_OUTPUT
  if (!path) throw new Error('--github-output 只能在 GitHub Actions 里使用（缺少 GITHUB_OUTPUT）。')
  const lines = [
    `source=${source}`,
    `components=${components.map((item) => item.component).join(',')}`,
    `app_version=${app?.version || ''}`,
  ]
  await writeFile(path, `${lines.join('\n')}\n`, { flag: 'a' })
}

async function plan() {
  await loadSecrets()
  const dryRun = flags.has('--dry-run')
  // 试算不碰 GitHub：不需要令牌，也不建草稿。
  const api = dryRun ? null : github()
  const input = positional[0] || 'patch'
  assertCleanTree()
  if (
    api &&
    (await api.listReleases()).some((release) =>
      release.assets?.some((asset) => asset.name === PLAN_ASSET),
    )
  )
    throw new Error(
      '已有一份进行中的发布计划（草稿 Release，或 publish 中途失败留下的计划）。先 publish，或删除那些草稿再重新 plan。',
    )

  const planPath = join(tmpdir(), `pisper-release-plan-${process.pid}.json`)
  // release.mjs 负责：分支与远端同步检查、Pi 依赖更新、通道检测、版本计算与发布前检查。
  node([
    'scripts/release.mjs',
    input,
    `--plan-json=${planPath}`,
    ...(flags.has('--skip-checks') ? ['--skip-checks'] : []),
    ...(dryRun ? ['--skip-dependency-update'] : []),
  ])
  const computed = JSON.parse(await readFile(planPath, 'utf8'))
  await rm(planPath, { force: true })
  const skipped = computed.components.filter(
    ({ component }) => !COMPONENT_ORDER.includes(component),
  )
  for (const item of skipped)
    console.warn(
      `${item.component} ${item.version} 不在本机发布范围内（移动 App 仍走 CI），这次跳过。`,
    )
  const components = computed.components
    .filter(({ component }) => COMPONENT_ORDER.includes(component))
    .sort(
      (left, right) =>
        COMPONENT_ORDER.indexOf(left.component) - COMPONENT_ORDER.indexOf(right.component),
    )
  const app = skipped.find((item) => item.component === 'app')
  const githubOutput = flags.has('--github-output') && !dryRun
  if (!components.length) {
    // 只有 App 要发：不建草稿，交给标签流程直接发 App。
    if (githubOutput && app) {
      await writeGithubOutput({ source: computed.source, components, app })
      console.log(`这次只有 App ${app.version} 要发布，不建草稿。`)
      return
    }
    throw new Error('计划里没有 Desktop、TUI 或 Runtime。')
  }

  // 更新日志在这里生成一次写进计划：各台机器构建时用同一份，不因日期或时区不同而不一致。
  for (const item of components) {
    try {
      node(['scripts/stage-release-version.mjs', item.component, item.version, computed.source], {
        env: {
          RELEASE_TUI_VERSION: computed.bundled.tui,
          RELEASE_RUNTIME_VERSION: computed.bundled.runtime,
        },
      })
      node(['scripts/prepare-release.mjs', item.tag, computed.source, item.component])
      item.body = await readFile(join(root, 'release-body.md'), 'utf8')
      item.releaseNotes = existsSync(join(root, 'public', 'release-notes.json'))
        ? JSON.parse(await readFile(join(root, 'public', 'release-notes.json'), 'utf8'))
        : null
      item.title = releaseTitleFor(item.component, item.version, item.tag)
    } finally {
      await restoreWorkingTree([item.component])
    }
  }

  const platforms = parsePlatforms(
    rest.find((value) => value.startsWith('--platforms='))?.slice('--platforms='.length),
  )
  // npm 的三个平台包缺一不可：跳过平台时这次不发 npm，下次三个平台齐了再一起发。
  if (computed.npm && platforms.length < SUPPORTED_TARGETS.length) {
    console.warn(`跳过了部分平台，这次不发布 npm（pisper@${computed.npm.version}）。`)
    computed.npm = null
  }
  if (computed.npm && flags.has('--no-npm')) {
    console.warn(`指定了 --no-npm，这次不发布 npm（pisper@${computed.npm.version}）。`)
    computed.npm = null
  }
  const result = {
    schema: 1,
    createdAt: new Date().toISOString(),
    ...computed,
    platforms,
    components,
  }
  if (dryRun) {
    console.log(
      JSON.stringify(
        {
          ...result,
          components: components.map(({ component, version, tag, title }) => ({
            component,
            version,
            tag,
            title,
          })),
        },
        null,
        2,
      ),
    )
    return
  }
  const file = join(tmpdir(), PLAN_ASSET)
  try {
    for (const item of components) {
      const draft = await api.createDraft({
        tag: item.tag,
        target: computed.source,
        title: item.title,
        body: item.body,
      })
      item.releaseId = draft.id
    }
    await writeFile(file, `${JSON.stringify(result, null, 2)}\n`)
    for (const item of components) await api.uploadAsset(item.releaseId, file)
  } catch (error) {
    // 建到一半的草稿留着会让下一次 plan 以为已有计划（或留下没有计划的空草稿），先删掉再报错。
    for (const item of components.filter((entry) => entry.releaseId)) {
      try {
        await api.deleteRelease(item.releaseId)
      } catch (cleanupError) {
        console.warn(
          `删除未完成的草稿 ${item.tag} 失败，请到 GitHub 手动删除：${cleanupError.message}`,
        )
      }
    }
    throw error
  } finally {
    await rm(file, { force: true })
  }
  console.log(`已建草稿：${components.map((item) => item.tag).join('、')}`)
  if (githubOutput) await writeGithubOutput({ source: computed.source, components, app })
  console.log(
    `接下来在 ${platformNames(platforms)} 上各切到 ${computed.source} 运行：npm run release:local -- build`,
  )
}

function platformTests(hasTui, hasDesktop, hasRuntime) {
  if (hasTui || hasDesktop)
    run('cargo', [
      'test',
      '--manifest-path',
      'src-tui/Cargo.toml',
      '--locked',
      '-p',
      'pisper-component-updater',
    ])
  if (process.platform === 'win32' && (hasRuntime || hasDesktop))
    node(['--import', 'tsx', '--test', 'runtime/tests/host-bash.test.mjs'])
}

async function filesUnder(directory) {
  if (!existsSync(directory)) return []
  const files = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) files.push(...(await filesUnder(path)))
    else files.push(path)
  }
  return files
}

// 产物归哪个草稿：组件归档按名字前缀，Tauri 安装包与更新包都归 Desktop。
function componentOfArtifact(path) {
  const name = basename(path)
  if (name.startsWith('Pisper_TUI_')) return 'tui'
  if (name.startsWith('Pisper_Runtime_')) return 'runtime'
  if (name.startsWith('Pisper_Desktop_') || path.includes(`${join('release', 'tauri-artifacts')}`))
    return 'desktop'
  return ''
}

async function build() {
  await loadSecrets()
  if (!SUPPORTED_TARGETS.includes(TARGET)) throw new Error(`不支持在 ${TARGET} 上构建发布产物。`)
  requireEnv('TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD')
  const api = github()
  assertCleanTree()
  const source = git(['rev-parse', 'HEAD'])
  const current = await findPlan(api, { source })
  if (!planPlatforms(current).includes(TARGET))
    throw new Error(`这次发布不包含 ${TARGET}（计划平台：${planPlatforms(current).join('、')}）。`)
  const has = (component) => current.components.some((item) => item.component === component)
  const item = (component) => current.components.find((entry) => entry.component === component)
  if (has('desktop') && PLATFORM === 'darwin' && !flags.has('--allow-unsigned-mac'))
    requireEnv(
      'APPLE_CERTIFICATE',
      'APPLE_CERTIFICATE_PASSWORD',
      'APPLE_SIGNING_IDENTITY',
      'APPLE_ID',
      'APPLE_PASSWORD',
      'APPLE_TEAM_ID',
    )
  console.log(
    `在 ${TARGET} 上构建 ${current.components.map((entry) => entry.tag).join('、')}（源 ${source}）`,
  )

  if (!flags.has('--no-install')) npm(['ci'])
  platformTests(has('tui'), has('desktop'), has('runtime'))
  await Promise.all(
    ['release/component-artifacts', 'release/tauri-artifacts'].map((dir) =>
      rm(join(root, dir), { recursive: true, force: true }),
    ),
  )
  const staged = []
  const bundledEnv = {
    RELEASE_TUI_VERSION: current.bundled.tui || '',
    RELEASE_RUNTIME_VERSION: current.bundled.runtime || '',
  }
  const writeNotes = async (component) => {
    const notes = item(component)?.releaseNotes
    if (!notes) return
    await mkdir(join(root, 'public'), { recursive: true })
    await writeFile(
      join(root, 'public', 'release-notes.json'),
      `${JSON.stringify(notes, null, 2)}\n`,
    )
  }
  try {
    for (const component of COMPONENT_ORDER.filter(has)) {
      node(['scripts/stage-release-version.mjs', component, item(component).version, source], {
        env: bundledEnv,
      })
      staged.push(component)
    }
    if (has('tui') || has('runtime')) {
      await writeNotes('runtime')
      npm(['run', 'sidecar:sea'])
      npm(['run', 'sidecar:sea:smoke'])
      if (has('tui')) {
        npm(['run', 'tui:package'])
        node(['scripts/archive-component-release.mjs', 'tui'])
      }
      if (has('runtime')) node(['scripts/archive-component-release.mjs', 'runtime'])
    }
    if (has('desktop')) {
      await writeNotes('desktop')
      npm(['run', 'build'])
      // 刚在本机构建并签名的 TUI/Runtime 还没发布：从本地归档取，没在计划里的才去下载已发布的版本。
      await signComponentArchives()
      node([
        'scripts/fetch-bundled-components.mjs',
        `--tui=${current.bundled.tui}`,
        `--runtime=${current.bundled.runtime}`,
        '--local=release/component-artifacts',
      ])
      npm(['run', 'desktop:webview:package'])
      node(['scripts/archive-component-release.mjs', 'desktop'])
      run('cargo', ['test', '--manifest-path', 'src-tauri/Cargo.toml', '--locked'])
    }
    await signComponentArchives()
  } finally {
    await restoreWorkingTree(staged)
  }

  const artifacts = [
    ...(await filesUnder(join(root, 'release', 'component-artifacts'))),
    ...(await filesUnder(join(root, 'release', 'tauri-artifacts'))),
  ].filter((path) => basename(path).includes(TARGET) && componentOfArtifact(path))
  if (!artifacts.length) throw new Error('没有找到要上传的产物。')
  for (const entry of current.components) {
    const files = artifacts.filter((path) => componentOfArtifact(path) === entry.component)
    const existing = await api.listAssets(entry.releaseId)
    for (const file of files) {
      console.log(`上传 ${basename(file)} → ${entry.tag}`)
      await api.uploadAsset(entry.releaseId, file, existing)
    }
  }
  console.log(
    `${TARGET} 的产物已上传。${platformNames(planPlatforms(current))} 都传完后运行：npm run release:local -- status / publish`,
  )
}

// 组件归档用 Tauri 的更新签名（与 CI 的 Sign independently updateable component archives 一致）；已签过的跳过。
async function signComponentArchives() {
  for (const archive of await filesUnder(join(root, 'release', 'component-artifacts'))) {
    if (!archive.endsWith('.tar.gz') || existsSync(`${archive}.sig`)) continue
    node([tauriCli, 'signer', 'sign', archive])
  }
}

function expectedAssetsReport(entry, names, platforms) {
  const missing = platforms.filter((target) => !names.some((name) => name.includes(target)))
  return `${entry.tag}: ${names.length - 1} 个产物${missing.length ? `，缺 ${missing.join('、')}` : '，计划的平台都齐了'}`
}

async function status() {
  await loadSecrets()
  const api = github()
  const current = await findPlan(api, { resume: true })
  console.log(
    `发布计划：源 ${current.source}，${current.createdAt}，平台 ${planPlatforms(current).join('、')}`,
  )
  for (const entry of current.components) {
    const names = (await api.listAssets(entry.releaseId)).map((asset) => asset.name)
    console.log(
      `${expectedAssetsReport(entry, names, planPlatforms(current))}${entry.published ? '（已发布）' : ''}`,
    )
  }
  if (current.npm) console.log(`npm：pisper@${current.npm.version}（publish 时发布）`)
}

async function publish() {
  await loadSecrets()
  const api = github()
  if (git(['branch', '--show-current']) !== RELEASE_BRANCH)
    throw new Error(`请在 ${RELEASE_BRANCH} 分支上发布。`)
  assertCleanTree()
  git(['fetch', '--tags', 'origin', RELEASE_BRANCH])
  const head = git(['rev-parse', 'HEAD'])
  if (head !== git(['rev-parse', `origin/${RELEASE_BRANCH}`]))
    throw new Error(
      `本地 ${RELEASE_BRANCH} 必须与 origin/${RELEASE_BRANCH} 一致（先 git pull --rebase）。`,
    )
  const current = await findPlan(api, { resume: true })
  if (current.npm) requireEnv('NPM_TOKEN')
  // 计划之后 release 上只能有版本提交，和 CI 的规则一样。
  node(['scripts/verify-release-head.mjs', current.source, head])

  const platforms = planPlatforms(current)
  const platformsArg = `--platforms=${platforms.join(',')}`
  const npmTag = current.npm ? `npm-v${current.npm.version}` : null

  // 上次 publish 已推送（标签都在远端、指向 release 上的版本提交）就跳过提交和推送，只补发布草稿和 npm。
  // 只有一部分标签在远端说明不是这份计划推的，停下来人工处理。
  const plannedTags = [...current.components.map((entry) => entry.tag), ...(npmTag ? [npmTag] : [])]
  const remoteTags = plannedTags.filter((tag) => git(['ls-remote', 'origin', `refs/tags/${tag}`]))
  const pushed = remoteTags.length === plannedTags.length
  if (remoteTags.length && !pushed) throw new Error(`远端已有标签 ${remoteTags.join('、')}。`)
  if (pushed) {
    for (const tag of plannedTags) {
      const component =
        tag === npmTag ? 'npm' : current.components.find((e) => e.tag === tag).component
      const subject = git(['log', '-1', '--format=%s', `refs/tags/${tag}^{commit}`])
      let reachable = true
      try {
        git(['merge-base', '--is-ancestor', `refs/tags/${tag}`, head])
      } catch {
        reachable = false
      }
      if (!reachable || subject !== `chore(release-${component}): ${tag}`)
        throw new Error(`远端标签 ${tag} 不是这份计划的版本提交。`)
    }
    console.log(`版本提交和标签上次已推送，继续发布草稿：${plannedTags.join('、')}`)
  }

  // 1. 下载每个草稿的产物（计划文件与旧的 latest.json 除外）。
  const publishRoot = join(root, 'release', 'publish')
  await rm(publishRoot, { recursive: true, force: true })
  for (const entry of current.components) {
    const directory = join(publishRoot, entry.tag)
    await mkdir(directory, { recursive: true })
    const assets = (await api.listAssets(entry.releaseId)).filter(
      (asset) => asset.name !== PLAN_ASSET && asset.name !== 'latest.json',
    )
    for (const asset of assets)
      await writeFile(join(directory, asset.name), await api.downloadAsset(asset.id))
  }

  // 2. 本地版本提交与标签：每个通道一个提交（与 CI 的提交信息、文件范围相同）。
  //    latest.json 要读暂存后的 Desktop 版本，所以先提交、再生成清单和校验，全部通过才推送。
  const tags = []
  const bundledEnv = {
    RELEASE_TUI_VERSION: current.bundled.tui || '',
    RELEASE_RUNTIME_VERSION: current.bundled.runtime || '',
  }
  const undo = () => {
    for (const tag of tags) git(['tag', '-d', tag])
    git(['reset', '--keep', head])
  }
  try {
    // 已推送时 HEAD 上的版本文件已经是这次的版本，直接生成清单和校验。
    for (const entry of pushed ? [] : current.components) {
      node(['scripts/stage-release-version.mjs', entry.component, entry.version], {
        env: bundledEnv,
      })
      git(['add', ...VERSION_FILES[entry.component]])
      git(['commit', '-m', `chore(release-${entry.component}): ${entry.tag}`])
      git(['tag', entry.tag])
      tags.push(entry.tag)
    }
    if (npmTag && !pushed) {
      node([
        'scripts/stage-npm-release-version.mjs',
        current.npm.version,
        current.npm.tui,
        current.npm.runtime,
      ])
      git(['add', ...VERSION_FILES.npm])
      git(['commit', '-m', `chore(release-npm): ${npmTag}`])
      git(['tag', npmTag])
      tags.push(npmTag)
    }

    // 3. 校验完整资产集合；Desktop 用它的更新日志生成 latest.json。
    for (const entry of current.components) {
      const directory = join(publishRoot, entry.tag)
      if (entry.component === 'desktop') {
        await writeFile(join(root, 'release-body.md'), entry.body)
        try {
          node([
            'scripts/create-tauri-update-manifest.mjs',
            entry.tag,
            directory,
            '--require-all',
            platformsArg,
            '--output',
            join(directory, 'latest.json'),
          ])
        } finally {
          await rm(join(root, 'release-body.md'), { force: true })
        }
        node(['scripts/validate-tauri-release-assets.mjs', entry.tag, directory, platformsArg])
      } else {
        node([
          'scripts/validate-component-release-assets.mjs',
          entry.component,
          entry.tag,
          directory,
          platformsArg,
        ])
      }
    }

    if (!pushed)
      git([
        'push',
        '--atomic',
        'origin',
        `HEAD:refs/heads/${RELEASE_BRANCH}`,
        ...tags.map((tag) => `refs/tags/${tag}`),
      ])
  } catch (error) {
    // 推送之前任何一步失败，远端都没有变化：撤掉本地的版本提交和标签，修好后直接重跑 publish。
    undo()
    throw error
  }

  // 4. 发布草稿：先补传 latest.json；TUI、Runtime 先发，Desktop 最后并设为 Latest。
  //    计划文件等 npm 也发完才删：中途失败时重跑 publish 还能找到这份计划，从断点继续。
  for (const entry of current.components) {
    if (entry.published) {
      console.log(`${entry.tag} 已发布，跳过`)
      continue
    }
    const existing = await api.listAssets(entry.releaseId)
    if (entry.component === 'desktop')
      await api.uploadAsset(entry.releaseId, join(publishRoot, entry.tag, 'latest.json'), existing)
    await api.publish(entry.releaseId, {
      tag: entry.tag,
      title: entry.title,
      body: entry.body,
      latest: entry.component === 'desktop',
    })
    console.log(`已发布 ${entry.tag}`)
  }

  if (current.npm) await publishNpm(current, publishRoot)
  for (const entry of current.components) {
    const planAsset = (await api.listAssets(entry.releaseId)).find(
      (asset) => asset.name === PLAN_ASSET,
    )
    if (planAsset) await api.deleteAsset(planAsset.id)
  }
  console.log('本机发布完成。')
}

// npm：与 publish-npm.yml 相同的打包与校验，只是在本机用 NPM_TOKEN 发布（本机没有 provenance）。
async function publishNpm(current, publishRoot) {
  const { version, tui, runtime } = current.npm
  npm(['run', 'build'])
  node(['scripts/validate-npm-targets.mjs'])
  npm(['run', 'npm:pack:check'])
  const componentDir = join(root, 'release', 'npm', 'component-artifacts')
  await rm(componentDir, { recursive: true, force: true })
  await mkdir(componentDir, { recursive: true })
  const wanted = [
    [`tui-v${tui}`, `Pisper_TUI_Component_${tui}_`],
    [`runtime-v${runtime}`, `Pisper_Runtime_Node_${runtime}_`],
  ]
  for (const [tag, prefix] of wanted) {
    const local = join(publishRoot, tag)
    if (existsSync(local)) {
      for (const file of await filesUnder(local))
        if (basename(file).startsWith(prefix))
          await writeFile(join(componentDir, basename(file)), await readFile(file))
      continue
    }
    // 这次没发布的组件：从已发布的 Release 下载。
    const release = (await github().listReleases()).find(
      (item) => item.tag_name === tag && !item.draft,
    )
    if (!release) throw new Error(`找不到已发布的 ${tag}。`)
    for (const asset of release.assets.filter((item) => item.name.startsWith(prefix)))
      await writeFile(join(componentDir, asset.name), await github().downloadAsset(asset.id))
  }
  node(['scripts/package-npm-platforms.mjs'])

  const userconfig = join(tmpdir(), `pisper-npmrc-${process.pid}`)
  await writeFile(userconfig, `//registry.npmjs.org/:_authToken=${process.env.NPM_TOKEN}\n`, {
    mode: 0o600,
  })
  try {
    const published = (name) => {
      try {
        npm(['view', name, 'version', '--json'], { capture: true })
        return true
      } catch {
        return false
      }
    }
    for (const slug of ['win32-x64', 'darwin-arm64', 'linux-x64']) {
      const platformVersion = `${version}-${slug}.0`
      if (published(`pisper@${platformVersion}`)) continue
      npm([
        'publish',
        `release/npm/tarballs/pisper-${platformVersion}.tgz`,
        '--access',
        'public',
        '--no-provenance',
        '--tag',
        `platform-${slug}`,
        '--userconfig',
        userconfig,
      ])
    }
    if (!published(`pisper@${version}`))
      npm([
        'publish',
        `release/npm/tarballs/pisper-${version}.tgz`,
        '--access',
        'public',
        '--no-provenance',
        '--userconfig',
        userconfig,
      ])
  } finally {
    await rm(userconfig, { force: true })
  }
  console.log(`已发布 pisper@${version}`)
}

const commands = { plan, build, status, publish }
if (!commands[command]) {
  console.log(
    '用法：npm run release:local -- <plan [patch|minor|major|X.Y.Z] [--platforms=windows,darwin,linux] [--no-npm] [--github-output] [--skip-checks] [--dry-run] | build [--no-install] [--allow-unsigned-mac] | status | publish>',
  )
  process.exit(command === 'help' ? 0 : 1)
}
await commands[command]()
