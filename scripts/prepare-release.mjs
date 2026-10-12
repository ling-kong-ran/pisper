import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { mkdir, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { componentReleaseSubjects } from './release-changes.mjs'
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
const generated = generateComponentNotes({ component, tag, previousTag, sourceRef })
const date = new Date().toISOString().slice(0, 10)
const rustDesktop =
  component === 'desktop' &&
  existsSync(join(root, 'runtime-rs/Cargo.toml')) &&
  !existsSync(join(root, 'runtime/index.mjs'))
const upgradeNotes = rustDesktop ? desktopRustUpgradeNotes(tag) : ''
const markdown = normalizeMarkdown(`${generated.body}${upgradeNotes}`, component, version)

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

function generateComponentNotes({
  component: selectedComponent,
  tag: currentTag,
  previousTag: previous,
  sourceRef: source,
}) {
  const subjects = componentReleaseSubjects(
    (args) => run('git', args),
    selectedComponent,
    previous,
    source,
  ).filter((value) => !/^chore\(release(?:-[^)]+)?\):/i.test(value))
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

function desktopRustUpgradeNotes(currentTag) {
  return [
    '',
    '',
    '### 从 0.5.x 升级到 Rust 版本',
    '',
    `请从 [${currentTag} 完整安装包](https://github.com/${repository}/releases/tag/${currentTag}) 下载对应系统和架构的 Windows 安装器、macOS DMG 或 Linux AppImage/DEB。退出 Pisper 后覆盖原应用，重新启动。`,
    '',
    '0.5.80/0.5.81 的 Desktop 组件只替换前端，不能替换原生壳并完成这次 Rust 升级。旧版应用内更新将展示“尚未完成升级”的下载提示；请点击提示中的完整安装包入口进行覆盖安装。',
    '',
    '原有会话、模型、MCP 和 Skill 继续读取 ~/.pisper/agent；如使用 PISPER_AGENT_DIR，请保留原设置。升级前备份该目录，不要删除它，也不需要先卸载旧版。',
    '',
    '完整安装后打开设置确认应用版本，检查原会话与模型配置。后续 Rust 组件更新使用独立资产，旧 Node 组件不会覆盖新 Runtime。',
    '',
    '新 Rust Runtime 启动并通过健康检查后，只清理受管理组件目录中的旧 Node Runtime 和升级提示缓存，不删除 Agent 数据、项目工作区或用户下载文件。',
  ].join('\n')
}
