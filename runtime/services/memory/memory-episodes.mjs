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
  const projectId = store.spaceIdsForCwd(cwd).at(-1)
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
      cleanIso(input.at) || store.isoNow(),
      user,
      assistant,
    )
    store.index.upsert({
      kind: 'episode',
      id,
      spaceId,
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
 * 需要批量抽取的会话：游标之后有没抽过的原文，且最后一轮已经空闲够久或累计够多。
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
      `SELECT episodes.session_id AS session_id, COUNT(*) AS turns, MAX(episodes.at) AS last_at
       FROM memory_episodes episodes
       LEFT JOIN memory_session_cursors cursors ON cursors.session_id = episodes.session_id
       WHERE episodes.captured = 0
         AND (cursors.episode_at IS NULL OR episodes.at > cursors.episode_at)
         AND (? = '' OR episodes.session_id = ?)
       GROUP BY episodes.session_id
       ORDER BY last_at`,
    )
    .all(sessionId, sessionId)
  const idleBefore = new Date(now - idleMs).toISOString()
  const due = sessions
    .filter((row) => force || row.last_at <= idleBefore || Number(row.turns) >= minTurns)
    .slice(0, limit)
  const turns = db.prepare(
    `SELECT episodes.* FROM memory_episodes episodes
     LEFT JOIN memory_session_cursors cursors ON cursors.session_id = episodes.session_id
     WHERE episodes.session_id = ? AND episodes.captured = 0
       AND (cursors.episode_at IS NULL OR episodes.at > cursors.episode_at)
     ORDER BY episodes.at, episodes.rowid LIMIT ?`,
  )
  return due.map((row) => {
    const episodes = turns.all(row.session_id, maxTurns).map(rowEpisode)
    const last = episodes.at(-1)
    return { sessionId: row.session_id, cwd: last.cwd, spaceId: last.spaceId, episodes }
  })
}

// 游标只进不退；走过的原文标记为已抽取，待抽取的查询因此只需看 captured = 0 的那一小部分。
export function saveSessionCursor(store, sessionId, episodeAt) {
  const at = cleanIso(episodeAt)
  if (!sessionId || !at) return
  const { db } = store
  atomically(db, () => {
    db.prepare(
      `INSERT INTO memory_session_cursors (session_id, episode_at, updated_at) VALUES (?, ?, ?)
       ON CONFLICT(session_id) DO UPDATE SET
         episode_at = MAX(episode_at, excluded.episode_at),
         updated_at = excluded.updated_at`,
    ).run(sessionId, at, store.isoNow())
    db.prepare(
      'UPDATE memory_episodes SET captured = 1 WHERE session_id = ? AND captured = 0 AND at <= ?',
    ).run(sessionId, at)
  })
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
    if (!spaceId) db.exec('DELETE FROM memory_session_cursors')
  })
  return list.length
}

// 会话删了，它的原文和索引也要删。抽出来的事实是独立的长期记忆，保留，只去掉指针。
export function deleteSessionEpisodes(store, sessionId) {
  const { db } = store
  const ids = db
    .prepare('SELECT id FROM memory_episodes WHERE session_id = ?')
    .all(sessionId)
    .map((row) => row.id)
  atomically(db, () => {
    for (let start = 0; start < ids.length; start += PRUNE_BATCH)
      removeEpisodes(store, ids.slice(start, start + PRUNE_BATCH))
    db.prepare('DELETE FROM memory_session_cursors WHERE session_id = ?').run(sessionId)
  })
  return ids.length
}
