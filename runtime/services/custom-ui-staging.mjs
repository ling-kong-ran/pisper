// 导入暂存（规格 5.7）：他人分享的 ZIP 先解包到 <dataDir>/custom-ui-staging/<stageId>/component/，只暂存、不执行，
// 等用户在信任确认里看过文件、源码、能力与静态扫描后再安装。暂存目录在被监听、被服务的 custom-ui/ 之外，
// 模块路由永远不会服务它；源码预览只走 readStagedFile 的文本读取。
//
// 校验沿用 unpackCustomUiZip（路径、条目数、体积、大小写冲突）；另外拒绝旧 iframe 格式、manifest 无效、
// 入口缺失与 HTML 文件。暂存超过 7 天的在启动时清理，最多同时保留 20 份。
import { createHash, randomBytes } from 'node:crypto'
import { lstat, mkdir, readdir, rm, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import {
  RESERVED_COMPONENT_IDS,
  classifyManifest,
  isSafeComponentPath,
  normalizeNativeManifest,
  toWellFormed,
} from '../../shared/custom-ui-manifest.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { CustomUiError } from './custom-ui-error.mjs'
import { componentFileType } from './custom-ui-files.mjs'
import {
  CUSTOM_UI_MANIFEST_MAX_BYTES,
  CustomUiImportError,
  unpackCustomUiZip,
} from './custom-ui-import.mjs'
import { readComponentTree } from './custom-ui-versions.mjs'

const STAGE_ID_PATTERN = /^[a-f0-9]{16}$/
const MAX_STAGED = 20
const STAGE_RETENTION_MS = 7 * 24 * 60 * 60 * 1000
const MAX_FILE_NAME = 200
const MAX_PREVIEW_BYTES = 512 * 1024
const PREVIEW_FILE = /\.(?:m?js|css|json|svg|md|txt)$/i

/**
 * @typedef {{
 *   version: 1,
 *   stageId: string,
 *   id: string,
 *   name: string,
 *   componentVersion: string,
 *   fileName: string,
 *   bytes: number,
 *   sha256: string,
 *   createdAt: string,
 * }} StagedMeta
 */

/** @param {unknown} value */
export function isStageId(value) {
  return typeof value === 'string' && STAGE_ID_PATTERN.test(value)
}

/** @param {unknown} value */
function cleanFileName(value) {
  if (typeof value !== 'string') return ''
  // 文件名只用于显示：去掉路径部分与控制字符。
  const base = value.split(/[/\\]/).pop() || ''
  return toWellFormed(base.replace(/\p{Cc}+/gu, '').trim()).slice(0, MAX_FILE_NAME)
}

/** @param {unknown} raw @returns {StagedMeta | null} */
function parseMeta(raw) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const meta = /** @type {Record<string, unknown>} */ (raw)
  if (
    meta.version !== 1 ||
    !isStageId(meta.stageId) ||
    typeof meta.id !== 'string' ||
    typeof meta.sha256 !== 'string' ||
    !Number.isSafeInteger(meta.bytes) ||
    typeof meta.createdAt !== 'string'
  )
    return null
  return {
    version: 1,
    stageId: /** @type {string} */ (meta.stageId),
    id: meta.id,
    name: typeof meta.name === 'string' ? meta.name.slice(0, 120) : meta.id,
    componentVersion:
      typeof meta.componentVersion === 'string' ? meta.componentVersion.slice(0, 64) : '',
    fileName: cleanFileName(meta.fileName),
    bytes: Number(meta.bytes),
    sha256: meta.sha256.slice(0, 64),
    createdAt: meta.createdAt,
  }
}

export class CustomUiStaging {
  /** @param {{ root: string, now?: () => number, hostVersion?: () => string | null }} options */
  constructor({ root, now = Date.now, hostVersion = () => null }) {
    this.root = root
    this.now = now
    this.hostVersion = hostVersion
  }

  /** @param {string} stageId */
  componentDir(stageId) {
    return join(this.root, stageId, 'component')
  }

