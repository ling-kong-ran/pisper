import { execFileSync } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  appReleasePaths,
  appReleaseSubjects,
  componentReleasePaths,
  componentReleaseSubjects,
} from './release-changes.mjs'
import { APP_VERSION_FILE, appReleaseTag, appTagPattern, appVersionFromTag } from './app-paths.mjs'
import { isSubstantiveReleaseCommit } from './release-policy.mjs'
import {
  RELEASE_COMPONENTS,
  fallbackReleaseTag,
  readComponentVersion,
  releaseTag,
  releaseVersionFromTag,
} from './release-components.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const npmCli = String(process.env.npm_execpath || '').trim()
const rawArgs = process.argv.slice(2)
const args = rawArgs.filter((value) => !value.startsWith('--'))
if (RELEASE_COMPONENTS[args[0]] || args[0] === 'app') {
  throw new Error('发布通道由变更路径自动检测，无需指定 desktop、tui、runtime 或 app。')
}
if (args.length > 1) {
  throw new Error('用法：npm run release -- <major|minor|patch|X.Y.Z>')
}
const input = args[0] || 'patch'

function run(command, commandArgs, { capture = false } = {}) {
  const result = execFileSync(command, commandArgs, {
    cwd: root,
    encoding: 'utf8',
    stdio: capture ? 'pipe' : 'inherit',
  })
  return typeof result === 'string' ? result.trim() : ''
}

function runNpm(commandArgs, options) {
  if (npmCli) return run(process.execPath, [npmCli, ...commandArgs], options)
  return run(process.platform === 'win32' ? 'npm.cmd' : 'npm', commandArgs, options)
}

const PI_CODING_AGENT_PACKAGE = '@earendil-works/pi-coding-agent'

function updateUpstreamPiDependency() {
  console.log(`正在检查 ${PI_CODING_AGENT_PACKAGE} 的最新版本…`)
  // 只更新清单和锁文件，避免发布前安装依赖改变 node_modules 或引入未跟踪文件。
  runNpm(['install', `${PI_CODING_AGENT_PACKAGE}@latest`, '--save', '--package-lock-only'])
  const changed = run('git', ['diff', '--name-only', '--', 'package.json', 'package-lock.json'], {
    capture: true,
  })
  if (!changed) {
    console.log(`${PI_CODING_AGENT_PACKAGE} 已是最新版本。`)
    return false
  }

  const unexpected = run('git', ['status', '--porcelain', '--untracked-files=no'], {
    capture: true,
  })
    .split(/\r?\n/)
    .filter(Boolean)
    .filter((line) => !/^(?: M|M |MM) (?:package\.json|package-lock\.json)$/.test(line))
  if (unexpected.length > 0) {
    throw new Error(`自动更新 ${PI_CODING_AGENT_PACKAGE} 产生了非预期修改：\n${unexpected.join('\n')}`)
  }

  run('git', ['add', 'package.json', 'package-lock.json'])
  run('git', ['commit', '-m', 'chore(deps): update pi coding agent'])
  run('git', ['push', 'origin', releaseBranch])
  console.log(`已提交并推送 ${PI_CODING_AGENT_PACKAGE} 更新。`)
  return true
}

function assertVersionInput(value) {
  if (!['major', 'minor', 'patch'].includes(value) && !/^\d+\.\d+\.\d+$/.test(value)) {
    throw new Error(`版本参数无效：${value}。请使用 major、minor、patch 或 x.y.z。`)
  }
}

function parseVersion(value) {
  return String(value).split('.').map(Number)
}

function compareVersions(left, right) {
  const a = parseVersion(left)
  const b = parseVersion(right)
  for (let index = 0; index < 3; index += 1) {
    if (a[index] !== b[index]) return a[index] - b[index]
  }
  return 0
}

function resolveVersion(current, target) {
  if (/^\d+\.\d+\.\d+$/.test(target)) return target
  const [major, minor, patch] = parseVersion(current)
  if (target === 'major') return `${major + 1}.0.0`
  if (target === 'minor') return `${major}.${minor + 1}.0`
  return `${major}.${minor}.${patch + 1}`
}

