import { execFileSync } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { componentReleaseSubjects } from './release-changes.mjs'
import { formatReleaseNotes, releaseNotesDate } from './release-notes-format.mjs'
import {
  assertReleaseComponent,
  fallbackReleaseTag,
  readComponentVersion,
  releaseTag,
} from './release-components.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const releaseNotesPath = join(root, 'public', 'release-notes.json')
const releaseBodyPath = join(root, 'release-body.md')
const tag = String(process.argv[2] || process.env.GITHUB_REF_NAME || '').trim()
const sourceRef = String(process.argv[3] || tag).trim()
const component = assertReleaseComponent(process.argv[4] || 'desktop')
const version = await readComponentVersion(root, component)
if (tag !== releaseTag(component, version)) {
  throw new Error(
    `版本不一致：${component} 为 ${version}，Git Tag 为 ${tag}。请使用 npm run release 创建版本。`,
  )
}

const repository = String(process.env.GITHUB_REPOSITORY || 'ling-kong-ran/pisper')

function run(command, args) {
  return execFileSync(command, args, { cwd: root, encoding: 'utf8', stdio: 'pipe' }).trim()
}

function readComponentVersionAtRef(ref) {
  const path =
    component === 'desktop'
      ? 'src-tauri/desktop-package.json'
      : component === 'tui'
        ? 'src-tui/Cargo.toml'
        : 'package.json'
  const source = run('git', ['show', `${ref}:${path}`])
  if (component !== 'tui') return JSON.parse(source).version
  return source.match(/\[package\][\s\S]*?\r?\nversion\s*=\s*"([^"]+)"/)?.[1] || ''
}

const tags = run('git', ['tag', '--list', '--sort=-version:refname']).split(/\r?\n/).filter(Boolean)
const baselineVersion = readComponentVersionAtRef(sourceRef)
const previousTag = fallbackReleaseTag(
  component,
  tags.filter((value) => value !== tag),
  baselineVersion,
)
const date = releaseNotesDate()
const markdown = formatReleaseNotes({
  component,
  version,
  tag,
  previousTag,
  sourceSha: run('git', ['rev-parse', '--verify', `${sourceRef}^{commit}`]),
  repository,
  date,
  subjects: componentReleaseSubjects((args) => run('git', args), component, previousTag, sourceRef),
})

if (component !== 'tui') {
  await mkdir(dirname(releaseNotesPath), { recursive: true })
  await writeFile(
    releaseNotesPath,
    `${JSON.stringify({ component, version, date, notes: markdown }, null, 2)}\n`,
    'utf8',
  )
}
await writeFile(releaseBodyPath, `${markdown}\n`, 'utf8')
console.log(`已从组件相关的 Git 提交生成 Pisper ${component} ${version} 更新日志。`)
