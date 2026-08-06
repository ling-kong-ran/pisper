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
if (dirty) throw new Error('发布前tracked工作区必须保持干净，请先提交或处理现有修改。')

const branch = run('git', ['branch', '--show-current'], { capture: true })
if (!branch) throw new Error('当前处于 detached HEAD，无法创建版本提交。')
const releaseBranch = String(process.env.PISPER_RELEASE_BRANCH || 'release').trim()
if (branch !== releaseBranch)
  throw new Error(`只能从 ${releaseBranch} 分支发布，当前分支为 ${branch}。`)

run('git', ['fetch', '--tags', 'origin'])

let upstream = ''
try {
  upstream = run('git', ['rev-parse', '--abbrev-ref', '--symbolic-full-name', '@{upstream}'], {
    capture: true,
  })
} catch {
  try {
    run('git', ['show-ref', '--verify', '--quiet', `refs/remotes/origin/${branch}`])
    upstream = `origin/${branch}`
  } catch {
    console.log(`远端 ${branch} 分支尚不存在，将在发布时创建。`)
  }
}
if (upstream) {
  const behind = Number(run('git', ['rev-list', '--count', `HEAD..${upstream}`], { capture: true }))
  if (behind > 0) throw new Error(`当前分支落后于 ${upstream} ${behind} 个提交，请先同步远端。`)
}

const latestTag = run('git', ['tag', '--list', 'v*', '--sort=-version:refname'], { capture: true })
  .split(/\r?\n/)
  .find((value) => /^v\d+\.\d+\.\d+$/.test(value))

if (latestTag) {
  console.log(`正在检查自 ${latestTag} 以来的实质性提交…`)
  const subjects = run('git', ['log', '--format=%s', `${latestTag}..HEAD`], { capture: true })
    .split(/\r?\n/)
    .map((value) => value.trim())
    .filter(Boolean)
  const substantive = assertHasSubstantiveReleaseCommits(subjects, latestTag)
  console.log(`已找到 ${substantive.length} 个实质性提交：`)
  for (const subject of substantive) console.log(`  - ${subject}`)
} else {
  console.log('未找到已有版本标签，允许作为首次发布继续。')
}

console.log('正在刷新发布依赖…')
runNpm(['update'])
run('cargo', ['update', '--manifest-path', join(root, 'src-tui', 'Cargo.toml')])

console.log('正在执行发布前检查…')
runNpm(['test'])
runNpm(['run', 'check'])
runNpm(['run', 'build'])
runNpm(['run', 'tui:test'])
runNpm(['run', 'tui:check'])

const dependencyFiles = [
  'package.json',
  'package-lock.json',
  'src-tui/Cargo.toml',
  'src-tui/Cargo.lock',
]
if (run('git', ['status', '--porcelain', '--', ...dependencyFiles], { capture: true })) {
  run('git', ['add', '--', ...dependencyFiles])
  run('git', ['commit', '-m', 'chore(deps): refresh release dependencies'])
}
const unrelatedChanges = run('git', ['status', '--porcelain', '--untracked-files=no'], {
  capture: true,
})
if (unrelatedChanges) throw new Error(`依赖刷新产生了未纳入发布的文件：\n${unrelatedChanges}`)

const packageJson = JSON.parse(await readFile(packagePath, 'utf8'))
const nextVersion = resolveVersion(packageJson.version, input)
if (compareVersions(nextVersion, packageJson.version) <= 0) {
  throw new Error(`新版本 ${nextVersion} 必须高于当前版本 ${packageJson.version}。`)
}
const tag = `v${nextVersion}`
if (run('git', ['tag', '--list', tag], { capture: true })) throw new Error(`标签 ${tag} 已经存在。`)
if (latestTag && compareVersions(nextVersion, latestTag.replace(/^v/i, '')) <= 0) {
  throw new Error(`新版本 ${nextVersion} 必须高于最新标签 ${latestTag}。`)
}

const versionOutput = runNpm(['version', nextVersion, '--message', 'chore(release): v%s'], {
  capture: true,
})
const bumpedVersion = versionOutput
  .split(/\r?\n/)
  .map((line) => line.trim())
  .filter(Boolean)
  .at(-1)
  ?.replace(/^v/i, '')
if (bumpedVersion !== nextVersion) throw new Error(`npm version 返回了意外版本：${versionOutput}`)

const synchronizedClientFiles = ['src-tui/Cargo.toml', 'src-tui/Cargo.lock']
if (run('git', ['status', '--porcelain', '--', ...synchronizedClientFiles], { capture: true })) {
  run('git', ['add', '--', ...synchronizedClientFiles])
  run('git', ['commit', '--amend', '--no-edit'])
  run('git', ['tag', '--force', tag])
}

try {
  run('git', ['push', '--atomic', 'origin', `HEAD:${branch}`, tag])
} catch (error) {
  console.error(`版本提交和标签 ${tag} 已在本地创建，但推送失败。网络或权限恢复后可执行：`)
  console.error(`git push --atomic origin HEAD:${branch} ${tag}`)
  throw error
}

console.log(`已发布 ${tag}。GitHub Actions 将生成更新日志并构建全平台安装包。`)
