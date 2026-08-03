import { execFileSync } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { hasMeaningfulGeneratedNotes } from '../shared/release-notes.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const packagePath = join(root, 'package.json')
const releaseNotesPath = join(root, 'public', 'release-notes.json')
const releaseBodyPath = join(root, 'release-body.md')
const tag = String(process.argv[2] || process.env.GITHUB_REF_NAME || '').trim()
const match = tag.match(/^v(\d+\.\d+\.\d+)$/)
if (!match) throw new Error(`发布标签无效：${tag}。标签必须使用 vX.Y.Z 格式。`)

const version = match[1]
const repository = String(process.env.GITHUB_REPOSITORY || 'ling-kong-ran/pisper')
const token = String(process.env.GITHUB_TOKEN || '').trim()

function run(command, args) {
  return execFileSync(command, args, { cwd: root, encoding: 'utf8', stdio: 'pipe' }).trim()
}

const packageJson = JSON.parse(await readFile(packagePath, 'utf8'))
if (packageJson.version !== version) {
  throw new Error(
    `版本不一致：package.json 为 ${packageJson.version}，Git Tag 为 ${tag}。请使用 npm run release 创建版本。`,
  )
}

const tags = run('git', ['tag', '--list', 'v*', '--sort=-version:refname'])
  .split(/\r?\n/)
  .filter((value) => /^v\d+\.\d+\.\d+$/.test(value))
const previousTag = tags.find((value) => value !== tag)
let source = '本地 Git 提交'
let generated = generateLocalNotes({ tag, previousTag })
if (token) {
  const githubNotes = await generateGitHubNotes({ repository, token, tag, previousTag })
  if (hasMeaningfulGeneratedNotes(githubNotes.body)) {
    generated = githubNotes
    source = 'GitHub 自动 Release Notes'
  } else {
    source = '本地 Git 提交（GitHub 自动日志无有效条目）'
  }
}
const date = new Date().toISOString().slice(0, 10)
const markdown = normalizeMarkdown(generated.body, version)

await mkdir(dirname(releaseNotesPath), { recursive: true })
await writeFile(
  releaseNotesPath,
  `${JSON.stringify({ version, date, notes: markdown }, null, 2)}\n`,
  'utf8',
)
await writeFile(releaseBodyPath, `${markdown}\n`, 'utf8')
console.log(`已从 ${source}生成 Pisper ${version} 更新日志。`)

async function generateGitHubNotes({
  repository: repo,
  token: githubToken,
  tag: currentTag,
  previousTag: previous,
}) {
  const payload = {
    tag_name: currentTag,
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

function generateLocalNotes({ tag: currentTag, previousTag: previous }) {
  const range = previous ? `${previous}..${currentTag}` : currentTag
  const subjects = run('git', ['log', range, '--pretty=format:%s'])
    .split(/\r?\n/)
    .map((value) => value.trim())
    .filter((value) => value && !/^chore\(release\):/i.test(value))
  const notes = subjects.length
    ? subjects.map((subject) => `- ${subject}`).join('\n')
    : '- Maintenance release'
  const compare = previous
    ? `\n\n**完整变更**：https://github.com/${repository}/compare/${previous}...${currentTag}`
    : ''
  return { body: `## What's Changed\n\n${notes}${compare}` }
}

function normalizeMarkdown(body, currentVersion) {
  const content = String(body || '').trim() || '- Maintenance release'
  return `## Pisper ${currentVersion}\n\n${content}`
}
