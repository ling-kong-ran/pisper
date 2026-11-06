import { lstat, opendir, realpath, stat } from 'node:fs/promises'
import { basename, dirname, extname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { runGit } from './git-changes-service.mjs'

const MAX_SCANNED_ENTRIES = 30_000
const GIT_QUERY_TIMEOUT_MS = 10_000
export const MAX_AUTO_ARCHIVE_BYTES = 128 * 1024 * 1024

const IGNORED_DIRECTORY_NAMES = new Set([
  '.git',
  '.hg',
  '.svn',
  '.cache',
  '.gradle',
  '.idea',
  '.next',
  '.nuxt',
  '.parcel-cache',
  '.pytest_cache',
  '.ruff_cache',
  '.svelte-kit',
  '.tox',
  '.turbo',
  '.venv',
  '.vite',
  '.vscode',
  '__pycache__',
  'bower_components',
  'build',
  'coverage',
  'dist',
  'node_modules',
  'out',
  'release',
  'target',
  'temp',
  'tmp',
  'vendor',
  'venv',
])

const IGNORED_FILE_NAMES = new Set([
  '.ds_store',
  'bun.lock',
  'bun.lockb',
  'cargo.lock',
  'composer.lock',
  'gemfile.lock',
  'package-lock.json',
  'pnpm-lock.yaml',
  'podfile.lock',
  'uv.lock',
  'yarn.lock',
])

const IGNORED_FILE_EXTENSIONS = new Set([
  '.a',
  '.class',
  '.d',
  '.dll',
  '.dylib',
  '.exe',
  '.map',
  '.o',
  '.obj',
  '.pdb',
  '.pyc',
  '.pyo',
  '.so',
  '.swp',
  '.swo',
  '.tmp',
  '.tsbuildinfo',
])

function ignoredDirectory(name) {
  const normalized = name.toLowerCase()
  return normalized.startsWith('.') || IGNORED_DIRECTORY_NAMES.has(normalized)
}

function ignoredFile(name) {
  const normalized = name.toLowerCase()
  return (
    normalized.startsWith('.') ||
    normalized.endsWith('~') ||
    IGNORED_FILE_NAMES.has(normalized) ||
    IGNORED_FILE_EXTENSIONS.has(extname(normalized))
  )
}

async function scanWorkspace(root, exclude = []) {
  const files = new Map()
  const pending = [root]
  let scannedEntries = 0

  while (pending.length) {
    const directory = pending.pop()
    if (
      exclude.some((base) => {
        const child = relative(base, directory)
        return !isAbsolute(child) && child !== '..' && !child.startsWith(`..${sep}`)
      })
    )
      continue
    let handle
    try {
      handle = await opendir(directory)
    } catch {
      // 不完整快照不能用于认定文件是本次工具新生成的。
      return null
    }
    for await (const entry of handle) {
      scannedEntries += 1
      if (scannedEntries > MAX_SCANNED_ENTRIES) return null
      if (entry.isSymbolicLink()) continue
      const path = resolve(directory, entry.name)
      if (entry.isDirectory()) {
        if (!ignoredDirectory(entry.name)) pending.push(path)
        continue
      }
      if (!entry.isFile() || ignoredFile(entry.name)) continue
      const key = relative(root, path)
      if (!key || key.startsWith(`..${sep}`) || key === '..') continue
      const info = await workspaceAssetFile(path)
      if (!info) continue
      files.set(key, info)
    }
  }

  return files
}

// 新目录尚不存在时也要解析已存在的父目录链接，否则别名路径会绕过隔离。
export async function resolveWorkspaceAssetPath(path) {
  let parent = resolve(path)
  const suffix = []
  while (true) {
    const actual = await realpath(parent).catch(() => null)
    if (actual) return resolve(actual, ...suffix)
    const next = dirname(parent)
    if (next === parent) return resolve(path)
    suffix.unshift(basename(parent))
    parent = next
  }
}

export async function workspaceAssetFile(path) {
  const info = await lstat(path).catch(() => null)
  if (!info?.isFile() || info.size > MAX_AUTO_ARCHIVE_BYTES) return null
  return {
    path,
    size: info.size,
    modified: info.mtimeMs,
    version: `${info.dev}:${info.ino}:${info.size}:${info.mtimeMs}:${info.ctimeMs}`,
  }
}

const queryGit = (repository, ...args) =>
  runGit(repository, ['--no-optional-locks', '-c', 'core.fsmonitor=false', ...args], {
    timeout: GIT_QUERY_TIMEOUT_MS,
  })

const nulSeparated = (result) => new Set(result.ok ? result.stdout.split('\0').filter(Boolean) : [])

// 工作区可能只是仓库的子目录，也可能含嵌套仓库（clone、子模块），因此按文件逐级向上找。
async function gitRepositoryRoot(path, cache) {
  const directory = dirname(path)
  if (cache.has(directory)) return cache.get(directory)
  const parent = dirname(directory)
  let repository = null
  if (await lstat(join(directory, '.git')).catch(() => null)) repository = directory
  else if (parent !== directory) repository = await gitRepositoryRoot(directory, cache)
  cache.set(directory, repository)
  return repository
}

const gitHead = async (repository) => {
  const result = await queryGit(repository, 'rev-parse', '-q', '--verify', 'HEAD')
  return result.ok ? result.stdout.trim() : ''
}

// HEAD 自基线以来只因本地 commit 前进：提交里的新文件是这条命令自己写的，不是拉下来的。
// 没有基线（新出现的嵌套仓库）时看全部记录，clone 留下的记录会让它不成立。
async function onlyLocalCommitsSince(repository, baselineHead) {
  const reflog = await queryGit(repository, 'log', '-g', '-n', '500', '--format=%H%x00%gs', 'HEAD')
  if (!reflog.ok) return false
  let entries = 0
  for (const line of reflog.stdout.split('\n').filter(Boolean)) {
    const [hash, subject = ''] = line.split('\0')
    if (baselineHead && hash === baselineHead) return entries > 0
    if (!subject.startsWith('commit')) return false
    entries += 1
  }
  return !baselineHead && entries > 0
}

// 合并、变基、拣选停在半路时，带进来的新文件已暂存但不在 HEAD 里，要和 Agent 自己 git add 的区分开。
async function gitIntegrationInProgress(repository) {
  const names = ['MERGE_HEAD', 'CHERRY_PICK_HEAD', 'REVERT_HEAD', 'REBASE_HEAD', 'SQUASH_MSG']
  const paths = await queryGit(
    repository,
    'rev-parse',
    ...names.flatMap((name) => ['--git-path', name]),
  )
  if (!paths.ok) return false
  const found = await Promise.all(
    paths.stdout
      .split('\n')
      .filter(Boolean)
      .map((path) => lstat(resolve(repository, path)).catch(() => null)),
  )
  return found.some(Boolean)
}

// 查询失败（如没装 git）时都算不是：宁可多显示，也不漏掉 Agent 新建的文件。
async function gitArrivalCheck(repository, baselineHead) {
  const [tracked, committed, head] = await Promise.all([
    queryGit(repository, 'ls-files', '-z').then(nulSeparated),
    queryGit(repository, 'ls-tree', '-r', '-z', '--name-only', 'HEAD').then(nulSeparated),
    gitHead(repository),
  ])
  const integrating = await gitIntegrationInProgress(repository)
  const localCommits =
    Boolean(head) &&
    head !== baselineHead &&
    (await onlyLocalCommitsSince(repository, baselineHead))
  return (path) => tracked.has(path) && (committed.has(path) ? !localCommits : integrating)
}

// pull、切分支、clone 带来的新文件是更新代码，不算 Agent 新建；它自己写完再 git add 或提交的仍然算。
async function withGitArrivalsNotAdded(files, baseline) {
  const roots = new Map()
  const checks = new Map()
  for (const file of files) {
    if (!file.added) continue
    const repository = await gitRepositoryRoot(file.path, roots)
    if (!repository) continue
    if (!checks.has(repository)) {
      const baselineHead = repository === baseline.repository ? baseline.head : ''
      checks.set(repository, await gitArrivalCheck(repository, baselineHead))
    }
    file.added = !checks.get(repository)(relative(repository, file.path).split(sep).join('/'))
  }
  return files
}

// 升级前的旧资产没有隐藏标记，也无从得知当时是否新建。能确认的只有两类：
// 已被 git 跟踪的文件（更新代码或改已有文件留下的）和没有文件路径的工具预览图。
// 查询失败或不在仓库里的文件保持原样，返回是否有改动。
export async function hideLegacyReplyAssets(assets) {
  const roots = new Map()
  const tracked = new Map()
  let changed = false
  for (const asset of assets) {
    if (!asset.filePath) {
      for (const item of [asset, ...(asset.references || [])]) {
        if (item.source !== 'agent' || item.hiddenInReply) continue
        item.hiddenInReply = true
        changed = true
      }
      continue
    }
    if (asset.source !== 'agent' || asset.hiddenInReply) continue
    const path = resolve(asset.filePath)
    const repository = await gitRepositoryRoot(path, roots)
    if (!repository) continue
    if (!tracked.has(repository))
      tracked.set(repository, nulSeparated(await queryGit(repository, 'ls-files', '-z')))
    if (!tracked.get(repository).has(relative(repository, path).split(sep).join('/'))) continue
    asset.hiddenInReply = true
    changed = true
  }
  return changed
}

// 基线不完整时返回 null，防止把无法确认的新旧文件整批误收进资产页。
export async function captureWorkspaceAssetBaseline(cwd, { exclude = [] } = {}) {
  // 根路径与排除目录统一为真实路径，防止经 symlink/junction 把运行时数据当产物。
  const root = await realpath(resolve(String(cwd || ''))).catch(() => null)
  if (!root) return null
  exclude = await Promise.all(exclude.map(resolveWorkspaceAssetPath))
  const info = await stat(root).catch(() => null)
  if (!info?.isDirectory()) return null
  const files = await scanWorkspace(root, exclude)
  if (!files) return null
  // 只记工作区所在仓库的 HEAD：判断这条命令是本地提交还是更新代码时要用。
  const repository = await gitRepositoryRoot(join(root, 'file'), new Map())
  return {
    root,
    files: new Set(files.keys()),
    entries: files,
    exclude,
    repository,
    head: repository ? await gitHead(repository) : '',
  }
}

export async function listNewWorkspaceAssets(baseline) {
  if (!baseline?.root || !(baseline.files instanceof Set)) return []
  const current = await scanWorkspace(baseline.root, baseline.exclude)
  if (!current) return []
  return [...current.entries()]
    .filter(([key]) => !baseline.files.has(key))
    .map(([, value]) => value)
    .sort(
      (left, right) =>
        left.modified - right.modified || basename(left.path).localeCompare(basename(right.path)),
    )
}

export async function listChangedWorkspaceAssets(baseline) {
  if (!baseline?.entries) return []
  const current = await scanWorkspace(baseline.root, baseline.exclude)
  if (!current) return []
  return withGitArrivalsNotAdded(
    [...current.entries()]
      .filter(([key, file]) => baseline.entries.get(key)?.version !== file.version)
      .map(([key, file]) => ({ ...file, added: !baseline.entries.has(key) })),
    baseline,
  )
}
