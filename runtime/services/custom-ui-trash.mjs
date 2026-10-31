// 组件回收站（规格 5.9）：删除的组件在 custom-ui-trash/<id>-<UTC>/ 里保留 7 天，可以恢复；过期的启动时清理。
// 目录结构由 moveComponentToTrash 写入：component/（原目录）、可选 data.json（组件存储）、meta.json（状态与实例快照）。
//
// 例外：启动迁移移进来的旧 iframe 组件与坏目录（meta.reason = 'legacy-retired'）不参与 7 天自动清理。
// 它们是负责人决定删除时用户手上的唯一副本，回滚旧版本前可能要手动移回；只在用户点“永久删除”时才删。
import { lstat, readdir, rename, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { readJson } from '../storage/json-file.mjs'
import { COMPONENT_ID_PATTERN, renameWithRetry } from './custom-ui-files.mjs'

export const TRASH_RETENTION_MS = 7 * 24 * 60 * 60 * 1000
const TRASH_ENTRY_PATTERN = /^([a-z0-9][a-z0-9._-]{0,63})-(\d{8}T\d{6}Z)(?:-\d{1,4})?$/
const MAX_TRASH_ENTRIES = 200

/**
 * @typedef {{
 *   entry: string,
 *   id: string,
 *   name: string,
 *   version: string,
 *   kind: 'native' | 'legacy' | 'broken',
 *   reason: 'deleted' | 'legacy-retired',
 *   deletedAt: string,
 *   expiresAt: string | null,
 *   instances: number,
 *   hasData: boolean,
 * }} TrashEntry
 */

/** @param {unknown} value */
export function isTrashEntryName(value) {
  return typeof value === 'string' && TRASH_ENTRY_PATTERN.test(value)
}

/** 从目录名里的 UTC 时间戳还原删除时间（meta.json 坏了也能判断是否过期）。 @param {string} stamp */
function stampTime(stamp) {
  const match = stamp.match(/^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z$/)
  if (!match) return NaN
  const [, y, mo, d, h, mi, s] = match
  return Date.UTC(Number(y), Number(mo) - 1, Number(d), Number(h), Number(mi), Number(s))
}

/** @param {unknown} value @param {number} max */
function text(value, max) {
  return typeof value === 'string' ? value.trim().slice(0, max) : ''
}

/**
 * @param {string} trashRoot
 * @param {string} entry
 * @returns {Promise<TrashEntry | null>}
 */
async function readTrashEntry(trashRoot, entry) {
  const match = entry.match(TRASH_ENTRY_PATTERN)
  if (!match) return null
  const dir = join(trashRoot, entry)
  const info = await lstat(dir).catch(() => null)
  if (!info?.isDirectory()) return null
  const meta = await readJson(join(dir, 'meta.json'), null).catch(() => null)
  const manifest = await readJson(join(dir, 'component', 'manifest.json'), null).catch(() => null)
  const reason = meta?.reason === 'legacy-retired' ? 'legacy-retired' : 'deleted'
  const deletedTime = Date.parse(meta?.deletedAt) || stampTime(match[2]) || info.mtimeMs
  const kind =
    manifest && typeof manifest === 'object' && !Array.isArray(manifest)
      ? manifest.runtime === 'native'
        ? 'native'
        : 'legacy'
      : 'broken'
  return {
    entry,
    id: match[1],
    name: text(manifest?.name, 120) || match[1],
    version: text(manifest?.version, 64),
    kind,
    reason,
    deletedAt: new Date(deletedTime).toISOString(),
    expiresAt:
      reason === 'legacy-retired' ? null : new Date(deletedTime + TRASH_RETENTION_MS).toISOString(),
    instances: Array.isArray(meta?.instances) ? meta.instances.length : 0,
    hasData: Boolean(await lstat(join(dir, 'data.json')).catch(() => null)),
  }
}

/**
 * 回收站清单，最新删除的在前。
 * @param {string} trashRoot
 * @returns {Promise<TrashEntry[]>}
 */
export async function listTrash(trashRoot) {
  const entries = await readdir(trashRoot, { withFileTypes: true }).catch(() => [])
  const rows = []
  for (const entry of entries) {
    if (!entry.isDirectory() || !isTrashEntryName(entry.name)) continue
    const row = await readTrashEntry(trashRoot, entry.name)
    if (row) rows.push(row)
    if (rows.length >= MAX_TRASH_ENTRIES) break
  }
  return rows.sort((left, right) => right.deletedAt.localeCompare(left.deletedAt))
}

/**
 * 读一条回收站记录的元数据（恢复时用来还原状态与实例）。
 * @param {string} trashRoot
 * @param {string} entry
 */
export async function readTrashMeta(trashRoot, entry) {
  if (!isTrashEntryName(entry)) return null
  const row = await readTrashEntry(trashRoot, entry)
  if (!row) return null
  const meta = await readJson(join(trashRoot, entry, 'meta.json'), null).catch(() => null)
  return { row, meta: meta && typeof meta === 'object' && !Array.isArray(meta) ? meta : {} }
}

/**
 * 把组件目录与存储文件移回原处；目标已存在时不覆盖（由调用方先检查并报 component_already_installed）。
 * 成功后删掉回收站里剩下的 meta.json 与空目录。调用方负责按组件串行。
 * @param {{ trashRoot: string, root: string, dataRoot: string, entry: string, id: string, renameDirectory?: (from: string, to: string) => Promise<void> }} options
 */
export async function moveOutOfTrash({
  trashRoot,
  root,
  dataRoot,
  entry,
  id,
  renameDirectory = rename,
}) {
  if (!isTrashEntryName(entry) || !COMPONENT_ID_PATTERN.test(id))
    throw new Error('trash entry invalid')
  const dir = join(trashRoot, entry)
  await renameWithRetry(join(dir, 'component'), join(root, id), renameDirectory)
  const data = join(dir, 'data.json')
  const storage = join(dataRoot, `${id}.json`)
  // 存储文件：同名新组件已有存储时保留现有的，不覆盖（回收站里的那份随目录一起删掉）。
  if ((await lstat(data).catch(() => null)) && !(await lstat(storage).catch(() => null))) {
    await rename(data, storage).catch(() => {})
  }
  await rm(dir, { recursive: true, force: true }).catch(() => {})
}

/**
 * 永久删除一条回收站记录。
 * @param {string} trashRoot
 * @param {string} entry
 * @returns {Promise<boolean>} 记录存在并已删除
 */
export async function purgeTrashEntry(trashRoot, entry) {
  if (!isTrashEntryName(entry)) return false
  const dir = join(trashRoot, entry)
  const info = await lstat(dir).catch(() => null)
  if (!info?.isDirectory()) return false
  await rm(dir, { recursive: true, force: true })
  return true
}

/**
 * 启动时清理超过 7 天的记录；旧组件迁移进来的记录不自动清（见文件头）。
 * @param {{ trashRoot: string, now?: () => number }} options
 * @returns {Promise<string[]>} 被删除的记录名
 */
export async function purgeExpiredTrash({ trashRoot, now = Date.now }) {
  const removed = []
  for (const row of await listTrash(trashRoot)) {
    if (!row.expiresAt || Date.parse(row.expiresAt) > now()) continue
    if (await purgeTrashEntry(trashRoot, row.entry).catch(() => false)) removed.push(row.entry)
  }
  return removed
}
