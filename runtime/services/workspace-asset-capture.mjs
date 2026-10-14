import { lstat, opendir, realpath, stat } from 'node:fs/promises'
import { basename, dirname, extname, isAbsolute, relative, resolve, sep } from 'node:path'

const MAX_SCANNED_ENTRIES = 30_000
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

// 基线不完整时返回 null，防止把无法确认的新旧文件整批误收进资产页。
export async function captureWorkspaceAssetBaseline(cwd, { exclude = [] } = {}) {
  // 根路径与排除目录统一为真实路径，防止经 symlink/junction 把运行时数据当产物。
  const root = await realpath(resolve(String(cwd || ''))).catch(() => null)
  if (!root) return null
  exclude = await Promise.all(exclude.map(resolveWorkspaceAssetPath))
  const info = await stat(root).catch(() => null)
  if (!info?.isDirectory()) return null
  const files = await scanWorkspace(root, exclude)
  return files ? { root, files: new Set(files.keys()), entries: files, exclude } : null
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
  return [...current.entries()]
    .filter(([key, file]) => baseline.entries.get(key)?.version !== file.version)
    .map(([, file]) => file)
}