function runComponentChecks(selected) {
  console.log(`正在执行 ${selected.join('、')} 发布前检查…`)
  // lockfile 损坏只会在冷 `npm ci` 时爆发，这里提前对照注册表校验变更条目。
  run(process.execPath, ['scripts/verify-lockfile-integrity.mjs'])
  runNpm(['run', 'postinstall'])
  if (selected.includes('desktop') || selected.includes('runtime') || selected.includes('app')) {
    runNpm(['test'])
    runNpm(['run', 'check'])
  }
  if (selected.includes('desktop') || selected.includes('runtime')) {
    runNpm(['run', 'build'])
  }
  if (selected.includes('desktop') || selected.includes('tui')) {
    run('cargo', ['fmt', '--manifest-path', 'src-tui/Cargo.toml', '--', '--check'])
    runNpm(['run', 'tui:test'])
    runNpm(['run', 'tui:check'])
  }
  if (selected.includes('desktop') || selected.includes('app')) {
    run('cargo', ['fmt', '--manifest-path', 'src-tauri/Cargo.toml', '--', '--check'])
    run('cargo', ['test', '--manifest-path', 'src-tauri/Cargo.toml', '--locked'])
    run('cargo', [
      'clippy',
      '--manifest-path',
      'src-tauri/Cargo.toml',
      '--all-targets',
      '--locked',
      '--',
      '-D',
      'warnings',
    ])
  }
}

assertVersionInput(input)

const dirty = run('git', ['status', '--porcelain', '--untracked-files=no'], { capture: true })
if (dirty) throw new Error('发布前 tracked 工作区必须保持干净，请先提交或处理现有修改。')

const branch = run('git', ['branch', '--show-current'], { capture: true })
if (!branch) throw new Error('当前处于 detached HEAD，无法发起发布。')
const releaseBranch = String(process.env.PISPER_RELEASE_BRANCH || 'release').trim()
if (branch !== releaseBranch) {
  throw new Error(`只能从 ${releaseBranch} 分支发布，当前分支为 ${branch}。`)
}

run('git', ['fetch', '--tags', 'origin'])
let source = run('git', ['rev-parse', 'HEAD'], { capture: true })
let remoteSource = run('git', ['rev-parse', `origin/${releaseBranch}`], { capture: true })
if (source !== remoteSource) {
  throw new Error(
    `本地 ${releaseBranch} 必须与 origin/${releaseBranch} 完全同步后才能发布。` +
      `\n本地：${source}\n远端：${remoteSource}`,
  )
}

updateUpstreamPiDependency()
source = run('git', ['rev-parse', 'HEAD'], { capture: true })
remoteSource = run('git', ['rev-parse', `origin/${releaseBranch}`], { capture: true })
if (source !== remoteSource) {
  throw new Error(
    `自动更新依赖后 ${releaseBranch} 未与 origin/${releaseBranch} 同步。` +
      `\n本地：${source}\n远端：${remoteSource}`,
  )
}

const tags = run('git', ['tag', '--list', '--sort=-version:refname'], { capture: true })
  .split(/\r?\n/)
  .filter(Boolean)
const runGit = (gitArgs) => run('git', gitArgs, { capture: true })
const candidates = Object.keys(RELEASE_COMPONENTS)
const plans = []

for (const component of candidates) {
  const currentVersion = await readComponentVersion(root, component)
  const latestTag = fallbackReleaseTag(component, tags, currentVersion)
  const paths = componentReleasePaths(runGit, component, latestTag, source)
  if (paths.length === 0) continue

  console.log(`正在检查自 ${latestTag || '仓库初始提交'} 以来的 ${component} 实质性提交…`)
  const subjects = componentReleaseSubjects(runGit, component, latestTag, source)
  const substantive = subjects.filter(isSubstantiveReleaseCommit)
  if (substantive.length === 0) {
    console.log(`${component} 只有非实质性变更，自动跳过。`)
    continue
  }
  console.log(
    `已找到 ${substantive.length} 个 ${component} 实质性提交、${paths.length} 个变更文件：`,
  )
  for (const subject of substantive) console.log(`  - ${subject}`)

  const nextVersion = resolveVersion(currentVersion, input)
  if (compareVersions(nextVersion, currentVersion) <= 0) {
    throw new Error(`新版本 ${nextVersion} 必须高于当前 ${component} 版本 ${currentVersion}。`)
  }
  const tag = releaseTag(component, nextVersion)
  if (runGit(['tag', '--list', tag])) throw new Error(`标签 ${tag} 已经存在。`)
  if (latestTag) {
    const latestVersion = releaseVersionFromTag(component, latestTag) || latestTag.slice(1)
    if (compareVersions(nextVersion, latestVersion) <= 0) {
      throw new Error(`新版本 ${nextVersion} 必须高于最新 ${component} 标签 ${latestTag}。`)
    }
  }
  plans.push({ component, nextVersion, tag })
}

