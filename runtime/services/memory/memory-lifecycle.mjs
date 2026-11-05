// 记忆的生命周期：到期、引用失效、长期不用、超保留期，以及确认、恢复和撤销。
// 除用户删除外（见 local-memory-runtime.mjs 的 forget），这里的每一步都写 memory_events，事后能看、能撤销。
// 全部不调 LLM，放在读路径和启动路径上也不贵。
import { createHash } from 'node:crypto'
import { readFileSync, statSync } from 'node:fs'
import { isAbsolute, resolve } from 'node:path'
import { decayFactor, factTimestamp } from './memory-retrieval.mjs'

const DAY_MS = 86_400_000
const HOUR_MS = 3_600_000
const ANCHOR_MAX_BYTES = 2 * 1024 * 1024
const ANCHOR_CHECKS_PER_RETRIEVAL = 8
const SOFT_LIMIT = 2000
const SOFT_LIMIT_BOT = 500
const SUPERSEDED_RETENTION_DAYS = 365
const SUPERSEDED_KEEP_VERSIONS = 5
const ARCHIVED_RETENTION_DAYS = 180
const EVENT_RETENTION_DAYS = 180
const EVENT_SNAPSHOT_RETENTION_DAYS = 30
// 单次清扫的上限：清扫跑在读路径上，积压再多也分几次做完，不卡住请求。
const MAX_ARCHIVED_PER_SWEEP = 200
const MAX_PURGED_PER_SWEEP = 500

// 权威档：用户自己说的（≥90）> 模型推断和工具结果（≥40）> 外部渠道。低一档不能替换高一档的事实。
export const authorityTier = (authority) => (authority >= 90 ? 3 : authority >= 40 ? 2 : 1)

// 撤销要能完整写回的字段。不含 id、时间戳和索引状态。
const EVENT_SNAPSHOT_FIELDS = [
  'space_id',
  'status',
  'superseded_by',
  'superseded_at',
  'valid_from',
  'valid_to',
  'archived_at',
  'archive_reason',
  'title',
  'content',
  'type',
  'topic_key',
  'identity_key',
  'source_type',
  'source_path',
  'evidence',
  'evidence_role',
  'importance',
  'authority',
  'pinned',
  'tainted',
  'expires_at',
  'confirm_count',
  'last_confirmed_at',
  'anchor_hash',
]
const SNAPSHOT_FIELD_SET = new Set(EVENT_SNAPSHOT_FIELDS)

export function memorySnapshot(row) {
  return Object.fromEntries(EVENT_SNAPSHOT_FIELDS.map((field) => [field, row[field] ?? null]))
}

// 只看普通文件：FIFO 和设备文件一读就可能挂住。
function inspectAnchor(sourcePath, cwd) {
  const value = String(sourcePath || '')
  if (!value) return { state: 'unknown', hash: '' }
  if (!isAbsolute(value) && !cwd) return { state: 'unknown', hash: '' }
  const path = isAbsolute(value) ? value : resolve(String(cwd), value)
  try {
    const stat = statSync(path)
    if (!stat.isFile()) return { state: 'missing', hash: '' }
    // 记录锚点时只收 2MB 以内的文件，现在超了说明内容肯定变过。
    if (stat.size > ANCHOR_MAX_BYTES) return { state: 'too_large', hash: '' }
    const hash = createHash('sha256').update(readFileSync(path)).digest('hex').slice(0, 16)
    return { state: 'ok', hash }
  } catch (error) {
    if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return { state: 'missing', hash: '' }
    return { state: 'unknown', hash: '' }
  }
}

// source_path 指向内容的哈希。相对路径按 cwd 解析；没有路径或读不到时返回空字符串。
export function computeAnchorHash(sourcePath, cwd = '') {
  return inspectAnchor(sourcePath, cwd).hash
}

function archiveRow(store, row, reason, now) {
  store.db
    .prepare(
      `UPDATE memories SET status = 'archived', archived_at = ?, archive_reason = ?, updated_at = ?,
         valid_to = CASE WHEN ? = 'expired' THEN COALESCE(valid_to, expires_at) ELSE valid_to END
       WHERE id = ?`,
    )
    .run(now, reason, now, reason, row.id)
  store.indexFact(row.id)
  store.recordEvent({
    actor: 'lifecycle',
    action: 'archive',
    memoryIds: [row.id],
    spaceId: row.space_id,
    before: { [row.id]: memorySnapshot(row) },
    reason,
  })
}

