// L0 原文层：每轮对话脱敏后存一份（用户消息 + 最终回复，不含工具输出），用来跨会话检索和后台批量抽取。
// 它只是证据，不自动进上下文；会话删除时一起删。
import { randomUUID } from 'node:crypto'

const TURN_CHAR_LIMIT = 8000
const USER_CHAR_LIMIT = 4000
// 先脱敏再截断：先截断可能把密钥切成两半，模式就匹配不上了。超过这个长度的部分反正不会存。
const REDACTION_WINDOW_CHARS = 64_000
const TITLE_CHARS = 80
export const EPISODE_BYTES_SOFT_LIMIT = 200 * 1024 * 1024
// 清理后留出余量，避免每写一轮都要再删一次。
const PRUNE_TARGET_RATIO = 0.9
const PRUNE_BATCH = 500
// SUM 要扫表，不必每轮都算：每个 store 第一次写入时查一次，之后隔一段再查。
const PRUNE_CHECK_INTERVAL = 256
const pruneCounters = new WeakMap()

function cleanIso(value) {
  const time = Date.parse(String(value || ''))
  return Number.isNaN(time) ? '' : new Date(time).toISOString()
}

// 切在代理对中间会留下孤立的半个字符，序列化后不是合法 UTF-8。
function sliceText(text, limit) {
  if (limit <= 0) return ''
  return text.slice(0, limit).replace(/[\uD800-\uDBFF]$/, '')
}

function turnText(store, value, limit) {
  return sliceText(store.safeText(value, REDACTION_WINDOW_CHARS), limit)
}

function atomically(db, run) {
  if (db.isTransaction) return run()
  db.exec('BEGIN IMMEDIATE')
  try {
    const result = run()
    db.exec('COMMIT')
    return result
  } catch (error) {
    db.exec('ROLLBACK')
    throw error
  }
}

// 项目空间要先存在才能写（外键）。这里是同步路径，没法等 ensureWorkspaceSpace，由调用方提前建好。
function resolveSpaceId(store, { spaceId, cwd }) {
  if (spaceId) return String(spaceId)
  if (!cwd) return ''
  const projectId = store.workspaceSpaceId(cwd)
  return store.db.prepare('SELECT 1 FROM memory_spaces WHERE id = ?').get(projectId)
    ? projectId
    : ''
}

function rowEpisode(row) {
  return {
    id: row.id,
    spaceId: row.space_id,
    sessionId: row.session_id,
    entryId: row.entry_id || '',
    cwd: row.cwd || '',
    user: row.user_text,
    assistant: row.assistant_text,
    at: row.at,
  }
}

/**
 * 写一轮原文。同一个 id 再调一次只更新 captured：即时抽取成功后用它标记这一轮已抽过。
 * @param {object} store
 * @param {{ id?: string, sessionId: string, cwd?: string, spaceId?: string, entryId?: string,
 *   user?: string, assistant?: string, at?: string, captured?: boolean }} input
 * @returns {{ id: string } | null} 没有内容或空间不存在时返回 null
 */
