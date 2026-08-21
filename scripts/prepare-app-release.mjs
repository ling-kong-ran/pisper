// App 发版工作流的准备阶段：把目标版本写入 mobile-package.json（不提交）、
// 生成 Release 正文（自上一 App 标签以来的 App 实质性提交）、
// 并刷新 docs/latest-app.json（主页 Android/iOS 下载按钮的版本依据）。
// 用法：node scripts/prepare-app-release.mjs <app-vX.Y.Z> <source_sha>
import { execFileSync } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  APP_VERSION_FILE,
  appReleaseTag,
  appTagPattern,
  appVersionFromTag,
  isAppOwnedPath,
  normalizeRepoPath,
} from './app-paths.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const tag = String(process.argv[2] || '').trim()
const sourceRef = String(process.argv[3] || '').trim()
if (!appTagPattern().test(tag) || !sourceRef) {
  throw new Error('用法：node scripts/prepare-app-release.mjs <app-vX.Y.Z> <source_sha>')
}
const version = appVersionFromTag(tag)

const runGit = (args) =>
  execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: 'pipe' }).trim()

// 版本文件以不可变源为准（工作流检出的是 source_sha，此处再写一遍目标版本用于暂存）。
const versionPath = join(root, APP_VERSION_FILE)
await writeFile(versionPath, `${JSON.stringify({ version }, null, 2)}\n`, 'utf8')

const tags = runGit(['tag', '--list', '--sort=-version:refname']).split(/\r?\n/).filter(Boolean)
const latestTag = tags.find((value) => value !== tag && appTagPattern().test(value)) || ''
const range = latestTag ? `${latestTag}..${sourceRef}` : sourceRef
const commits = runGit(['log', '--format=%H', range]).split(/\r?\n/).filter(Boolean)

const subjects = []
for (const commit of commits) {
  const paths = runGit([
    'diff-tree',
    '--root',
    '--no-commit-id',
    '--name-only',
    '--diff-filter=ACMRTUXB',
    '-r',
    commit,
  ])
    .split(/\r?\n/)
    .map(normalizeRepoPath)
    .filter(Boolean)
  if (!paths.some(isAppOwnedPath)) continue
  const subject = runGit(['show', '-s', '--format=%s', commit])
  if (subject) subjects.push(subject)
}

const date = new Date().toISOString().slice(0, 10)
const body = [
  `## Pisper App ${tag}（${date}）`,
  '',
  'Android 安装包（APK，已签名）与 iOS 构建产物见附件。',
  latestTag ? `自 ${latestTag} 以来的变更：` : '首个 App 版本，包含：',
  '',
  ...subjects.map((subject) => `- ${subject}`),
  '',
  '> iOS 产物当前为未签名构建，需自行重签名（AltStore/Sideloadly/开发者账号）后安装。',
  '',
].join('\n')
await writeFile(join(root, 'release-body-app.md'), body, 'utf8')

const latestApp = {
  version,
  tag,
  url: `https://github.com/ling-kong-ran/pisper/releases/tag/${tag}`,
  apk: 'app-universal-release-signed.apk',
  ipa: 'pisper-ios-unsigned.ipa',
  notes: subjects.map((subject) => `- ${subject}`).join('\n'),
  releaseDate: date,
}
await mkdir(join(root, 'docs'), { recursive: true })
await writeFile(
  join(root, 'docs', 'latest-app.json'),
  `${JSON.stringify(latestApp, null, 2)}\n`,
  'utf8',
)

console.log(`已暂存 ${appReleaseTag(version)}：版本文件、Release 正文、docs/latest-app.json`)
