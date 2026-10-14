import { execFileSync } from 'node:child_process'
import { readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { assertHasSubstantiveReleaseCommits } from './release-policy.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const manifestPath = join(root, 'packages', 'pisper', 'package.json')
const targetVersion = String(process.argv[2] || '').trim()
const targetTuiVersion = String(process.argv[3] || '').trim()
const targetRuntimeVersion = String(process.argv[4] || '').trim()
const expectedSource = String(process.argv[5] || '').trim()

function git(args) {
  return execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: 'pipe' }).trim()
}

function validVersion(value, label) {
  if (!/^\d+\.\d+\.\d+$/.test(value)) throw new Error(`${label} must use X.Y.Z format: ${value}`)
}

function compareVersions(left, right) {
  const a = left.split('.').map(Number)
  const b = right.split('.').map(Number)
  for (let index = 0; index < 3; index += 1) {
    if (a[index] !== b[index]) return a[index] - b[index]
  }
  return 0
}

validVersion(targetVersion, 'npm version')
if (targetTuiVersion) validVersion(targetTuiVersion, 'TUI version')
if (targetRuntimeVersion) validVersion(targetRuntimeVersion, 'Runtime version')
const source = git(['rev-parse', 'HEAD'])
if (expectedSource && source !== git(['rev-parse', expectedSource])) {
  throw new Error(`npm release source mismatch: expected ${expectedSource}, found ${source}`)
}

const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
if (compareVersions(targetVersion, manifest.version) <= 0) {
  throw new Error(`npm version ${targetVersion} must be higher than ${manifest.version}.`)
}
const tag = `npm-v${targetVersion}`
if (git(['tag', '--list', tag])) throw new Error(`Tag already exists: ${tag}`)
const latestTag = git(['tag', '--list', 'npm-v*', '--sort=-version:refname']).split(/\r?\n/)[0]
if (latestTag) {
  const latestVersion = latestTag.slice('npm-v'.length)
  if (compareVersions(targetVersion, latestVersion) <= 0) {
    throw new Error(`npm version ${targetVersion} must be higher than ${latestTag}.`)
  }
}
const range = latestTag ? `${latestTag}..${source}` : source
const subjects = git(['log', range, '--pretty=format:%s']).split(/\r?\n/).filter(Boolean)
assertHasSubstantiveReleaseCommits(subjects, latestTag || 'repository start')
const changedPaths = git(
  latestTag
    ? ['diff', '--name-only', '--diff-filter=ACMRTUXB', `${latestTag}..${source}`]
    : ['ls-tree', '-r', '--name-only', source],
)
  .split(/\r?\n/)
  .filter(Boolean)
const hasNpmChanges = changedPaths.some(
  (path) =>
    path.startsWith('packages/pisper/') ||
    path === '.github/workflows/publish-npm.yml' ||
    /^scripts\/(?:package-npm|stage-npm-release-version|validate-npm-)/.test(path),
)
const changesComponentTarget =
  (targetTuiVersion && targetTuiVersion !== manifest.pisper.tuiVersion) ||
  (targetRuntimeVersion && targetRuntimeVersion !== manifest.pisper.runtimeVersion)
if (!hasNpmChanges && !changesComponentTarget) {
  throw new Error('npm release has no launcher changes and does not select a new TUI or Runtime.')
}

manifest.version = targetVersion
if (targetTuiVersion) manifest.pisper.tuiVersion = targetTuiVersion
if (targetRuntimeVersion) manifest.pisper.runtimeVersion = targetRuntimeVersion
await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8')
console.log(
  `Staged npm-v${targetVersion} with TUI ${manifest.pisper.tuiVersion} and Runtime ${manifest.pisper.runtimeVersion}.`,
)