export function recordEpisode(store, input) {
  const { db } = store
  if (input.id && db.prepare('SELECT 1 FROM memory_episodes WHERE id = ?').get(input.id)) {
    db.prepare('UPDATE memory_episodes SET captured = ? WHERE id = ?').run(
      input.captured ? 1 : 0,
      input.id,
    )
    return { id: input.id }
  }
  const sessionId = store.cleanText(input.sessionId, 100)
  const spaceId = resolveSpaceId(store, input)
  if (!sessionId || !spaceId) return null
  const user = turnText(store, input.user, USER_CHAR_LIMIT)
  const assistant = turnText(store, input.assistant, TURN_CHAR_LIMIT - user.length)
  if (!user && !assistant) return null
  const id = randomUUID()
  const at = cleanIso(input.at) || store.isoNow()
  atomically(db, () => {
    db.prepare(
      `INSERT INTO memory_episodes (id, space_id, session_id, entry_id, cwd, captured, bytes, at, user_text, assistant_text)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(
      id,
      spaceId,
      sessionId,
      store.safeText(input.entryId, 180),
      store.safeText(input.cwd, 1000),
      input.captured ? 1 : 0,
      Buffer.byteLength(user) + Buffer.byteLength(assistant),
      at,
      user,
      assistant,
    )
    // 回退到旧版时，它仍会用时间游标筛选。迟到的原文需要让旧版也能重新看到。
    if (!input.captured)
      db.prepare('DELETE FROM memory_session_cursors WHERE session_id = ? AND episode_at >= ?').run(
        sessionId,
        at,
      )
    store.index.upsert({
      kind: 'episode',
      id,
      spaceId,
      sessionId,
      title: sliceText(user || assistant, TITLE_CHARS),
      body: `${user}\n${assistant}`,
    })
  })
  const writes = pruneCounters.get(store) ?? PRUNE_CHECK_INTERVAL
  if (writes >= PRUNE_CHECK_INTERVAL) {
    pruneCounters.set(store, 1)
    pruneEpisodes(store)
  } else {
    pruneCounters.set(store, writes + 1)
  }
  return { id }
}

function removeEpisodes(store, ids) {
  const { db } = store
  const marks = ids.map(() => '?').join(',')
  db.prepare(`UPDATE memories SET episode_id = '' WHERE episode_id IN (${marks})`).run(...ids)
  for (const id of ids) store.index.remove(id)
  db.prepare(`DELETE FROM memory_episodes WHERE id IN (${marks})`).run(...ids)
}

// 总量超过软上限时从最旧的删起。事实保留（只去掉指向原文的指针）：原文是证据，事实才是真值。
export function pruneEpisodes(store, { maxBytes = EPISODE_BYTES_SOFT_LIMIT } = {}) {
  const { db } = store
  const total = Number(
    db.prepare('SELECT COALESCE(SUM(bytes), 0) AS total FROM memory_episodes').get()?.total || 0,
  )
  if (total <= maxBytes) return 0
  let excess = total - Math.floor(maxBytes * PRUNE_TARGET_RATIO)
  let removed = 0
  const oldest = db.prepare('SELECT id, bytes FROM memory_episodes ORDER BY at, rowid LIMIT ?')
  while (excess > 0) {
    const rows = oldest.all(PRUNE_BATCH)
    if (!rows.length) break
    const ids = []
    for (const row of rows) {
      if (excess <= 0) break
      ids.push(row.id)
      excess -= Number(row.bytes || 0)
    }
    atomically(db, () => removeEpisodes(store, ids))
    removed += ids.length
  }
  return removed
}

/**
 * 需要批量抽取的会话：有没抽过的原文，且最后一轮已经空闲够久或累计够多。
 * force 不看空闲和累计（压缩前用），sessionId 只看这一个会话。
 * @returns {Array<{ sessionId: string, cwd: string, spaceId: string, episodes: Array<object> }>}
 */
export function pendingEpisodeBatches(
  store,
  {
    now = store.now(),
    idleMs = 10 * 60 * 1000,
    minTurns = 10,
    maxTurns = 30,
    limit = 4,
    sessionId = '',
    force = false,
  } = {},
) {
  const { db } = store
  const sessions = db
    .prepare(
      `SELECT session_id, COUNT(*) AS turns, MAX(at) AS last_at
       FROM memory_episodes
       WHERE captured = 0 AND (? = '' OR session_id = ?)
       GROUP BY session_id
       ORDER BY last_at`,
    )
    .all(sessionId, sessionId)
  const idleBefore = new Date(now - idleMs).toISOString()
  const due = sessions
    .filter((row) => force || row.last_at <= idleBefore || Number(row.turns) >= minTurns)
    .slice(0, limit)
  const turns = db.prepare(
    `SELECT * FROM memory_episodes WHERE session_id = ? AND captured = 0 ORDER BY at, rowid LIMIT ?`,
  )
  return due.flatMap((row) => {
    const pending = turns.all(row.session_id, maxTurns)
    // 同一聊天中途可以切换项目。一轮模型抽取只读一个空间，其他原文仍待处理。
    const episodes = pending.filter((turn) => turn.space_id === pending[0].space_id).map(rowEpisode)
    const last = episodes.at(-1)
    return last
      ? [{ sessionId: row.session_id, cwd: last.cwd, spaceId: last.spaceId, episodes }]
      : []
  })
}

// 调用方把这一步和事实写入放在同一事务里。等待模型时任何一轮被删或已处理，
// 整批结果就作废：即使证据引用了另一轮，结果也可能转述了那段已删的原文。
export function markEpisodesCaptured(store, sessionId, episodeIds) {
  const { db } = store
  const marks = episodeIds.map(() => '?').join(',')
  const source = db
    .prepare(
      `SELECT COUNT(*) AS count, MAX(at) AS at FROM memory_episodes
      WHERE session_id = ? AND captured = 0 AND id IN (${marks})`,
    )
    .get(sessionId, ...episodeIds)
  if (source.count !== episodeIds.length) return false
  db.prepare(`UPDATE memory_episodes SET captured = 1 WHERE id IN (${marks})`).run(...episodeIds)
  // 保留旧版能安全使用的时间游标。同一毫秒的下一批原文不能被它跳过。
  db.prepare(
    `INSERT INTO memory_session_cursors (session_id, episode_at, updated_at)
     SELECT ?, ?, ? WHERE NOT EXISTS (
       SELECT 1 FROM memory_episodes WHERE session_id = ? AND captured = 0 AND at <= ?
     ) ON CONFLICT(session_id) DO UPDATE SET
       episode_at = MAX(episode_at, excluded.episode_at), updated_at = excluded.updated_at`,
  ).run(sessionId, source.at, store.isoNow(), sessionId, source.at)
  return true
}

// 会话摘要是从原文写出来的：原文清掉了，摘要不能留着接着复述。
function removeSessionDigests(store, rows) {
  const remove = store.db.prepare('DELETE FROM memory_digests WHERE id = ?')
  for (const row of rows) {
    remove.run(row.id)
    store.index.remove(row.id)
  }
}

// 事实所在会话的摘要也可能转述过这件事，而这些摘要不记事实 id：重写之前不能再被检索到。
export function hideSessionDigestsOfFacts(store, facts) {
  const hide = store.db.prepare(
    "UPDATE memory_digests SET dirty = 2 WHERE level = 'session' AND session_id = ?",
  )
  for (const sessionId of new Set(facts.map((fact) => fact.session_id).filter(Boolean)))
    hide.run(sessionId)
}

// 用户删掉一条事实时，把能确定说过这件事的原文也删掉：事实指着的那一轮，
// 以及同一会话里逐字包含它证据的轮次。一轮原文里还可能有别的话，但用户要删的内容优先。
const MIN_EVIDENCE_CHARS = 8

export function deleteEpisodesOfFacts(store, facts) {
  const { db } = store
  const ids = new Set()
  const bySession = db.prepare(
    'SELECT id FROM memory_episodes WHERE session_id = ? AND (instr(user_text, ?) > 0 OR instr(assistant_text, ?) > 0)',
  )
  for (const fact of facts) {
    if (fact.episode_id) ids.add(fact.episode_id)
    const evidence = String(fact.evidence || '')
    if (fact.session_id && evidence.length >= MIN_EVIDENCE_CHARS) {
      for (const row of bySession.all(fact.session_id, evidence, evidence)) ids.add(row.id)
    }
  }
  const list = [...ids]
  atomically(db, () => {
    for (let start = 0; start < list.length; start += PRUNE_BATCH)
      removeEpisodes(store, list.slice(start, start + PRUNE_BATCH))
  })
  return list.length
}

export function clearEpisodes(store, { spaceId = '' } = {}) {
  const { db } = store
  const list = db
    .prepare("SELECT id FROM memory_episodes WHERE ? = '' OR space_id = ?")
    .all(spaceId, spaceId)
    .map((row) => row.id)
  atomically(db, () => {
    for (let start = 0; start < list.length; start += PRUNE_BATCH)
      removeEpisodes(store, list.slice(start, start + PRUNE_BATCH))
    removeSessionDigests(
      store,
      db
        .prepare(
          "SELECT id FROM memory_digests WHERE level = 'session' AND (? = '' OR space_id = ?)",
        )
        .all(spaceId, spaceId),
    )
    if (!spaceId) db.exec('DELETE FROM memory_session_cursors')
  })
  return list.length
}

// 会话删了，它的原文、摘要和索引也要删。抽出来的事实是独立的长期记忆，保留，只去掉指针。
export function deleteSessionEpisodes(store, sessionId) {
  const { db } = store
  const ids = db
    .prepare('SELECT id FROM memory_episodes WHERE session_id = ?')
    .all(sessionId)
    .map((row) => row.id)
  atomically(db, () => {
    for (let start = 0; start < ids.length; start += PRUNE_BATCH)
      removeEpisodes(store, ids.slice(start, start + PRUNE_BATCH))
    removeSessionDigests(
      store,
      db.prepare('SELECT id FROM memory_digests WHERE session_id = ?').all(sessionId),
    )
    db.prepare('DELETE FROM memory_session_cursors WHERE session_id = ?').run(sessionId)
  })
  return ids.length
}
