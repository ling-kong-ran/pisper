// 移动端 App 独立发版：与 npm run release（desktop/tui/runtime）互不影响。
// 门禁与主发布脚本一致：release 分支、工作区干净、与远端同步、自上一 app-v 标签
// 以来有 App 独属路径的实质性提交。通过 gh 派发 .github/workflows/release-app.yml。
// 用法：npm run app:release -- <major|minor|patch|X.Y.Z>
import { execFileSync } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  APP_VERSION_FILE,
  appReleaseTag,
  appTagPattern,
  appVersionFromTag,
  isAppOwnedPath,
} from './app-paths.mjs'
import { isSubstantiveReleaseCommit } from './release-policy.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const releaseBranch = String(process.env.PISPER_RELEASE_BRANCH || 'release').trim()
const npmCli = String(process.env.npm_execpath || '').trim()
const input = process.argv[2] || 'patch'

function run(command, args, { capture = false } = {}) {
  const result = execFileSync(command, args, {
    cwd: root,
    encoding: 'utf8',
    stdio: capture ? 'pipe' : 'inherit',
  })
  return typeof result === 'string' ? result.trim() : ''
}
const runGit = (args) => run('git', args, { capture: true })
function runNpm(args) {
  if (npmCli) return run(process.execPath, [npmCli, ...args])
  return run(process.platform === 'win32' ? 'npm.cmd' : 'npm', args)
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

function splitLines(value) {
  return String(value || '')
    .split(/\r?\n/)
    .map((item) => item.trim())
    .filter(Boolean)
}

assertVersionInput(input)

const dirty = runGit(['status', '--porcelain', '--untracked-files=no'])
if (dirty) throw new Error('发布前 tracked 工作区必须保持干净，请先提交或处理现有修改。')

const branch = runGit(['branch', '--show-current'])
if (!branch) throw new Error('当前处于 detached HEAD，无法发起发布。')
if (branch !== releaseBranch) {
  throw new Error(`只能从 ${releaseBranch} 分支发布，当前分支为 ${branch}。`)
}

run('git', ['fetch', '--tags', 'origin'])
const source = runGit(['rev-parse', 'HEAD'])
const remoteSource = runGit(['rev-parse', `origin/${releaseBranch}`])
if (source !== remoteSource) {
  throw new Error(`本地 ${releaseBranch} 必须与 origin/${releaseBranch} 完全同步后才能发布。`)
}

const currentVersion = JSON.parse(await readFile(join(root, APP_VERSION_FILE), 'utf8')).version
const tags = splitLines(runGit(['tag', '--list', '--sort=-version:refname']))
const latestTag = tags.find((tag) => appTagPattern().test(tag)) || ''

// 收集自上一 App 标签以来触及 App 独属路径的实质性提交。
const range = latestTag ? `${latestTag}..${source}` : source
const commits = splitLines(runGit(['log', '--format=%H', range]))
const substantive = []
for (const commit of commits) {
  const paths = splitLines(
    runGit([
      'diff-tree',
      '--root',
      '--no-commit-id',
      '--name-only',
      '--diff-filter=ACMRTUXB',
      '-r',
      commit,
    ]),
  )
  if (!paths.some(isAppOwnedPath)) continue
  const subject = runGit(['show', '-s', '--format=%s', commit])
  if (subject && isSubstantiveReleaseCommit(subject)) substantive.push(subject)
}
if (substantive.length === 0) {
  throw new Error(
    `自 ${latestTag || '仓库初始提交'} 以来没有 App 独属路径的实质性提交，无需发布 App。`,
  )
}

const nextVersion = resolveVersion(currentVersion, input)
if (compareVersions(nextVersion, currentVersion) <= 0) {
  throw new Error(`新版本 ${nextVersion} 必须高于当前 App 版本 ${currentVersion}。`)
}
if (latestTag) {
  const latestVersion = appVersionFromTag(latestTag)
  if (latestVersion && compareVersions(nextVersion, latestVersion) <= 0) {
    throw new Error(`新版本 ${nextVersion} 必须高于最新 App 标签 ${latestTag}。`)
  }
}
const tag = appReleaseTag(nextVersion)
if (runGit(['tag', '--list', tag])) throw new Error(`标签 ${tag} 已经存在。`)

console.log(`将发布 ${tag}（源提交 ${source}），实质性提交 ${substantive.length} 个：`)
for (const subject of substantive) console.log(`  - ${subject}`)

console.log('正在执行 App 发布前检查…')
runNpm(['test'])
runNpm(['run', 'check'])
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

const output = run(
  'gh',
  [
    'workflow',
    'run',
    'release-app.yml',
    '--ref',
    releaseBranch,
    '-f',
    `version=${nextVersion}`,
    '-f',
    `source_sha=${source}`,
  ],
  { capture: true },
)
const runId = output.match(/actions\/runs\/(\d+)/)?.[1]
console.log(output)
console.log(`已请求构建 ${tag}${runId ? `（run ${runId}）` : ''}。`)
console.log('可执行 gh run list --workflow release-app.yml --limit 3 查看发布进度。')
