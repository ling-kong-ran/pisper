// 组件版本（规格 5.9）：custom-ui-versions/<id>/<n>/ 是第 n 版的完整副本，index.json 记序号。
// 快照在每次写入（代理写入、替换导入、回滚）之前拍下，所以 index 里第 n 条记的是“把第 n 版改成第 n+1 版”的那次写入：
// 时间、说明、来源与改动文件都描述第 n+1 版。当前目录 = 第 (最大序号 + 1) 版；没有快照时当前就是 v1。
// 回滚 = 先给当前拍快照、再把目录换成第 n 版的内容，生成新的一版，不改写历史。
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { generateUnifiedPatch } from '../runtime/pi-coding-agent.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { COMPONENT_ID_PATTERN, listComponentFiles } from './custom-ui-files.mjs'

export const MAX_COMPONENT_VERSIONS = 20
const MAX_DIFF_CHARS = 200_000
const MAX_DIFF_FILE_BYTES = 512 * 1024
const TEXT_FILE = /\.(?:m?js|css|json|svg|md|txt)$/i
export const VERSION_SOURCES = Object.freeze(
  /** @type {const} */ (['agent', 'import', 'restore', 'local']),
)

/**
 * @typedef {{ path: string, added: number, removed: number }} VersionFileChange
 * @typedef {{
 *   n: number,
 *   createdAt: string,
 *   summary: string,
 *   source?: (typeof VERSION_SOURCES)[number],
 *   files?: VersionFileChange[],
 *   deleted?: string[],
 * }} SnapshotEntry
 * @typedef {{
 *   n: number,
 *   at: string | null,
 *   summary: string,
 *   source: (typeof VERSION_SOURCES)[number] | null,
 *   files: VersionFileChange[],
 *   deleted: string[],
 * }} VersionInfo
 */

/** @param {unknown} value @returns {VersionFileChange[]} */
function parseFileChanges(value) {
  if (!Array.isArray(value)) return []
  return value
    .filter(
      (item) =>
        item &&
        typeof item.path === 'string' &&
        item.path.length <= 512 &&
        Number.isSafeInteger(item.added) &&
        Number.isSafeInteger(item.removed),
    )
    .slice(0, 64)
    .map((item) => ({ path: item.path, added: item.added, removed: item.removed }))
}

/**
 * @param {string} versionsRoot
 * @param {string} id
 * @returns {Promise<SnapshotEntry[]>} 按序号升序
 */
export async function readSnapshotIndex(versionsRoot, id) {
  if (!COMPONENT_ID_PATTERN.test(id)) return []
  const raw = await readJson(join(versionsRoot, id, 'index.json'), null).catch(() => null)
  if (!Array.isArray(raw?.snapshots)) return []
  return raw.snapshots
    .filter((/** @type {any} */ item) => Number.isSafeInteger(item?.n) && item.n > 0)
    .map((/** @type {any} */ item) => ({
      n: item.n,
      createdAt: typeof item.createdAt === 'string' ? item.createdAt : '',
      summary: typeof item.summary === 'string' ? item.summary.slice(0, 200) : '',
      ...(VERSION_SOURCES.includes(item.source) ? { source: item.source } : {}),
      ...(Array.isArray(item.files) ? { files: parseFileChanges(item.files) } : {}),
      ...(Array.isArray(item.deleted)
        ? {
            deleted: item.deleted
              .filter((path) => typeof path === 'string' && path.length <= 512)
              .slice(0, 64),
          }
        : {}),
    }))
    .sort((left, right) => left.n - right.n)
}

/**
 * 写入成功后补记第 n 条快照描述的那次写入（来源与改动文件）。只做加法：旧 index 没有这些字段也能读。
 * 调用方负责按组件串行。
 * @param {string} versionsRoot
 * @param {string} id
 * @param {number} n
 * @param {{ source: (typeof VERSION_SOURCES)[number], files?: VersionFileChange[], deleted?: string[] }} patch
 */
export async function annotateSnapshot(versionsRoot, id, n, patch) {
  const indexPath = join(versionsRoot, id, 'index.json')
  const raw = await readJson(indexPath, null).catch(() => null)
  if (!Array.isArray(raw?.snapshots)) return
  const snapshots = raw.snapshots.map((/** @type {any} */ item) =>
    item?.n === n
      ? {
          ...item,
          source: patch.source,
          files: parseFileChanges(patch.files || []),
          deleted: (patch.deleted || []).slice(0, 64),
        }
      : item,
  )
  await writeJsonAtomic(indexPath, { ...raw, snapshots })
}