function archiveExpired(store, now) {
  const rows = store.db
    .prepare(
      `SELECT * FROM memories
       WHERE status IN ('active', 'stale') AND expires_at IS NOT NULL AND expires_at <= ?
       LIMIT ?`,
    )
    .all(now, MAX_ARCHIVED_PER_SWEEP)
  for (const row of rows) archiveRow(store, row, 'expired', now)
  return rows
}

function memoryUtility(row, nowMs) {
  const anchor = Math.max(
    Date.parse(factTimestamp(row)) || 0,
    Date.parse(row.last_confirmed_at || '') || 0,
    Date.parse(row.last_accessed_at || '') || 0,
  )
  const decay = decayFactor(row.type, anchor ? (nowMs - anchor) / DAY_MS : 0, {
    pinned: Boolean(row.pinned),
  })
  return (
    Number(row.importance || 0) *
    (Number(row.authority || 0) / 100) *
    decay *
    Math.log2(2 + Number(row.access_count || 0))
  )
}

// 超软上限时，把最没用的转归档。钉住的和用户亲口说的不会因为长期不用被归档。
function archiveUnused(store, now, nowMs, budget) {
  const spaces = store.db
    .prepare(
      `SELECT spaces.id AS id, spaces.kind AS kind, COUNT(*) AS total
       FROM memories JOIN memory_spaces spaces ON spaces.id = memories.space_id
       WHERE memories.status IN ('active', 'stale')
       GROUP BY spaces.id`,
    )
    .all()
  const archived = []
  for (const space of spaces) {
    const limit = space.kind === 'bot' ? SOFT_LIMIT_BOT : SOFT_LIMIT
    const excess = Number(space.total) - limit
    if (excess <= 0 || archived.length >= budget) continue
    const rows = store.db
      .prepare(
        `SELECT * FROM memories
         WHERE space_id = ? AND status IN ('active', 'stale') AND pinned = 0 AND authority < 90`,
      )
      .all(space.id)
    rows
      .map((row) => ({ row, utility: memoryUtility(row, nowMs) }))
      .sort((left, right) => left.utility - right.utility)
      .slice(0, Math.min(excess, budget - archived.length))
      .forEach(({ row }) => {
        archiveRow(store, row, 'unused', now)
        archived.push(row)
      })
  }
  return archived
}

// 物理清除超保留期的旧版本和归档。不写 memory_forgotten：这不是用户要删，同样的内容以后还能再记。
function purgeExpired(store, nowMs) {
  const supersededBefore = new Date(nowMs - SUPERSEDED_RETENTION_DAYS * DAY_MS).toISOString()
  const archivedBefore = new Date(nowMs - ARCHIVED_RETENTION_DAYS * DAY_MS).toISOString()
  const rows = store.db
    .prepare(
      `SELECT id, space_id, superseded_by FROM (
         SELECT id, space_id, superseded_by, superseded_at, updated_at,
           ROW_NUMBER() OVER (
             PARTITION BY space_id, identity_key
             ORDER BY COALESCE(valid_from, created_at) DESC, created_at DESC
           ) AS version
         FROM memories WHERE status = 'superseded'
       )
       WHERE version > ? AND COALESCE(superseded_at, updated_at) <= ?
       UNION ALL
       SELECT id, space_id, superseded_by FROM memories
       WHERE status = 'archived' AND COALESCE(archived_at, updated_at) <= ?
       LIMIT ?`,
    )
    .all(SUPERSEDED_KEEP_VERSIONS, supersededBefore, archivedBefore, MAX_PURGED_PER_SWEEP)
  const relink = store.db.prepare('UPDATE memories SET superseded_by = ? WHERE superseded_by = ?')
  const remove = store.db.prepare('DELETE FROM memories WHERE id = ?')
  for (const row of rows) {
    // 版本链中间少一环时接上，否则更早的版本就找不到后继了。
    relink.run(row.superseded_by || '', row.id)
    store.index.remove(row.id)
    remove.run(row.id)
  }
  return rows
}

function pruneEvents(store, nowMs) {
  const deleteBefore = new Date(nowMs - EVENT_RETENTION_DAYS * DAY_MS).toISOString()
  const snapshotBefore = new Date(nowMs - EVENT_SNAPSHOT_RETENTION_DAYS * DAY_MS).toISOString()
  const deleted = store.db.prepare('DELETE FROM memory_events WHERE at <= ?').run(deleteBefore)
  // 快照带着记忆正文，只留够撤销的时间。
  const cleared = store.db
    .prepare("UPDATE memory_events SET before_json = '' WHERE before_json <> '' AND at <= ?")
    .run(snapshotBefore)
  return { eventsDeleted: Number(deleted.changes), snapshotsCleared: Number(cleared.changes) }
}

