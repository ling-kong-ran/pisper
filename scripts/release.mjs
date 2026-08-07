import { execFileSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { assertHasSubstantiveReleaseCommits } from './release-policy.mjs'
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
const component = RELEASE_COMPONENTS[args[0]] ? args.shift() : 'desktop'
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
  console.log(`正在执行 ${selected} 发布前检查…`)
  if (selected === 'desktop') {
    runNpm(['test'])
    runNpm(['run', 'check'])
    runNpm(['run', 'build'])
    runNpm(['run', 'tui:test'])
    runNpm(['run', 'tui:check'])
    return
  }
  if (selected === 'server') {
    runNpm(['test'])
    runNpm(['run', 'check'])
    runNpm(['run', 'build'])
    return
  }
  run('cargo', ['fmt', '--manifest-path', 'src-tui/Cargo.toml', '--', '--check'])
  runNpm(['run', 'tui:test'])
  runNpm(['run', 'tui:check'])
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
const latestTag = fallbackReleaseTag(component, tags)
if (latestTag) {
  console.log(`正在检查自 ${latestTag} 以来的 ${component} 实质性提交…`)
  const subjects = run('git', ['log', '--format=%s', `${latestTag}..${source}`], { capture: true })
    .split(/\r?\n/)
    .map((value) => value.trim())
    .filter(Boolean)
  const substantive = assertHasSubstantiveReleaseCommits(subjects, latestTag)
  console.log(`已找到 ${substantive.length} 个实质性提交：`)
  for (const subject of substantive) console.log(`  - ${subject}`)
} else {
  console.log(`未找到已有 ${component} 版本标签，允许作为首次组件发布继续。`)
}

const currentVersion = await readComponentVersion(root, component)
const nextVersion = resolveVersion(currentVersion, input)
if (compareVersions(nextVersion, currentVersion) <= 0) {
  throw new Error(`新版本 ${nextVersion} 必须高于当前 ${component} 版本 ${currentVersion}。`)
}
const tag = releaseTag(component, nextVersion)
if (run('git', ['tag', '--list', tag], { capture: true })) throw new Error(`标签 ${tag} 已经存在。`)
if (latestTag) {
  const latestVersion = releaseVersionFromTag(component, latestTag) || latestTag.slice(1)
  if (compareVersions(nextVersion, latestVersion) <= 0) {
    throw new Error(`新版本 ${nextVersion} 必须高于最新 ${component} 标签 ${latestTag}。`)
  }
}

runComponentChecks(component)

run('gh', [
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
])

console.log(`已请求构建 ${tag}（源提交 ${source}）。`)
console.log(`只会执行 ${component} 对应的质量门禁和平台产物构建。`)
console.log('版本文件、release 分支和 tag 只会在全部平台构建及资产校验成功后更新。')
console.log('可执行 gh run list --workflow release.yml --limit 1 查看发布进度。')
