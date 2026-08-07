import { execFileSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { componentReleasePaths, componentReleaseSubjects } from './release-changes.mjs'
import {
  assertHasSubstantiveReleaseCommits,
  isSubstantiveReleaseCommit,
} from './release-policy.mjs'
import {
  RELEASE_COMPONENTS,
  fallbackReleaseTag,
  readComponentVersion,
  releaseTag,
  releaseVersionFromTag,
} from './release-components.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const npmCli = String(process.env.npm_execpath || '').trim()
const args = process.argv.slice(2).filter((value) => !value.startsWith('--'))
const requestedComponent = RELEASE_COMPONENTS[args[0]] ? args.shift() : ''
if (args[0] === 'auto') args.shift()
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
  if (selected.includes('desktop') || selected.includes('runtime')) {
    runNpm(['test'])
    runNpm(['run', 'check'])
    runNpm(['run', 'build'])
  }
  if (selected.includes('desktop') || selected.includes('tui')) {
    run('cargo', ['fmt', '--manifest-path', 'src-tui/Cargo.toml', '--', '--check'])
    runNpm(['run', 'tui:test'])
    runNpm(['run', 'tui:check'])
  }
  if (selected.includes('desktop')) {
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
const source = run('git', ['rev-parse', 'HEAD'], { capture: true })
const remoteSource = run('git', ['rev-parse', `origin/${releaseBranch}`], { capture: true })
if (source !== remoteSource) {
  throw new Error(
    `本地 ${releaseBranch} 必须与 origin/${releaseBranch} 完全同步后才能发布。` +
      `\n本地：${source}\n远端：${remoteSource}`,
  )
}

const tags = run('git', ['tag', '--list', '--sort=-version:refname'], { capture: true })
  .split(/\r?\n/)
  .filter(Boolean)
const runGit = (gitArgs) => run('git', gitArgs, { capture: true })
const candidates = requestedComponent ? [requestedComponent] : Object.keys(RELEASE_COMPONENTS)
const plans = []

for (const component of candidates) {
  const latestTag = fallbackReleaseTag(component, tags)
  const paths = componentReleasePaths(runGit, component, latestTag, source)
  if (!requestedComponent && paths.length === 0) continue
  if (requestedComponent && paths.length === 0) {
    throw new Error(
      `${component} 自 ${latestTag || '仓库初始提交'} 以来没有归属于该组件的变更，无需发布。`,
    )
  }

  console.log(`正在检查自 ${latestTag || '仓库初始提交'} 以来的 ${component} 实质性提交…`)
  const subjects = componentReleaseSubjects(runGit, component, latestTag, source)
  const substantive = requestedComponent
    ? assertHasSubstantiveReleaseCommits(subjects, latestTag || '仓库初始提交')
    : subjects.filter(isSubstantiveReleaseCommit)
  if (substantive.length === 0) {
    console.log(`${component} 只有非实质性变更，自动跳过。`)
    continue
  }
  console.log(
    `已找到 ${substantive.length} 个 ${component} 实质性提交、${paths.length} 个变更文件：`,
  )
  for (const subject of substantive) console.log(`  - ${subject}`)

  const currentVersion = await readComponentVersion(root, component)
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

if (plans.length === 0) {
  throw new Error('未检测到 desktop、tui 或 runtime 的待发布产品变更。')
}

const selectedComponents = plans.map(({ component }) => component)
console.log(`${requestedComponent ? '发布组件' : '自动发布组件'}：${selectedComponents.join('、')}`)
runComponentChecks(selectedComponents)

for (const [index, { component, nextVersion, tag }] of plans.entries()) {
  const output = run(
    'gh',
    [
      'workflow',
      'run',
      'release.yml',
      '--ref',
      releaseBranch,
      '-f',
      `component=${component}`,
      '-f',
      `version=${nextVersion}`,
      '-f',
      `source_sha=${source}`,
    ],
    { capture: true },
  )
  const runId = output.match(/actions\/runs\/(\d+)/)?.[1]
  if (!runId) throw new Error(`无法从 GitHub CLI 输出识别 ${tag} 的 workflow run：${output}`)
  console.log(output)
  console.log(`已请求构建 ${tag}（源提交 ${source}，run ${runId}）。`)

  if (index < plans.length - 1) {
    console.log(`等待 ${tag} 成功后再派发下一个组件，避免 GitHub 取消排队任务…`)
    run('gh', ['run', 'watch', runId, '--exit-status'])
  }
}

console.log(`只会执行 ${selectedComponents.join('、')} 对应的质量门禁和平台产物构建。`)
console.log('多个组件任务会依次派发；各自的版本文件和 tag 仍在资产验证后原子更新。')
console.log(`可执行 gh run list --workflow release.yml --limit ${plans.length} 查看发布进度。`)