function markDirtyBySpace(store, rows) {
  const bySpace = new Map()
  for (const row of rows) {
    if (!bySpace.has(row.space_id)) bySpace.set(row.space_id, [])
    bySpace.get(row.space_id).push(row.id)
  }
  for (const [spaceId, ids] of bySpace) store.markDerivedDirty(ids, { spaceId })
}

export function sweepMemoryLifecycle(store) {
  const nowMs = store.now()
  const now = store.isoNow()
  return store.transaction(() => {
    const expired = archiveExpired(store, now)
    const unused = archiveUnused(store, now, nowMs, MAX_ARCHIVED_PER_SWEEP - expired.length)
    const purged = purgeExpired(store, nowMs)
    const events = pruneEvents(store, nowMs)
    markDirtyBySpace(store, [...expired, ...unused, ...purged])
    return {
      archived: expired.length + unused.length,
      expired: expired.length,
      unused: unused.length,
      purged: purged.length,
      ...events,
    }
  })
}

// 用前校验引用：检索命中带 source_path 的事实时看一眼文件，没了或变了就标 stale。
// hit.row.status 同步改掉，让这次结果就带上 stale。stale 的不再重算，等 Agent 或用户确认。
export function checkMemoryAnchors(store, hits) {
  const nowMs = store.now()
  let checked = 0
  for (const hit of hits) {
    if (checked >= ANCHOR_CHECKS_PER_RETRIEVAL) return
    if (hit.kind !== 'fact') continue
    const row = hit.row
    if (!row.source_path || !row.anchor_hash || row.status !== 'active') continue
    const last = Date.parse(row.anchor_checked_at || '')
    if (!Number.isNaN(last) && nowMs - last < HOUR_MS) continue
    checked += 1
    const now = store.isoNow()
    const { state, hash } = inspectAnchor(row.source_path, row.cwd)
    const reason =
      state === 'missing'
        ? 'anchor_missing'
        : state === 'too_large' || (state === 'ok' && hash !== row.anchor_hash)
          ? 'anchor_changed'
          : ''
    row.anchor_checked_at = now
    if (!reason) {
      store.db.prepare('UPDATE memories SET anchor_checked_at = ? WHERE id = ?').run(now, row.id)
      continue
    }
    store.transaction(() => {
      store.db
        .prepare(
          "UPDATE memories SET status = 'stale', anchor_checked_at = ?, updated_at = ? WHERE id = ?",
        )
        .run(now, now, row.id)
      store.indexFact(row.id)
      store.recordEvent({
        actor: 'lifecycle',
        action: 'stale',
        memoryIds: [row.id],
        spaceId: row.space_id,
        before: { [row.id]: memorySnapshot(row) },
        reason,
      })
    })
    row.status = 'stale'
  }
}

// Agent 或用户核对过文件后确认它仍然有效：刷新锚点，记一次证实。
export function confirmMemory(store, id) {
  const row = store.db.prepare('SELECT * FROM memories WHERE id = ?').get(id)
  if (!row) return null
  if (row.status !== 'active' && row.status !== 'stale') return store.getMemory(id)
  const now = store.isoNow()
  store.transaction(() => {
    store.db
      .prepare(
        `UPDATE memories SET status = 'active', confirm_count = confirm_count + 1, last_confirmed_at = ?,
           anchor_hash = ?, anchor_checked_at = ?, updated_at = ?
         WHERE id = ?`,
      )
      .run(now, computeAnchorHash(row.source_path, row.cwd), now, now, id)
    store.indexFact(id)
    store.recordEvent({
      actor: 'user',
      action: 'confirm',
      memoryIds: [id],
      spaceId: row.space_id,
      before: { [id]: memorySnapshot(row) },
    })
  })
  return store.getMemory(id)
}

