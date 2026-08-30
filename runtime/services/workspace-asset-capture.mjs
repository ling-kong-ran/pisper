import { opendir, stat } from 'node:fs/promises'
import { basename, extname, relative, resolve, sep } from 'node:path'

const MAX_SCANNED_ENTRIES = 30_000
const MAX_NEW_ASSETS_PER_RUN = 200
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

async function scanWorkspace(root, { includeMetadata = false } = {}) {
  const files = new Map()
  const pending = [root]
  let scannedEntries = 0

  while (pending.length) {
    const directory = pending.pop()
    let handle
    try {
      handle = await opendir(directory)
    } catch {
      continue
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
      if (!includeMetadata) {
        files.set(key, null)
        continue
      }
      const info = await stat(path).catch(() => null)
      if (!info?.isFile() || info.size <= 0 || info.size > MAX_AUTO_ARCHIVE_BYTES) continue
      files.set(key, { path, size: info.size, modified: info.mtimeMs })
    }
  }

  return files
}

// 基线不完整时返回 null，防止把无法确认的新旧文件整批误收进资产页。
export async function captureWorkspaceAssetBaseline(cwd) {
  const root = resolve(String(cwd || ''))
  const info = await stat(root).catch(() => null)
  if (!info?.isDirectory()) return null
  const files = await scanWorkspace(root)
  return files ? { root, files: new Set(files.keys()) } : null
}

export async function listNewWorkspaceAssets(baseline) {
  if (!baseline?.root || !(baseline.files instanceof Set)) return []
  const current = await scanWorkspace(baseline.root, { includeMetadata: true })
  if (!current) return []
  return [...current.entries()]
    .filter(([key]) => !baseline.files.has(key))
    .map(([, value]) => value)
    .sort(
      (left, right) =>
        left.modified - right.modified || basename(left.path).localeCompare(basename(right.path)),
    )
    .slice(0, MAX_NEW_ASSETS_PER_RUN)
}
