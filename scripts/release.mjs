import { execFileSync } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { assertHasSubstantiveReleaseCommits } from './release-policy.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const packagePath = join(root, 'package.json')
const npmCli = String(process.env.npm_execpath || '').trim()
const input = process.argv.slice(2).find((value) => !value.startsWith('--')) || 'patch'

function run(command, args, { capture = false } = {}) {
  const result = execFileSync(command, args, {
    cwd: root,
    encoding: 'utf8',
    stdio: capture ? 'pipe' : 'inherit',
  })
  return typeof result === 'string' ? result.trim() : ''
}

function runNpm(args, options) {
  if (npmCli) return run(process.execPath, [npmCli, ...args], options)
  return run(process.platform === 'win32' ? 'npm.cmd' : 'npm', args, options)
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

const latestTag = run('git', ['tag', '--list', 'v*', '--sort=-version:refname'], { capture: true })
  .split(/\r?\n/)
  .find((value) => /^v\d+\.\d+\.\d+$/.test(value))
if (latestTag) {
  console.log(`正在检查自 ${latestTag} 以来的实质性提交…`)
  const subjects = run('git', ['log', '--format=%s', `${latestTag}..${source}`], { capture: true })
    .split(/\r?\n/)
    .map((value) => value.trim())
    .filter(Boolean)
  const substantive = assertHasSubstantiveReleaseCommits(subjects, latestTag)
  console.log(`已找到 ${substantive.length} 个实质性提交：`)
  for (const subject of substantive) console.log(`  - ${subject}`)
} else {
  console.log('未找到已有版本标签，允许作为首次发布继续。')
}

const currentVersion = JSON.parse(await readFile(packagePath, 'utf8')).version
const nextVersion = resolveVersion(currentVersion, input)
if (compareVersions(nextVersion, currentVersion) <= 0) {
  throw new Error(`新版本 ${nextVersion} 必须高于当前版本 ${currentVersion}。`)
}
const tag = `v${nextVersion}`
if (run('git', ['tag', '--list', tag], { capture: true })) throw new Error(`标签 ${tag} 已经存在。`)
if (latestTag && compareVersions(nextVersion, latestTag.slice(1)) <= 0) {
  throw new Error(`新版本 ${nextVersion} 必须高于最新标签 ${latestTag}。`)
}

console.log('正在执行发布前检查…')
runNpm(['test'])
runNpm(['run', 'check'])
runNpm(['run', 'build'])
runNpm(['run', 'tui:test'])
runNpm(['run', 'tui:check'])

run('gh', [
  'workflow',
  'run',
  'release.yml',
  '--ref',
  releaseBranch,
  '-f',
  `version=${nextVersion}`,
  '-f',
  `source_sha=${source}`,
])

console.log(`已请求构建 ${tag}（源提交 ${source}）。`)
console.log('版本文件、release 分支和 tag 只会在全部平台构建及资产校验成功后更新。')
console.log('可执行 gh run list --workflow release.yml --limit 1 查看发布进度。')