/**
 * 版本清单（GET …/versions）：当前版在前，其余按序号倒序。
 * 第 k 版的时间、说明与来源取自第 k-1 条快照（那次写入产生了第 k 版）；更早的记录被裁掉时如实留空。
 * @param {SnapshotEntry[]} snapshots
 * @param {{ createdAt?: string | null }} [origin] 组件创建时间（第 1 版的时间）
 * @returns {{ current: VersionInfo, versions: VersionInfo[] }}
 */
export function describeVersions(snapshots, { createdAt = null } = {}) {
  const byNumber = new Map(snapshots.map((entry) => [entry.n, entry]))
  const latest = snapshots.reduce((highest, entry) => Math.max(highest, entry.n), 0)
  /** @param {number} n @returns {VersionInfo} */
  const info = (n) => {
    const producer = byNumber.get(n - 1)
    return {
      n,
      at: producer?.createdAt || (n === 1 ? createdAt : null),
      summary: producer?.summary || '',
      source: producer?.source || null,
      files: producer?.files || [],
      deleted: producer?.deleted || [],
    }
  }
  return {
    current: info(latest + 1),
    versions: [...snapshots].sort((left, right) => right.n - left.n).map((entry) => info(entry.n)),
  }
}

/**
 * 读出一个目录里写入器可见的全部文件（与快照、哈希同一份遍历规则）。
 * @param {string} dir
 * @returns {Promise<Map<string, Buffer> | null>} 目录超限或不存在时返回 null
 */
export async function readComponentTree(dir) {
  const listed = await listComponentFiles(dir)
  if (listed.error) return null
  /** @type {Map<string, Buffer>} */
  const files = new Map()
  for (const entry of listed.files) {
    const content = await readFile(entry.file).catch(() => null)
    if (content) files.set(entry.path, content)
  }
  return files
}

/** @param {string} versionsRoot @param {string} id @param {number} n */
export function snapshotDir(versionsRoot, id, n) {
  return join(versionsRoot, id, String(n))
}

/** @param {string} path */
function diffPath(path) {
  return /\s|"|\\/.test(path) ? `"${path.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"` : path
}

/** @param {Buffer | undefined} content */
function textOf(content) {
  if (!content) return ''
  return content.toString('utf8').replace(/\r\n/g, '\n')
}

/**
 * 两版之间的统一 diff（与会话改动查看器同一格式：diff --git 头 + @@ 块），供“对比”与导入替换时的 diff。
 * 二进制或超过 512 KiB 的文件只给一行说明，不展开内容。
 * @param {Map<string, Buffer>} before
 * @param {Map<string, Buffer>} after
 * @returns {{ diff: string, truncated: boolean, files: { path: string, status: 'added' | 'deleted' | 'modified' }[] }}
 */
export function diffComponentTrees(before, after) {
  const paths = [...new Set([...before.keys(), ...after.keys()])].sort()
  /** @type {{ path: string, status: 'added' | 'deleted' | 'modified' }[]} */
  const files = []
  const parts = []
  for (const path of paths) {
    const old = before.get(path)
    const next = after.get(path)
    if (old && next && old.equals(next)) continue
    const status = !old ? 'added' : !next ? 'deleted' : 'modified'
    files.push({ path, status })
    const a = old ? diffPath(`a/${path}`) : '/dev/null'
    const b = next ? diffPath(`b/${path}`) : '/dev/null'
    const header = `diff --git ${diffPath(`a/${path}`)} ${diffPath(`b/${path}`)}\n${
      status === 'added'
        ? 'new file mode 100644\n'
        : status === 'deleted'
          ? 'deleted file mode 100644\n'
          : ''
    }`
    const large =
      (old?.byteLength || 0) > MAX_DIFF_FILE_BYTES || (next?.byteLength || 0) > MAX_DIFF_FILE_BYTES
    if (!TEXT_FILE.test(path) || large) {
      parts.push(`${header}Binary files ${a} and ${b} differ\n`)
      continue
    }
    const patch = generateUnifiedPatch(path, textOf(old), textOf(next))
      .replace(/^--- .*$/m, `--- ${a}`)
      .replace(/^\+\+\+ .*$/m, `+++ ${b}`)
    parts.push(`${header}${patch}${patch.endsWith('\n') ? '' : '\n'}`)
  }
  const diff = parts.join('')
  if (diff.length <= MAX_DIFF_CHARS) return { diff, truncated: false, files }
  return { diff: diff.slice(0, MAX_DIFF_CHARS), truncated: true, files }
}
