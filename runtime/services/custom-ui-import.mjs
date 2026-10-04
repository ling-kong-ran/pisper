// 自定义组件 ZIP 导入：只接受一个 manifest 所在目录的普通静态文件，校验后原子安装。
import { lstat, mkdir, mkdtemp, rename, rm, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { decodeBoundedZip, BoundedZipError } from '../storage/bounded-zip.mjs'

const ID = /^[a-z0-9][a-z0-9._-]{0,63}$/
const ASSET = /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/
const MAX_ZIP_BYTES = 16 * 1024 * 1024
const MAX_FILE_BYTES = 8 * 1024 * 1024
const MAX_UNPACKED_BYTES = 16 * 1024 * 1024
const MAX_ENTRIES = 128
// 导入与目录扫描必须使用同一限制，避免已报告成功的组件在重启后消失。
export const CUSTOM_UI_MANIFEST_MAX_BYTES = 64 * 1024

export class CustomUiImportError extends Error {
  constructor(code, statusCode = 400) {
    super(code)
    this.code = code
    this.statusCode = statusCode
  }
}

function safePath(name) {
  if (typeof name !== 'string' || name.length > 512 || name.includes('\\'))
    throw new CustomUiImportError('component_archive_invalid')
  const path = name.endsWith('/') ? name.slice(0, -1) : name
  const segments = path.split('/')
  if (
    segments.length > 8 ||
    segments.some(
      (segment) => !segment || segment === '.' || segment === '..' || !ASSET.test(segment),
    )
  )
    throw new CustomUiImportError('component_archive_invalid')
  return segments.join('/')
}

export function unpackCustomUiZip(input) {
  const bytes = input instanceof Uint8Array ? input : new Uint8Array(input)
  if (!bytes.length || bytes.length > MAX_ZIP_BYTES)
    throw new CustomUiImportError('component_archive_invalid')
  let decoded
  try {
    decoded = decodeBoundedZip(bytes, {
      maxArchiveBytes: MAX_ZIP_BYTES,
      maxTotalBytes: MAX_UNPACKED_BYTES,
      maxFileBytes: MAX_FILE_BYTES,
      maxFiles: MAX_ENTRIES,
      maxPathLength: 512,
      allowDirectories: true,
    })
  } catch (error) {
    throw new CustomUiImportError(
      error instanceof BoundedZipError && error.code === 'zip_archive_too_large'
        ? 'component_archive_too_large'
        : 'component_archive_invalid',
    )
  }
  const files = new Map(
    Object.entries(decoded).map(([name, content]) => [safePath(name), Buffer.from(content)]),
  )
  const manifests = [...files.keys()].filter((path) => path.endsWith('/manifest.json'))
  if (manifests.length !== 1) throw new CustomUiImportError('component_manifest_missing')
  const prefix = manifests[0].slice(0, -'manifest.json'.length)
  const id = prefix.slice(0, -1).split('/').at(-1)
  if (!ID.test(id)) throw new CustomUiImportError('component_archive_invalid')
  const assets = new Map()
  for (const [path, content] of files) {
    if (!path.startsWith(prefix)) continue
    const relative = safePath(path.slice(prefix.length))
    assets.set(relative, content)
  }
  if (assets.size > 64) throw new CustomUiImportError('component_archive_invalid')
  const paths = new Map()
  for (const name of assets.keys()) {
    const segments = name.split('/')
    for (let index = 0; index < segments.length; index++) {
      const path = segments.slice(0, index + 1).join('/')
      const kind = index === segments.length - 1 ? 'file' : 'directory'
      const previous = paths.get(path.toLowerCase())
      // 大小写不敏感文件系统中不能合并不同拼写的目录，也不能把文件当成目录。
      if (previous && (previous.path !== path || previous.kind !== kind))
        throw new CustomUiImportError('component_archive_invalid')
      paths.set(path.toLowerCase(), { path, kind })
    }
  }
  return { id, assets }
}

export async function importCustomUiZip({ root, bytes, normalizeManifest, reservedIds = [] }) {
  const { id, assets } = unpackCustomUiZip(bytes)
  if (reservedIds.includes(id)) throw new CustomUiImportError('component_id_reserved', 409)
  const manifestBytes = assets.get('manifest.json')
  if (manifestBytes.byteLength > CUSTOM_UI_MANIFEST_MAX_BYTES)
    throw new CustomUiImportError('component_manifest_invalid')
  let manifest
  try {
    manifest = normalizeManifest(id, JSON.parse(manifestBytes.toString('utf8')))
  } catch {
    throw new CustomUiImportError('component_manifest_invalid')
  }
  if (!assets.has(manifest.entry)) throw new CustomUiImportError('component_entry_missing')
  await mkdir(root, { recursive: true })
  const target = join(root, id)
  try {
    await lstat(target)
    throw new CustomUiImportError('component_already_installed', 409)
  } catch (error) {
    if (error instanceof CustomUiImportError) throw error
    if (error.code !== 'ENOENT') throw error
  }
  const stage = await mkdtemp(join(root, '.component-import-'))
  try {
    for (const [name, content] of assets) {
      const file = join(stage, name)
      await mkdir(dirname(file), { recursive: true, mode: 0o700 })
      await writeFile(file, content, { flag: 'wx', mode: 0o600 })
    }
    try {
      await rename(stage, target)
    } catch (error) {
      // 两个导入可能同时通过存在性检查；已完成的非空组件目录仍不能被覆盖。
      if (error.code === 'EEXIST' || error.code === 'ENOTEMPTY')
        throw new CustomUiImportError('component_already_installed', 409)
      throw error
    }
    return { id, name: manifest.name, version: manifest.version }
  } finally {
    await rm(stage, { recursive: true, force: true })
  }
}
