import { execFileSync } from 'node:child_process'
import { readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { componentReleasePaths, componentReleaseSubjects } from './release-changes.mjs'
import { assertHasSubstantiveReleaseCommits } from './release-policy.mjs'
import {
  assertReleaseComponent,
  fallbackReleaseTag,
  readComponentVersion,
  releaseTag,
  releaseVersionFromTag,
} from './release-components.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const component = assertReleaseComponent(process.argv[2])
const targetVersion = String(process.argv[3] || '').trim()
const expectedSource = String(process.argv[4] || '').trim()

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

async function stageTuiVersion(version) {
  const manifestPath = join(root, 'src-tui', 'Cargo.toml')
  const lockPath = join(root, 'src-tui', 'Cargo.lock')
  const [manifest, lock] = await Promise.all([
    readFile(manifestPath, 'utf8'),
    readFile(lockPath, 'utf8'),
  ])
  const updatedManifest = manifest.replace(
    /(\[package\][\s\S]*?\r?\nversion\s*=\s*")[^"]+("\s*\r?\n)/,
    `$1${version}$2`,
  )
  const updatedLock = lock.replace(
    /(\[\[package\]\]\s*\r?\nname\s*=\s*"pisper-tui"\s*\r?\nversion\s*=\s*")[^"]+("\s*\r?\n)/,
    `$1${version}$2`,
  )
  if (updatedManifest === manifest || updatedLock === lock) {
    throw new Error('Unable to stage the Pisper TUI release version.')
  }
  await Promise.all([
    writeFile(manifestPath, updatedManifest, 'utf8'),
    writeFile(lockPath, updatedLock, 'utf8'),
  ])
}

function stageRuntimeVersion(version) {
  const npmArgs = ['version', version, '--no-git-tag-version', '--ignore-scripts']
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
}

async function stageDesktopVersion(version) {
  const path = join(root, 'src-tauri', 'desktop-package.json')
  const value = JSON.parse(await readFile(path, 'utf8'))
  value.version = version
  // Sync the bundled component manifest when the workflow was dispatched with
  // the newly released TUI/Runtime versions (auto-chained installer release).
  const tuiVersion = String(process.env.RELEASE_TUI_VERSION || '').trim()
  const runtimeVersion = String(process.env.RELEASE_RUNTIME_VERSION || '').trim()
  const bundled = { ...(value.bundled || {}) }
  if (tuiVersion) bundled.tui = tuiVersion
  if (runtimeVersion) bundled.runtime = runtimeVersion
  value.bundled = bundled
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`, 'utf8')
}

if (!/^\d+\.\d+\.\d+$/.test(targetVersion)) {
  throw new Error(`发布版本无效：${targetVersion}。版本必须使用 X.Y.Z 格式。`)
}

const source = run('git', ['rev-parse', 'HEAD'])
if (expectedSource && source !== run('git', ['rev-parse', expectedSource])) {
  throw new Error(`发布源提交不一致：期望 ${expectedSource}，实际 ${source}。`)
}

const currentVersion = await readComponentVersion(root, component)
if (compareVersions(targetVersion, currentVersion) <= 0) {
  throw new Error(`${component} 新版本 ${targetVersion} 必须高于当前版本 ${currentVersion}。`)
}

const tag = releaseTag(component, targetVersion)
if (run('git', ['tag', '--list', tag])) throw new Error(`标签 ${tag} 已经存在。`)

const tags = run('git', ['tag', '--list', '--sort=-version:refname']).split(/\r?\n/).filter(Boolean)
const latestTag = fallbackReleaseTag(component, tags, currentVersion)
if (latestTag) {
  const latestVersion = releaseVersionFromTag(component, latestTag) || latestTag.slice(1)
  if (compareVersions(targetVersion, latestVersion) <= 0) {
    throw new Error(`新版本 ${targetVersion} 必须高于最新 ${component} 标签 ${latestTag}。`)
  }
  const runGit = (args) => run('git', args)
  const paths = componentReleasePaths(runGit, component, latestTag, source)
  if (paths.length === 0) {
    // The desktop installer bundles the newest published TUI/Runtime
    // components; a dispatch carrying new component versions is a
    // substantive installer change even when no desktop path changed.
    const tuiVersion = String(process.env.RELEASE_TUI_VERSION || '').trim()
    const runtimeVersion = String(process.env.RELEASE_RUNTIME_VERSION || '').trim()
    if (component !== 'desktop' || (!tuiVersion && !runtimeVersion)) {
      throw new Error(`${component} 自 ${latestTag} 以来没有归属于该组件的变更，无需发布。`)
    }
    console.log(
      `desktop 自 ${latestTag} 无自有变更，但随 TUI ${tuiVersion || '（保持）'} / Runtime ${runtimeVersion || '（保持）'} 发布更新安装包内置组件。`,
    )
  } else {
    const subjects = componentReleaseSubjects(runGit, component, latestTag, source)
    assertHasSubstantiveReleaseCommits(subjects, latestTag)
  }
}

if (component === 'desktop') await stageDesktopVersion(targetVersion)
else if (component === 'tui') await stageTuiVersion(targetVersion)
else await stageRuntimeVersion(targetVersion)

const stagedVersion = await readComponentVersion(root, component)
if (stagedVersion !== targetVersion) {
  throw new Error(`${component} release version was not synchronized.`)
}

console.log(`已在临时工作区暂存 ${tag}；尚未创建版本提交或标签。`)