const appCurrentVersion = JSON.parse(await readFile(join(root, APP_VERSION_FILE), 'utf8')).version
const latestAppTag = tags.find((tag) => appTagPattern().test(tag)) || ''
const appPaths = appReleasePaths(runGit, latestAppTag, source)
if (appPaths.length > 0) {
  console.log(`正在检查自 ${latestAppTag || '仓库初始提交'} 以来的 app 实质性提交…`)
  const subjects = appReleaseSubjects(runGit, latestAppTag, source)
  const substantive = subjects.filter(isSubstantiveReleaseCommit)
  if (substantive.length === 0) {
    console.log('app 只有非实质性变更，自动跳过。')
  } else {
    console.log(`已找到 ${substantive.length} 个 app 实质性提交、${appPaths.length} 个变更文件：`)
    for (const subject of substantive) console.log(`  - ${subject}`)

    const nextVersion = resolveVersion(appCurrentVersion, input)
    if (compareVersions(nextVersion, appCurrentVersion) <= 0) {
      throw new Error(`新版本 ${nextVersion} 必须高于当前 app 版本 ${appCurrentVersion}。`)
    }
    const tag = appReleaseTag(nextVersion)
    if (runGit(['tag', '--list', tag])) throw new Error(`标签 ${tag} 已经存在。`)
    if (latestAppTag) {
      const latestVersion = appVersionFromTag(latestAppTag)
      if (latestVersion && compareVersions(nextVersion, latestVersion) <= 0) {
        throw new Error(`新版本 ${nextVersion} 必须高于最新 app 标签 ${latestAppTag}。`)
      }
    }
    plans.push({ component: 'app', nextVersion, tag })
  }
}

if (plans.length === 0) {
  throw new Error('未检测到 desktop、tui、runtime 或 app 的待发布产品变更。')
}

// Desktop 安装包内置最新 TUI/Runtime；任一组件发布时必须链式更新 Desktop，
// 否则新下载的安装包仍会携带旧组件。
if (
  !plans.some(({ component }) => component === 'desktop') &&
  plans.some(({ component }) => component === 'tui' || component === 'runtime')
) {
  const desktopCurrent = await readComponentVersion(root, 'desktop')
  const desktopNext = resolveVersion(desktopCurrent, input)
  const desktopTag = releaseTag('desktop', desktopNext)
  if (compareVersions(desktopNext, desktopCurrent) <= 0) {
    throw new Error(`新版本 ${desktopNext} 必须高于当前 desktop 版本 ${desktopCurrent}。`)
  }
  if (runGit(['tag', '--list', desktopTag])) throw new Error(`标签 ${desktopTag} 已经存在。`)
  const desktopLatestTag = fallbackReleaseTag('desktop', tags, desktopCurrent)
  if (desktopLatestTag) {
    const latestVersion =
      releaseVersionFromTag('desktop', desktopLatestTag) || desktopLatestTag.slice(1)
    if (compareVersions(desktopNext, latestVersion) <= 0) {
      throw new Error(`新版本 ${desktopNext} 必须高于最新 desktop 标签 ${desktopLatestTag}。`)
    }
  }
  plans.push({ component: 'desktop', nextVersion: desktopNext, tag: desktopTag })
  console.log(
    `检测到 TUI/Runtime 变更，安装包自动链式发布 desktop ${desktopNext}（内置最新组件）。`,
  )
}

// App workflow 接受前序组件版本提交，但组件 workflow 不接受 App 版本提交，
// 因此 App 必须最后执行；Desktop 则在它所捆绑的 TUI/Runtime 之后执行。
const releaseOrder = { tui: 0, runtime: 1, desktop: 2, app: 3 }
plans.sort((left, right) => releaseOrder[left.component] - releaseOrder[right.component])