  /**
   * 解包、校验并暂存；不写 custom-ui/，不发 component_changed。
   * @param {Uint8Array} bytes
   * @param {{ fileName?: unknown }} [options]
   * @returns {Promise<StagedMeta>}
   */
  async stage(bytes, { fileName } = {}) {
    let unpacked
    try {
      unpacked = unpackCustomUiZip(bytes)
    } catch (error) {
      if (error instanceof CustomUiImportError)
        throw new CustomUiError(error.code, error.statusCode)
      throw error
    }
    const { id, assets } = unpacked
    if (RESERVED_COMPONENT_IDS.includes(id)) throw new CustomUiError('component_id_reserved', 409)
    const manifestBytes = assets.get('manifest.json')
    if (!manifestBytes || manifestBytes.byteLength > CUSTOM_UI_MANIFEST_MAX_BYTES)
      throw new CustomUiError('component_manifest_invalid', 400)
    let raw
    try {
      raw = JSON.parse(manifestBytes.toString('utf8'))
    } catch {
      throw new CustomUiError('component_manifest_invalid', 400)
    }
    const kind = classifyManifest(raw)
    if (kind === 'broken') throw new CustomUiError('component_manifest_invalid', 400)
    // 旧 iframe 格式一律拒绝（规格 5.7、5.10），并说明旧机制已删除。
    if (kind === 'legacy') throw new CustomUiError('component_legacy_unsupported', 409)
    const normalized = normalizeNativeManifest(id, raw, { hostVersion: this.hostVersion() })
    if (!normalized.manifest)
      throw new CustomUiError('component_manifest_invalid', 400, { errors: normalized.errors })
    const manifest = normalized.manifest
    const errors = []
    if (!assets.has(manifest.entry))
      errors.push({ path: 'entry', code: 'entry_missing', params: { entry: manifest.entry } })
    manifest.styles.forEach((style, index) => {
      if (!assets.has(style)) errors.push({ path: `styles.${index}`, code: 'style_missing' })
    })
    for (const path of assets.keys()) {
      if (/\.html?$/i.test(path)) errors.push({ path, code: 'component_html_unsupported' })
      else if (!isSafeComponentPath(path))
        errors.push({ path, code: 'component_file_path_invalid' })
    }
    if (errors.length) throw new CustomUiError('component_files_invalid', 400, { errors })

    await mkdir(this.root, { recursive: true, mode: 0o700 })
    const existing = await this.list()
    if (existing.length >= MAX_STAGED) throw new CustomUiError('component_staging_full', 409)
    const stageId = randomBytes(8).toString('hex')
    const base = join(this.root, stageId)
    try {
      for (const [path, content] of assets) {
        const file = join(base, 'component', ...path.split('/'))
        await mkdir(dirname(file), { recursive: true, mode: 0o700 })
        await writeFile(file, content, { flag: 'wx', mode: 0o600 })
      }
      /** @type {StagedMeta} */
      const meta = {
        version: 1,
        stageId,
        id,
        name: manifest.name,
        componentVersion: manifest.version,
        fileName: cleanFileName(fileName) || `${id}.zip`,
        bytes: bytes.byteLength,
        sha256: createHash('sha256').update(bytes).digest('hex'),
        createdAt: new Date(this.now()).toISOString(),
      }
      // meta.json 最后写：清单只认有 meta 的暂存，写到一半的目录在下次清理时删掉。
      await writeJsonAtomic(join(base, 'meta.json'), meta, { mode: 0o600 })
      return meta
    } catch (error) {
      await rm(base, { recursive: true, force: true }).catch(() => {})
      throw error
    }
  }

  /** @returns {Promise<StagedMeta[]>} 最新的在前 */
  async list() {
    const entries = await readdir(this.root, { withFileTypes: true }).catch(() => [])
    const rows = []
    for (const entry of entries) {
      if (!entry.isDirectory() || !isStageId(entry.name)) continue
      const meta = parseMeta(
        await readJson(join(this.root, entry.name, 'meta.json'), null).catch(() => null),
      )
      if (meta && meta.stageId === entry.name) rows.push(meta)
    }
    return rows.sort((left, right) => right.createdAt.localeCompare(left.createdAt))
  }

  /** @param {string} stageId @returns {Promise<StagedMeta>} */
  async require(stageId) {
    if (!isStageId(stageId)) throw new CustomUiError('component_staged_not_found', 404)
    const meta = parseMeta(
      await readJson(join(this.root, stageId, 'meta.json'), null).catch(() => null),
    )
    if (!meta || meta.stageId !== stageId)
      throw new CustomUiError('component_staged_not_found', 404)
    return meta
  }

  /**
   * 暂存组件的全部文件内容（安装与 diff 用）。
   * @param {string} stageId
   */
  async readTree(stageId) {
    await this.require(stageId)
    const tree = await readComponentTree(this.componentDir(stageId))
    if (!tree) throw new CustomUiError('component_staged_not_found', 404)
    return tree
  }

  /**
   * 源码预览：只读文本文件，≤512 KiB 时整份返回，否则截断。路径按组件路径规则校验。
   * @param {string} stageId
   * @param {unknown} path
   */
  async readFile(stageId, path) {
    if (typeof path !== 'string' || !isSafeComponentPath(path))
      throw new CustomUiError('component_file_path_invalid', 400)
    const tree = await this.readTree(stageId)
    const content = tree.get(path)
    if (!content) throw new CustomUiError('component_file_not_found', 404)
    if (!PREVIEW_FILE.test(path))
      return {
        path,
        type: componentFileType(path) || 'application/octet-stream',
        binary: true,
        content: '',
        truncated: false,
      }
    const truncated = content.byteLength > MAX_PREVIEW_BYTES
    return {
      path,
      type: componentFileType(path) || 'text/plain; charset=utf-8',
      binary: false,
      content: toWellFormed(content.subarray(0, MAX_PREVIEW_BYTES).toString('utf8')),
      truncated,
    }
  }

  /** @param {string} stageId */
  async discard(stageId) {
    await this.require(stageId)
    await rm(join(this.root, stageId), { recursive: true, force: true })
  }

  /** 启动清理：超过 7 天的暂存，以及没有 meta.json 的残缺目录（写到一半时进程退出）。 */
  async cleanup() {
    const entries = await readdir(this.root, { withFileTypes: true }).catch(() => [])
    const cutoff = this.now() - STAGE_RETENTION_MS
    for (const entry of entries) {
      if (!entry.isDirectory() || !isStageId(entry.name)) continue
      const dir = join(this.root, entry.name)
      const meta = parseMeta(await readJson(join(dir, 'meta.json'), null).catch(() => null))
      const info = await lstat(dir).catch(() => null)
      const created = meta ? Date.parse(meta.createdAt) : (info?.mtimeMs ?? 0)
      // 没有 meta 的目录可能正在写（同一数据目录的另一个进程）：给它 10 分钟。
      const stale = meta ? created < cutoff : created < this.now() - 10 * 60_000
      if (stale) await rm(dir, { recursive: true, force: true }).catch(() => {})
    }
  }
}
