import { execFile } from 'node:child_process'
import { createHash } from 'node:crypto'
import { chmod, copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { basename, join, win32 } from 'node:path'
import { promisify } from 'node:util'
import resources from './search-tool-resources.json' with { type: 'json' }

const run = promisify(execFile)
const MAX_ARCHIVE_BYTES = 20 * 1024 * 1024
const TOOL_ROOT = 'node_modules/@earendil-works/pi-coding-agent/vendor/bin'
// 下载源偶尔回 5xx 或断连（发布 CI 遇到过 504）：服务端错误与网络错误重试，4xx 与摘要不符直接失败。
const DOWNLOAD_ATTEMPTS = 3
const RETRY_DELAY_MS = 5_000

class TransientDownloadError extends Error {}

export function searchArchiveCommand(platform = process.platform, env = process.env) {
  // Git/MSYS 的 GNU tar 会把盘符当远端地址且不能解 ZIP；Windows 必须使用系统 bsdtar。
  return platform === 'win32'
    ? win32.join(env.SystemRoot || env.WINDIR || 'C:\\Windows', 'System32', 'tar.exe')
    : 'tar'
}

export function searchToolEntries(target) {
  if (target.platform === 'mobile') return []
  const key = `${target.platform}-${target.arch}`
  return Object.entries(resources).map(([name, resource]) => {
    const asset = resource.assets[key]
    if (!asset) throw new Error(`Unsupported search tool target: ${key}`)
    return {
      name,
      ...resource,
      asset,
      filename: name + (target.platform === 'win32' ? '.exe' : ''),
    }
  })
}

export function searchToolCriticalEntries(target) {
  return searchToolEntries(target).flatMap(({ name, filename, licenseFiles }) => [
    { kind: 'search', path: `${TOOL_ROOT}/${filename}` },
    { kind: 'license', path: `${TOOL_ROOT}/${name}-provenance.json` },
    ...licenseFiles.map((file) => ({ kind: 'license', path: `${TOOL_ROOT}/${name}-${file}` })),
  ])
}

function checksum(bytes) {
  return createHash('sha256').update(bytes).digest('hex')
}

async function downloadArchive(asset, fetchImpl) {
  let response
  try {
    response = await fetchImpl(asset.url, { signal: AbortSignal.timeout(120_000) })
  } catch (error) {
    throw new TransientDownloadError(`Search tool download: ${error?.message || error}`)
  }
  if (response.status >= 500)
    throw new TransientDownloadError(`Search tool download: HTTP ${response.status}`)
  if (!response.ok || !response.body)
    throw new Error(`Search tool download: HTTP ${response.status}`)
  const chunks = []
  let size = 0
  for await (const chunk of response.body) {
    size += chunk.length
    if (size > MAX_ARCHIVE_BYTES) throw new Error('Search tool archive exceeds size limit')
    chunks.push(chunk)
  }
  const bytes = Buffer.concat(chunks)
  if (checksum(bytes) !== asset.sha256) throw new Error('Search tool archive checksum mismatch')
  return bytes
}

async function archiveBytes(asset, cachePath, fetchImpl, retryDelayMs) {
  try {
    const bytes = await readFile(cachePath)
    if (checksum(bytes) === asset.sha256) return bytes
  } catch (error) {
    if (error.code !== 'ENOENT') throw error
  }
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await downloadArchive(asset, fetchImpl)
    } catch (error) {
      if (!(error instanceof TransientDownloadError) || attempt >= DOWNLOAD_ATTEMPTS) throw error
      await new Promise((resolve) => setTimeout(resolve, retryDelayMs * attempt))
    }
  }
}

// 下载仅发生在构建机。交付闭包使用固定版本与摘要，不依赖用户缓存、PATH 或公网。
export async function stageSearchTools({
  root,
  runtimeDir,
  target,
  fetchImpl = globalThis.fetch,
  retryDelayMs = RETRY_DELAY_MS,
}) {
  const entries = searchToolEntries(target)
  if (!entries.length) return
  const cacheDir = join(root, 'release', 'cache', 'search-tools')
  const destination = join(runtimeDir, TOOL_ROOT)
  await mkdir(cacheDir, { recursive: true })
  await mkdir(destination, { recursive: true })
  for (const { name, version, source, license, licenseFiles, asset, filename } of entries) {
    const cachePath = join(cacheDir, basename(new URL(asset.url).pathname))
    const bytes = await archiveBytes(asset, cachePath, fetchImpl, retryDelayMs)
    const temporary = await mkdtemp(join(cacheDir, 'extract-'))
    try {
      // 使用独占临时副本解包；并行构建不能读取尚未写完的共享缓存。
      const archive = join(temporary, 'archive' + (asset.url.endsWith('.zip') ? '.zip' : '.tar.gz'))
      await writeFile(archive, bytes)
      await run(searchArchiveCommand(), ['-xf', basename(archive)], {
        cwd: temporary,
        timeout: 30_000,
        windowsHide: true,
      })
      const extracted = join(temporary, asset.archiveRoot)
      await copyFile(join(extracted, filename), join(destination, filename))
      if (target.platform !== 'win32') await chmod(join(destination, filename), 0o755)
      for (const file of licenseFiles) {
        await copyFile(join(extracted, file), join(destination, `${name}-${file}`))
      }
      await writeFile(
        join(destination, `${name}-provenance.json`),
        JSON.stringify({ version, source, license, ...asset }, null, 2) + '\n',
      )
      await writeFile(cachePath, bytes)
    } finally {
      await rm(temporary, { recursive: true, force: true })
    }
  }
}