const selectedComponents = plans.map(({ component }) => component)
const npmComponents = plans.filter(
  ({ component }) => component === 'runtime' || component === 'tui',
)
const chainNpm = npmComponents.length > 0
let npmReleaseVersion = ''
let npmTuiVersion = ''
let npmRuntimeVersion = ''
if (chainNpm) {
  const manifest = JSON.parse(
    await readFile(join(root, 'packages', 'pisper', 'package.json'), 'utf8'),
  )
  npmReleaseVersion = resolveVersion(manifest.version, input)
  if (compareVersions(npmReleaseVersion, manifest.version) <= 0) {
    throw new Error(`npm 新版本 ${npmReleaseVersion} 必须高于当前版本 ${manifest.version}。`)
  }
  npmTuiVersion =
    npmComponents.find(({ component }) => component === 'tui')?.nextVersion ||
    manifest.pisper.tuiVersion
  npmRuntimeVersion =
    npmComponents.find(({ component }) => component === 'runtime')?.nextVersion ||
    manifest.pisper.runtimeVersion
}
console.log(`自动发布通道：${selectedComponents.join('、')}`)
if (chainNpm) {
  console.log(
    `检测到 Runtime/TUI 组件变更，自动链式发布 pisper@${npmReleaseVersion}` +
      `（TUI ${npmTuiVersion} / Runtime ${npmRuntimeVersion}）。`,
  )
}
runComponentChecks(selectedComponents)
if (chainNpm) runNpm(['run', 'npm:pack:check'])

// Desktop 派发携带同批次的新 TUI/Runtime 版本，使安装包 staging 能把组件清单
// 更新视为实质性变更；本地脚本不写版本或推送，远端 workflow 负责原子提交。
const tuiPlan = plans.find(({ component }) => component === 'tui')
const runtimePlan = plans.find(({ component }) => component === 'runtime')
const desktopTuiVersion = tuiPlan?.nextVersion || ''
const desktopRuntimeVersion = runtimePlan?.nextVersion || ''

const npmDispatchIndex = npmComponents.length
  ? plans.findLastIndex(({ component }) => component === 'runtime' || component === 'tui')
  : -1

for (const [index, { component, nextVersion, tag }] of plans.entries()) {
  const workflow = component === 'app' ? 'release-app.yml' : 'release.yml'
  const output = run(
    'gh',
    [
      'workflow',
      'run',
      workflow,
      '--ref',
      releaseBranch,
      ...(component === 'app' ? [] : ['-f', `component=${component}`]),
      '-f',
      `version=${nextVersion}`,
      '-f',
      `source_sha=${source}`,
      ...(component === 'desktop' && (desktopTuiVersion || desktopRuntimeVersion)
        ? [
            '-f',
            `tui_version=${desktopTuiVersion}`,
            '-f',
            `runtime_version=${desktopRuntimeVersion}`,
          ]
        : []),
      ...(index === npmDispatchIndex
        ? [
            '-f',
            'publish_npm=true',
            '-f',
            `npm_version=${npmReleaseVersion}`,
            '-f',
            `tui_version=${npmTuiVersion}`,
            '-f',
            `runtime_version=${npmRuntimeVersion}`,
          ]
        : []),
    ],
    { capture: true },
  )
  const runId = output.match(/actions\/runs\/(\d+)/)?.[1]
  if (!runId) throw new Error(`无法从 GitHub CLI 输出识别 ${tag} 的 workflow run：${output}`)
  console.log(output)
  console.log(`已请求构建 ${tag}（源提交 ${source}，run ${runId}）。`)
  console.log(`等待 ${tag} 完成后再继续，避免全局发布队列取消排队任务…`)
  run('gh', ['run', 'watch', runId, '--exit-status'])
}

console.log(`已完成 ${selectedComponents.join('、')} 对应的质量门禁和平台产物发布。`)
console.log('各版本文件与 tag 均在资产验证后由对应 workflow 原子更新。')
if (chainNpm) console.log(`已自动链式发布 pisper@${npmReleaseVersion}。`)