// 从归档或历史里找回：用户选了它，所以权威度按用户说的算（外部来源除外），也不再被后台对账碰。
// 同一件事现在的版本（同 identity_key，或当初替换它的那条）反过来被它替换。
export function restoreMemory(store, id) {
  const db = store.db
  const row = db.prepare('SELECT * FROM memories WHERE id = ?').get(id)
  if (!row) return null
  if (row.status !== 'archived' && row.status !== 'superseded') return store.getMemory(id)
  const now = store.isoNow()
  const replaced = db
    .prepare(
      `SELECT * FROM memories
       WHERE id <> ? AND space_id = ? AND status IN ('active', 'stale')
         AND (identity_key = ? OR id = ?)`,
    )
    .all(id, row.space_id, row.identity_key, row.superseded_by || '')
  store.transaction(() => {
    db.prepare(
      `UPDATE memories SET status = 'active', archived_at = NULL, archive_reason = '', superseded_by = '',
         superseded_at = NULL, valid_to = NULL, last_confirmed_at = ?, reconcile_status = 'done', updated_at = ?,
         authority = CASE WHEN tainted = 0 THEN MAX(authority, 90) ELSE authority END,
         expires_at = CASE WHEN expires_at <= ? THEN NULL ELSE expires_at END
       WHERE id = ?`,
    ).run(now, now, now, id)
    const supersede = db.prepare(
      "UPDATE memories SET status = 'superseded', superseded_by = ?, superseded_at = ?, valid_to = ?, updated_at = ? WHERE id = ?",
    )
    for (const other of replaced) {
      supersede.run(id, now, now, now, other.id)
      store.indexFact(other.id)
    }
    store.indexFact(id)
    store.recordEvent({
      actor: 'user',
      action: 'restore',
      memoryIds: [id, ...replaced.map((other) => other.id)],
      spaceId: row.space_id,
      before: Object.fromEntries([row, ...replaced].map((item) => [item.id, memorySnapshot(item)])),
    })
  })
  store.markDerivedDirty([id, ...replaced.map((other) => other.id)], { spaceId: row.space_id })
  // 归档状态的事实没跑过语义扩展。
  if (row.semantic_status !== 'ready') store.scheduleSemantic([id])
  return store.getMemory(id)
}

function parseJson(value, fallback) {
  try {
    return JSON.parse(value) ?? fallback
  } catch {
    return fallback
  }
}

function writeSnapshot(store, id, snapshot, now) {
  const fields = Object.keys(snapshot).filter((field) => SNAPSHOT_FIELD_SET.has(field))
  if (!fields.length) return
  store.db
    .prepare(
      `UPDATE memories SET ${fields.map((field) => `${field} = ?`).join(', ')}, updated_at = ? WHERE id = ?`,
    )
    .run(...fields.map((field) => snapshot[field]), now, id)
}

// 把一次改动写回去。快照过了 30 天会被清掉，那之后就不能撤销了。
export function undoMemoryEvent(store, id) {
  const db = store.db
  const event = db.prepare('SELECT * FROM memory_events WHERE id = ?').get(id)
  if (!event || !event.before_json || event.undone_at) return null
  const snapshots = parseJson(event.before_json, null)
  if (!snapshots || typeof snapshots !== 'object') return null
  const memoryIds = parseJson(event.memory_ids, [])
  const now = store.isoNow()
  const touched = new Map()
  const needsExpansion = []
  store.transaction(() => {
    const load = db.prepare('SELECT * FROM memories WHERE id = ?')
    for (const [rowId, snapshot] of Object.entries(snapshots)) {
      const current = load.get(rowId)
      if (!current) continue
      writeSnapshot(store, rowId, snapshot, now)
      touched.set(rowId, current)
      if (snapshot.title !== current.title || snapshot.content !== current.content) {
        db.prepare(
          "UPDATE memories SET semantic_text = '', semantic_status = 'pending' WHERE id = ?",
        ).run(rowId)
        needsExpansion.push(rowId)
      }
      // 撤销一次归档后要留得住：否则下一轮清扫又会因为同样的原因把它归档。
      if (event.action === 'archive' && snapshot.status !== 'archived') {
        db.prepare(
          `UPDATE memories SET last_confirmed_at = ?,
             expires_at = CASE WHEN expires_at <= ? THEN NULL ELSE expires_at END
           WHERE id = ?`,
        ).run(now, now, rowId)
      }
    }
    // 替换被撤销：新事实退到归档，还能再恢复。
    const newest = event.action === 'supersede' ? memoryIds[0] : ''
    const newestRow = newest ? load.get(newest) : null
    if (newestRow) {
      db.prepare(
        `UPDATE memories SET status = 'archived', archived_at = ?, archive_reason = 'undone',
           reconcile_status = 'done', updated_at = ?
         WHERE id = ?`,
      ).run(now, now, newest)
      touched.set(newest, newestRow)
    }
    for (const rowId of touched.keys()) store.indexFact(rowId)
    db.prepare('UPDATE memory_events SET undone_at = ? WHERE id = ?').run(now, id)
    store.recordEvent({
      actor: 'user',
      action: 'undo',
      memoryIds: [...touched.keys()],
      spaceId: event.space_id,
      reason: event.action,
    })
  })
  const ids = [...touched.keys()]
  for (const spaceId of new Set([...touched.values()].map((row) => row.space_id)))
    store.markDerivedDirty(ids, { spaceId })
  if (needsExpansion.length) store.scheduleSemantic(needsExpansion)
  return { id, undoneAt: now, memoryIds: ids }
}
