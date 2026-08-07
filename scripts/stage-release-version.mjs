import { execFileSync } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { assertHasSubstantiveReleaseCommits } from './release-policy.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const targetVersion = String(process.argv[2] || '').trim()
const expectedSource = String(process.argv[3] || '').trim()

function run(command, args) {
  return execFileSync(command, args, {
    cwd: root,
    encoding: 'utf8',
    stdio: 'pipe',
  }).trim()
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

if (!/^\d+\.\d+\.\d+$/.test(targetVersion)) {
  throw new Error(`发布版本无效：${targetVersion}。版本必须使用 X.Y.Z 格式。`)
}

const source = run('git', ['rev-parse', 'HEAD'])
if (expectedSource && source !== run('git', ['rev-parse', expectedSource])) {
  throw new Error(`发布源提交不一致：期望 ${expectedSource}，实际 ${source}。`)
}

const packagePath = join(root, 'package.json')
const currentVersion = JSON.parse(await readFile(packagePath, 'utf8')).version
if (compareVersions(targetVersion, currentVersion) <= 0) {
  throw new Error(`新版本 ${targetVersion} 必须高于当前版本 ${currentVersion}。`)
}

const tag = `v${targetVersion}`
if (run('git', ['tag', '--list', tag])) throw new Error(`标签 ${tag} 已经存在。`)

const latestTag = run('git', ['tag', '--list', 'v*', '--sort=-version:refname'])
  .split(/\r?\n/)
  .find((value) => /^v\d+\.\d+\.\d+$/.test(value))
if (latestTag) {
  if (compareVersions(targetVersion, latestTag.slice(1)) <= 0) {
    throw new Error(`新版本 ${targetVersion} 必须高于最新标签 ${latestTag}。`)
  }
  const subjects = run('git', ['log', '--format=%s', `${latestTag}..${source}`])
    .split(/\r?\n/)
    .map((value) => value.trim())
    .filter(Boolean)
  assertHasSubstantiveReleaseCommits(subjects, latestTag)
}

const npmArgs = ['version', targetVersion, '--no-git-tag-version']
const npmCli = String(process.env.npm_execpath || '').trim()
if (npmCli) {
  execFileSync(process.execPath, [npmCli, ...npmArgs], { cwd: root, stdio: 'inherit' })
} else if (process.platform === 'win32') {
  execFileSync(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', `npm ${npmArgs.join(' ')}`], {
    cwd: root,
    stdio: 'inherit',
  })
} else {
  execFileSync('npm', npmArgs, { cwd: root, stdio: 'inherit' })
}

const [packageJson, packageLock, cargoManifest, cargoLock] = await Promise.all([
  readFile(packagePath, 'utf8').then(JSON.parse),
  readFile(join(root, 'package-lock.json'), 'utf8').then(JSON.parse),
  readFile(join(root, 'src-tui', 'Cargo.toml'), 'utf8'),
  readFile(join(root, 'src-tui', 'Cargo.lock'), 'utf8'),
])
if (packageJson.version !== targetVersion || packageLock.version !== targetVersion) {
  throw new Error('npm release version files were not synchronized.')
}
const manifestVersion = cargoManifest.match(/\[package\][\s\S]*?\r?\nversion\s*=\s*"([^"]+)"/)?.[1]
const lockVersion = cargoLock.match(
  /\[\[package\]\]\s*\r?\nname\s*=\s*"pisper-tui"\s*\r?\nversion\s*=\s*"([^"]+)"/,
)?.[1]
if (manifestVersion !== targetVersion) {
  throw new Error('src-tui/Cargo.toml release version was not synchronized.')
}
if (lockVersion !== targetVersion) {
  throw new Error('src-tui/Cargo.lock release version was not synchronized.')
}

console.log(`已在临时工作区暂存 ${tag}；尚未创建版本提交或标签。`)
