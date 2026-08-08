import { execFileSync } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { assertHasSubstantiveReleaseCommits } from './release-policy.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const manifest = JSON.parse(
  await readFile(join(root, 'packages', 'pisper', 'package.json'), 'utf8'),
)
const rawArgs = process.argv.slice(2)
const input = rawArgs.find((value) => !value.startsWith('--')) || 'patch'
const tuiVersion =
  rawArgs.find((value) => value.startsWith('--tui='))?.slice(6) || manifest.pisper.tuiVersion
const runtimeVersion =
  rawArgs.find((value) => value.startsWith('--runtime='))?.slice(10) ||
  manifest.pisper.runtimeVersion

function run(command, args, { capture = false } = {}) {
  const result = execFileSync(command, args, {
    cwd: root,
    encoding: 'utf8',
    stdio: capture ? 'pipe' : 'inherit',
  })
  return typeof result === 'string' ? result.trim() : ''
}

function npm(args) {
  const npmCli = String(process.env.npm_execpath || '').trim()
  if (npmCli) return run(process.execPath, [npmCli, ...args])
  return run(process.platform === 'win32' ? 'npm.cmd' : 'npm', args)
}

function validVersion(value, label) {
  if (!/^\d+\.\d+\.\d+$/.test(value)) throw new Error(`${label} must use X.Y.Z format: ${value}`)
}

function nextVersion(current, target) {
  if (/^\d+\.\d+\.\d+$/.test(target)) return target
  if (!['major', 'minor', 'patch'].includes(target)) {
    throw new Error('Use major, minor, patch, or an explicit X.Y.Z npm version.')
  }
  const [major, minor, patch] = current.split('.').map(Number)
  if (target === 'major') return `${major + 1}.0.0`
  if (target === 'minor') return `${major}.${minor + 1}.0`
  return `${major}.${minor}.${patch + 1}`
}

function compareVersions(left, right) {
  const a = left.split('.').map(Number)
  const b = right.split('.').map(Number)
  for (let index = 0; index < 3; index += 1) {
    if (a[index] !== b[index]) return a[index] - b[index]
  }
  return 0
}

validVersion(tuiVersion, 'TUI version')
validVersion(runtimeVersion, 'Runtime version')
const target = nextVersion(manifest.version, input)
if (compareVersions(target, manifest.version) <= 0) {
  throw new Error(`npm version ${target} must be higher than ${manifest.version}.`)
}
const dirty = run('git', ['status', '--porcelain', '--untracked-files=no'], { capture: true })
if (dirty) throw new Error('Tracked worktree must be clean before an npm release.')
const branch = run('git', ['branch', '--show-current'], { capture: true })
const releaseBranch = String(process.env.PISPER_RELEASE_BRANCH || 'release')
if (branch !== releaseBranch)
  throw new Error(`npm releases must run from ${releaseBranch}, not ${branch}.`)
run('git', ['fetch', '--tags', 'origin'])
const source = run('git', ['rev-parse', 'HEAD'], { capture: true })
const remote = run('git', ['rev-parse', `origin/${releaseBranch}`], { capture: true })
if (source !== remote)
  throw new Error(`${releaseBranch} must exactly match origin/${releaseBranch}.`)
const latestTag = run('git', ['tag', '--list', 'npm-v*', '--sort=-version:refname'], {
  capture: true,
}).split(/\r?\n/)[0]
if (latestTag && compareVersions(target, latestTag.slice(5)) <= 0) {
  throw new Error(`npm version ${target} must be higher than ${latestTag}.`)
}
const subjects = run(
  'git',
  ['log', latestTag ? `${latestTag}..${source}` : source, '--pretty=format:%s'],
  { capture: true },
)
  .split(/\r?\n/)
  .filter(Boolean)
assertHasSubstantiveReleaseCommits(subjects, latestTag || 'repository start')
const changedPaths = run(
  'git',
  latestTag
    ? ['diff', '--name-only', '--diff-filter=ACMRTUXB', `${latestTag}..${source}`]
    : ['ls-tree', '-r', '--name-only', source],
  { capture: true },
)
  .split(/\r?\n/)
  .filter(Boolean)
const hasNpmChanges = changedPaths.some(
  (path) =>
    path.startsWith('packages/pisper/') ||
    path === '.github/workflows/publish-npm.yml' ||
    /^scripts\/(?:package-npm|release-npm|stage-npm-release-version|validate-npm-)/.test(path),
)
if (
  !hasNpmChanges &&
  tuiVersion === manifest.pisper.tuiVersion &&
  runtimeVersion === manifest.pisper.runtimeVersion
) {
  throw new Error('npm release has no launcher changes and does not select a new TUI or Runtime.')
}

npm(['run', 'npm:pack:check'])
run('node', ['scripts/validate-npm-targets.mjs'])
const output = run(
  'gh',
  [
    'workflow',
    'run',
    'publish-npm.yml',
    '--ref',
    releaseBranch,
    '-f',
    `npm_version=${target}`,
    '-f',
    `tui_version=${tuiVersion}`,
    '-f',
    `runtime_version=${runtimeVersion}`,
    '-f',
    `source_sha=${source}`,
  ],
  { capture: true },
)
console.log(output)
console.log(`Requested npm-v${target} for TUI ${tuiVersion} and Runtime ${runtimeVersion}.`)
