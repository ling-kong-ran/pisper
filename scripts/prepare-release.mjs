import { execFileSync } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { hasMeaningfulGeneratedNotes } from '../shared/release-notes.mjs'
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
const token = String(process.env.GITHUB_TOKEN || '').trim()

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
let source = '本地 Git 提交'
let generated = generateLocalNotes({ tag, previousTag, sourceRef })
if (token) {
  const githubNotes = await generateGitHubNotes({
    repository,
    token,
    tag,
    previousTag,
    sourceRef,
  })
  if (hasMeaningfulGeneratedNotes(githubNotes.body)) {
    generated = githubNotes
    source = 'GitHub 自动 Release Notes'
  } else {
    source = '本地 Git 提交（GitHub 自动日志无有效条目）'
  }
}
const date = new Date().toISOString().slice(0, 10)
const markdown = normalizeMarkdown(generated.body, component, version)

if (component !== 'tui') {
  await mkdir(dirname(releaseNotesPath), { recursive: true })
  await writeFile(
    releaseNotesPath,
    `${JSON.stringify({ component, version, date, notes: markdown }, null, 2)}\n`,
    'utf8',
  )
}
await writeFile(releaseBodyPath, `${markdown}\n`, 'utf8')
console.log(`已从 ${source}生成 Pisper ${component} ${version} 更新日志。`)

async function generateGitHubNotes({
  repository: repo,
  token: githubToken,
  tag: currentTag,
  previousTag: previous,
  sourceRef: targetCommitish,
}) {
  const payload = {
    tag_name: currentTag,
    target_commitish: targetCommitish,
    configuration_file_path: '.github/release.yml',
  }
  if (previous) payload.previous_tag_name = previous

  const response = await fetch(`https://api.github.com/repos/${repo}/releases/generate-notes`, {
    method: 'POST',
    headers: {
      Accept: 'application/vnd.github+json',
      Authorization: `Bearer ${githubToken}`,
      'X-GitHub-Api-Version': '2022-11-28',
      'User-Agent': 'pisper-release',
    },
    body: JSON.stringify(payload),
  })
  if (!response.ok) {
    const detail = await response.text()
    throw new Error(`GitHub Release Notes 生成失败：HTTP ${response.status} ${detail}`)
  }
  return response.json()
}

function generateLocalNotes({ tag: currentTag, previousTag: previous, sourceRef: source }) {
  const range = previous ? `${previous}..${source}` : source
  const subjects = run('git', ['log', range, '--pretty=format:%s'])
    .split(/\r?\n/)
    .map((value) => value.trim())
    .filter((value) => value && !/^chore\(release(?:-[^)]+)?\):/i.test(value))
  const notes = subjects.length
    ? subjects.map((subject) => `- ${subject}`).join('\n')
    : '- Maintenance release'
  const compare = previous
    ? `\n\n**完整变更**：https://github.com/${repository}/compare/${previous}...${currentTag}`
    : ''
  return { body: `## What's Changed\n\n${notes}${compare}` }
}

function normalizeMarkdown(body, selectedComponent, currentVersion) {
  const content = String(body || '').trim() || '- Maintenance release'
  const label = selectedComponent === 'desktop' ? 'Desktop' : selectedComponent.toUpperCase()
  return `## Pisper ${label} ${currentVersion}\n\n${content}`
}
