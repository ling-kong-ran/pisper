// 本地记忆运行时：长期记忆的门面，数据都在一个 SQLite 文件里（node:sqlite）。
// 四层：L0 原文（episodes）、L1 事实（memories，唯一的真值单元）、L2 时间摘要（digests）、L3 画像（profiles）。
// 写入不需要确认：事实直接生效，每次判定都写 memory_events，事后能看、能撤销。
// 空间（global/project/custom/bot）隔离由调用方传入的 spaceIds 决定，模型参数碰不到。
import { createHash, createHmac, randomUUID } from 'node:crypto'
import { realpathSync } from 'node:fs'
import { basename, resolve } from 'node:path'
import { platform } from 'node:os'
import { redactSecretText } from '../../security/secret-redaction.mjs'
import { extractEntities, isInformativeQuery } from './memory-tokenizer.mjs'
import {
  initializeMemorySchema,
  MEMORY_SCHEMA_REVISION,
  readMemoryMeta,
  writeMemoryMeta,
} from './memory-schema.mjs'
import { initializeMemoryFullTextSearch, MemorySearchIndex } from './memory-index.mjs'
import {
  factTimestamp,
  formatMemoryContext,
  passesRecallGate,
  retrieve,
} from './memory-retrieval.mjs'
import { reconcilePendingMemories } from './memory-reconcile.mjs'
import {
  authorityTier,
  checkMemoryAnchors,
  computeAnchorHash,
  confirmMemory,
  memorySnapshot,
  restoreMemory,
  sweepMemoryLifecycle,
  undoMemoryEvent,
} from './memory-lifecycle.mjs'
import {
  clearEpisodes,
  deleteEpisodesOfFacts,
  deleteSessionEpisodes,
  pendingEpisodeBatches,
  recordEpisode,
  saveSessionCursor,
} from './memory-episodes.mjs'
import {
  loadProfileContext,
  openMemoryDocument,
  runMemoryConsolidation,
} from './memory-consolidator.mjs'

let DatabaseSync
try {
  ;({ DatabaseSync } = await import('node:sqlite'))
} catch {}

const MEMORY_TYPES = new Set(['concept', 'file', 'risk', 'preference', 'decision', 'fact', 'task'])
const MEMORY_SCOPES = new Set(['global', 'project', 'custom'])
// 来源决定权威度。用户自己说的（手动、明确要求、对话里抽到的用户原话）是最高一档，
// 模型的推断和工具结果次之，外部渠道最低。低一档不能替换高一档的事实，见 authorityTier。
const SOURCE_AUTHORITIES = {
  manual: 100,
  user_confirmed: 100,
  conversation_confirmed: 100,
  auto_approved: 90,
  conversation: 90,
  tool_verified: 80,
  agent: 40,
  external: 20,
}
const BROAD_TOPIC_SEGMENTS = new Set([
  'architecture',
  'config',
  'configuration',
  'general',
  'memory',
  'project',
  'settings',
  'user',
])
const SEMANTIC_BATCH_SIZE = 16
const TOMBSTONE_RETENTION_DAYS = 90
const FORGOTTEN_RETENTION_DAYS = 365
const LIFECYCLE_SWEEP_INTERVAL_MS = 60 * 60 * 1000
const BACKGROUND_INTERVAL_MS = 2 * 60 * 1000

function cleanText(value, maxLength) {
  return String(value || '')
    .replaceAll(String.fromCharCode(0), '')
    .trim()
    .slice(0, maxLength)
}

function safeText(value, maxLength) {
  return redactSecretText(cleanText(value, maxLength))
}

function normalizeKey(value, maxLength = 180) {
  return cleanText(value, maxLength)
    .normalize('NFKC')
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, '.')
    .replace(/^\.+|\.+$/g, '')
}

function topicIdentity(topic, title) {
  const topicKey = normalizeKey(topic)
  const titleKey = normalizeKey(title, 140)
  if (!topicKey) return `title.${titleKey}`
  const segments = topicKey.split('.')
  const broad = segments.length < 2 || BROAD_TOPIC_SEGMENTS.has(segments.at(-1))
  return broad ? `${topicKey}.${titleKey}` : topicKey
}

function dateAfterDays(now, days) {
  return new Date(now + days * 86_400_000).toISOString()
}

function canonicalProjectPath(cwd) {
  const absolute = resolve(String(cwd || '.'))
  let canonical = absolute
  try {
    canonical = realpathSync.native(absolute)
  } catch {}
  canonical = canonical.replaceAll('\\', '/')
  return platform() === 'win32' ? canonical.toLowerCase() : canonical
}

function stableProjectId(cwd) {
  return `project-${createHash('sha256').update(canonicalProjectPath(cwd)).digest('hex').slice(0, 24)}`
}

export function botSpaceId(botId) {
  return `bot:${botId}`
}

export function rowMemory(row) {
  if (!row) return null
  return {
    id: row.id,
    spaceId: row.space_id,
    title: row.title,
    content: row.content,
    type: row.type,
    sourceType: row.source_type,
    sourceId: row.source_id || '',
    sourcePath: row.source_path || '',
    sessionId: row.session_id || '',
    cwd: row.cwd || '',
    evidence: row.evidence || '',
    evidenceRole: row.evidence_role || 'manual',
    episodeId: row.episode_id || '',
    sourceTimestamp: row.source_timestamp || '',
    importance: Number(row.importance || 0),
    authority: Number(row.authority || 0),
    accessCount: Number(row.access_count || 0),
    lastAccessedAt: row.last_accessed_at || '',
    topicKey: row.topic_key || '',
    identityKey: row.identity_key || '',
    status: row.status || 'active',
    revision: Number(row.revision || 1),
    supersededBy: row.superseded_by || '',
    supersededAt: row.superseded_at || '',
    verifiedAt: row.verified_at || '',
    expiresAt: row.expires_at || '',
    validFrom: row.valid_from || row.created_at,
    validTo: row.valid_to || '',
    lastConfirmedAt: row.last_confirmed_at || '',
    confirmCount: Number(row.confirm_count || 1),
    pinned: Boolean(row.pinned),
    tainted: Boolean(row.tainted),
    archivedAt: row.archived_at || '',
    archiveReason: row.archive_reason || '',
    semanticText: row.semantic_text || '',
    semanticStatus: row.semantic_status || 'pending',
    createdAt: row.created_at,
    updatedAt: row.updated_at,
  }
}

function rowSpace(row) {
  return {
    id: row.id,
    name: row.name,
    kind: row.kind,
    rootPath: row.root_path || '',
    nodeCount: Number(row.node_count || 0),
    candidateCount: 0,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
  }
}

// 检索结果里不同层的统一形状：事实保持 rowMemory，摘要和原文补上 kind 相关字段。
function itemFromHit(hit) {
  const base = { kind: hit.kind, score: hit.score, relevance: hit.coverage }
  if (hit.kind === 'fact') return { ...rowMemory(hit.row), ...base }
  if (hit.kind === 'digest')
    return {
      ...base,
      id: hit.row.id,
      spaceId: hit.row.space_id,
      level: hit.row.level,
      windowStart: hit.row.window_start,
      windowEnd: hit.row.window_end,
      title: `${hit.row.level} ${String(hit.row.window_start).slice(0, 10)}`,
      content: hit.row.text,
    }
  return {
    ...base,
    id: hit.row.id,
    spaceId: hit.row.space_id,
    sessionId: hit.row.session_id,
    at: hit.row.at,
    title: String(hit.row.user_text).slice(0, 80),
    content: `${hit.row.user_text}\n---\n${hit.row.assistant_text}`,
  }
}

export class LocalMemoryRuntime {
  constructor({
    path,
    cwd,
    fullTextSearchInitializer = initializeMemoryFullTextSearch,
    semanticSummarizer = null,
    modelClient = null,
    now = () => Date.now(),
  } = {}) {
    this.path = path
    this.cwd = resolve(cwd)
    this.db = null
    this.index = null
    this.store = null
    this.ftsAvailable = false
    this.fullTextSearchInitializer = fullTextSearchInitializer
    this.semanticSummarizer = semanticSummarizer
    this.modelClient = modelClient
    this.now = now
    this.semanticQueue = new Set()
    this.semanticPromise = null
    this.semanticGeneration = 0
    this.semanticLastError = ''
    this.lastLifecycleSweep = 0
    this.transactionDepth = 0
    this.backgroundTimer = null
    this.backgroundController = null
    this.backgroundPromise = null
    this.backgroundSteps = []
    this.getExcludedSpaceIds = null
  }

  async init() {
    // 移动 Node 可能没有 node:sqlite；由上层能力清单跳过记忆初始化。
    if (!DatabaseSync) throw new Error('当前 Runtime 不支持本地 SQLite 记忆。')
    this.db = new DatabaseSync(this.path)
    this.db.exec('PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;')
    initializeMemorySchema(this.db, {
      path: this.path,
      now: this.isoNow(),
      onBackupError: () => console.warn('[memory] 升级前备份失败，继续升级。'),
    })
    this.ftsAvailable = this.fullTextSearchInitializer(this.db)
    this.index = new MemorySearchIndex(this.db, { ftsAvailable: this.ftsAvailable })
    this.store = this.createStore()
    this.catchUpIndex()
    await this.ensureGlobalSpace()
    await this.ensureWorkspaceSpace(this.cwd)
    this.cleanupRetention({ force: true })
    if (this.semanticSummarizer)
      this.scheduleSemantic(
        this.requireDb()
          .prepare(
            "SELECT id FROM memories WHERE status IN ('active', 'stale') AND semantic_status IN ('pending', 'error')",
          )
          .all()
          .map((row) => row.id),
      )
  }

  // 给各子模块的窄接口：它们只碰这些，不拿整个运行时。
  createStore() {
    return {
      db: this.db,
      index: this.index,
      now: () => this.now(),
      isoNow: () => this.isoNow(),
      getMemory: (id) => this.getMemory(id),
      indexFact: (id) => this.indexFact(id),
      recordEvent: (event) => this.recordEvent(event),
      transaction: (run) => this.transaction(run),
      markDerivedDirty: (ids, options) => this.markDerivedDirty(ids, options),
      scheduleSemantic: (ids) => this.scheduleSemantic(ids),
      retrieve: (options) => retrieve({ db: this.db, index: this.index }, options),
      spaceIdsForCwd: (cwd) => this.resolveSpaceIds({ cwd }),
      ensureWorkspaceSpace: (cwd) => this.ensureWorkspaceSpace(cwd),
      safeText,
      cleanText,
      modelClient: () => this.modelClient,
    }
  }

  isoNow() {
    return new Date(this.now()).toISOString()
  }

  // 索引模式变了（换了不带 FTS5 的 Node）或旧版本写入过新行时，把差的部分补上。
  catchUpIndex() {
    const db = this.requireDb()
    const mode = this.ftsAvailable ? 'fts' : 'scan'
    const previous = readMemoryMeta(db, 'index_mode')
    if (previous && previous !== mode) {
      db.exec('DELETE FROM memory_docs')
      if (this.ftsAvailable) db.exec('DELETE FROM memory_search')
      db.exec('UPDATE memories SET indexed_at = NULL')
      this.reindexDerivedDocuments()
    }
    const rows = db
      .prepare(`SELECT id FROM memories WHERE indexed_at IS NULL OR updated_at > indexed_at`)
      .all()
    if (rows.length) {
      db.exec('BEGIN IMMEDIATE')
      try {
        for (const row of rows) this.indexFact(row.id)
        db.exec('COMMIT')
      } catch (error) {
        db.exec('ROLLBACK')
        throw error
      }
    }
    writeMemoryMeta(db, 'index_mode', mode)
  }

  // 原文和摘要没有 indexed_at：只在索引被清空后整体重建一次。
  reindexDerivedDocuments() {
    const db = this.requireDb()
    this.transaction(() => {
      for (const row of db.prepare('SELECT * FROM memory_episodes').all()) {
        this.index.upsert({
          kind: 'episode',
          id: row.id,
          spaceId: row.space_id,
          title: String(row.user_text || row.assistant_text).slice(0, 80),
          body: `${row.user_text}\n${row.assistant_text}`,
        })
      }
      for (const row of db.prepare('SELECT * FROM memory_digests WHERE dirty < 2').all()) {
        this.index.upsert({
          kind: 'digest',
          id: row.id,
          spaceId: row.space_id,
          title: row.level,
          body: row.text,
        })
      }
    })
  }

  indexFact(id) {
    const db = this.requireDb()
    const row = db.prepare('SELECT * FROM memories WHERE id = ?').get(id)
    if (!row) {
      this.index.remove(id)
      return
    }
    this.index.upsert({
      kind: 'fact',
      id: row.id,
      spaceId: row.space_id,
      title: row.title,
      body: row.content,
      expansion: row.semantic_text || '',
    })
    db.prepare('DELETE FROM memory_entities WHERE memory_id = ?').run(id)
    const insert = db.prepare(
      'INSERT OR IGNORE INTO memory_entities (memory_id, entity) VALUES (?, ?)',
    )
    for (const entity of extractEntities(`${row.title}\n${row.content}\n${row.source_path || ''}`))
      insert.run(id, entity)
    db.prepare('UPDATE memories SET indexed_at = ? WHERE id = ?').run(
      row.updated_at > this.isoNow() ? row.updated_at : this.isoNow(),
      id,
    )
  }

  async dispose() {
    this.stopBackground()
    if (this.backgroundPromise) await this.backgroundPromise.catch(() => {})
    this.semanticGeneration += 1
    this.semanticQueue.clear()
    this.semanticSummarizer = null
    try {
      if (this.semanticPromise) await this.semanticPromise
    } finally {
      this.db?.close()
      this.db = null
      this.index = null
      this.store = null
      this.ftsAvailable = false
    }
  }

  requireDb() {
    if (!this.db) throw new Error('记忆 Runtime 尚未初始化。')
    return this.db
  }

  requireStore() {
    this.requireDb()
    return this.store
  }

  // 可重入：外层已经开了事务时直接跑，由外层统一提交或回滚。
  transaction(run) {
    const db = this.requireDb()
    if (this.transactionDepth) return run(db)
    db.exec('BEGIN IMMEDIATE')
    this.transactionDepth = 1
    try {
      const result = run(db)
      db.exec('COMMIT')
      return result
    } catch (error) {
      db.exec('ROLLBACK')
      throw error
    } finally {
      this.transactionDepth = 0
    }
  }

  async ensureGlobalSpace() {
    const db = this.requireDb()
    if (db.prepare('SELECT id FROM memory_spaces WHERE id = ?').get('global')) return 'global'
    const now = this.isoNow()
    db.prepare(
      'INSERT INTO memory_spaces (id, name, kind, root_path, root_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
    ).run('global', '全局', 'global', '', '', now, now)
    return 'global'
  }

  async ensureWorkspaceSpace(cwd) {
    const rootPath = resolve(cwd || this.cwd)
    const rootKey = canonicalProjectPath(rootPath)
    const db = this.requireDb()
    const existing = db.prepare('SELECT id FROM memory_spaces WHERE root_key = ?').get(rootKey)
    if (existing) return existing.id
    const id = stableProjectId(rootPath)
    const collision = db.prepare('SELECT root_key FROM memory_spaces WHERE id = ?').get(id)
    if (collision && collision.root_key !== rootKey)
      throw new Error('项目记忆空间标识发生碰撞，请更换工作目录。')
    const now = this.isoNow()
    db.prepare(
      'INSERT INTO memory_spaces (id, name, kind, root_path, root_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
    ).run(id, basename(rootPath) || rootPath, 'project', rootPath, rootKey, now, now)
    return id
  }

  // Bot 的记忆是一个独立空间，只由运行时按 botForSession 决定谁能访问，不出现在记忆页。
  ensureBotSpace(botId, name = '') {
    const id = botSpaceId(botId)
    const db = this.requireDb()
    if (db.prepare('SELECT id FROM memory_spaces WHERE id = ?').get(id)) return id
    const now = this.isoNow()
    db.prepare(
      'INSERT INTO memory_spaces (id, name, kind, root_path, root_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
    ).run(id, cleanText(name, 80) || id, 'bot', '', '', now, now)
    return id
  }

  listSpaces({ includeBots = false } = {}) {
    return this.requireDb()
      .prepare(
        `
      SELECT spaces.*,
        COUNT(DISTINCT CASE WHEN memories.status IN ('active', 'stale') THEN memories.id END) AS node_count
      FROM memory_spaces spaces
      LEFT JOIN memories ON memories.space_id = spaces.id
      ${includeBots ? '' : "WHERE spaces.kind <> 'bot'"}
      GROUP BY spaces.id
      ORDER BY CASE spaces.kind WHEN 'project' THEN 0 WHEN 'global' THEN 1 ELSE 2 END, spaces.updated_at DESC
    `,
      )
      .all()
      .map(rowSpace)
  }

  createSpace(input = {}) {
    const name = safeText(input.name, 80)
    if (!name) throw new Error('记忆空间名称不能为空。')
    const kind = MEMORY_SCOPES.has(input.kind) ? input.kind : 'custom'
    const rootPath = kind === 'project' && input.rootPath ? resolve(String(input.rootPath)) : ''
    const rootKey = rootPath ? canonicalProjectPath(rootPath) : ''
    const id = rootPath ? stableProjectId(rootPath) : randomUUID()
    const now = this.isoNow()
    try {
      this.requireDb()
        .prepare(
          'INSERT INTO memory_spaces (id, name, kind, root_path, root_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
        )
        .run(id, name, kind, rootPath, rootKey, now, now)
    } catch (error) {
      if (String(error).includes('UNIQUE')) throw new Error('该工作目录已经有记忆空间。')
      throw error
    }
    return this.getSpace(id)
  }

  getSpace(id) {
    const row = this.requireDb()
      .prepare(
        `
      SELECT spaces.*,
        COUNT(DISTINCT CASE WHEN memories.status IN ('active', 'stale') THEN memories.id END) AS node_count
      FROM memory_spaces spaces
      LEFT JOIN memories ON memories.space_id = spaces.id
      WHERE spaces.id = ? GROUP BY spaces.id
    `,
      )
      .get(id)
    return row ? rowSpace(row) : null
  }

  updateSpace(id, input = {}) {
    const current = this.getSpace(id)
    if (!current) return null
    const name = safeText(input.name ?? current.name, 80)
    if (!name) throw new Error('记忆空间名称不能为空。')
    this.requireDb()
      .prepare('UPDATE memory_spaces SET name = ?, updated_at = ? WHERE id = ?')
      .run(name, this.isoNow(), id)
    return this.getSpace(id)
  }

  deleteSpace(id) {
    const space = this.getSpace(id)
    if (!space) return false
    if (space.kind === 'global') throw new Error('全局记忆空间不能删除。')
    return this.transaction((db) => {
      this.index.removeSpace(id)
      db.prepare('DELETE FROM memory_events WHERE space_id = ?').run(id)
      return db.prepare('DELETE FROM memory_spaces WHERE id = ?').run(id).changes > 0
    })
  }

  getMemory(id) {
    return rowMemory(this.requireDb().prepare('SELECT * FROM memories WHERE id = ?').get(id))
  }

  // 时间线：按“事实何时为真”从新到旧。before 是上一页最后一条的时间，用来翻页。
  listMemories({ spaceId, query = '', limit = 100, status = 'current', before = '' } = {}) {
    const safeLimit = Math.min(300, Math.max(1, Number(limit) || 100))
    const statuses = statusFilter(status)
    if (String(query).trim()) {
      return this.search(query, {
        spaceIds: [spaceId],
        limit: safeLimit,
        minScore: 0.04,
        trackAccess: false,
        statuses,
      })
    }
    return this.requireDb()
      .prepare(
        `SELECT * FROM memories
         WHERE space_id = ? AND status IN (${statuses.map(() => '?').join(',')})
           AND (? = '' OR COALESCE(valid_from, created_at) < ?)
         ORDER BY COALESCE(valid_from, created_at) DESC, id DESC LIMIT ?`,
      )
      .all(spaceId, ...statuses, before, before, safeLimit)
      .map(rowMemory)
  }

  getDashboard({ spaceId = '', query = '', status = 'current', before = '', limit = 100 } = {}) {
    this.cleanupRetention()
    const spaces = this.listSpaces()
    const selectedSpaceId = spaces.some((space) => space.id === spaceId)
      ? spaceId
      : spaces[0]?.id || ''
    const nodes = selectedSpaceId
      ? this.listMemories({ spaceId: selectedSpaceId, query, status, before, limit })
      : []
    return {
      spaces,
      selectedSpaceId,
      nodes,
      hasMore: !String(query).trim() && nodes.length >= Math.min(300, Number(limit) || 100),
      counts: selectedSpaceId
        ? this.statusCounts(selectedSpaceId)
        : { current: 0, archived: 0, history: 0 },
      // 下面两个字段留给旧版前端：候选箱已经取消，星图连线也不再计算。
      links: [],
      candidates: [],
      semantic: this.semanticStatus(),
    }
  }

  statusCounts(spaceId) {
    const rows = this.requireDb()
      .prepare('SELECT status, COUNT(*) AS count FROM memories WHERE space_id = ? GROUP BY status')
      .all(spaceId)
    const byStatus = Object.fromEntries(rows.map((row) => [row.status, Number(row.count)]))
    return {
      current: (byStatus.active || 0) + (byStatus.stale || 0),
      archived: byStatus.archived || 0,
      history: byStatus.superseded || 0,
    }
  }

  // 时间演算视图的数据：一次给全，前端拖动时间轴时在本地算“那一刻的状态”，不用每一帧请求。
  // 只给标题不给正文：列表可能上千条，正文在选中时按 id 取。Bot 空间不在这里出现。
  getEvolution({ spaceId = '', limit = 5000 } = {}) {
    const db = this.requireDb()
    const spaces = this.listSpaces()
    const ids =
      spaceId && spaceId !== 'all'
        ? spaces.filter((space) => space.id === spaceId).map((space) => space.id)
        : spaces.map((space) => space.id)
    const safeLimit = Math.min(5000, Math.max(1, Number(limit) || 5000))
    if (!ids.length)
      return { spaceId, now: this.isoNow(), spaces, facts: [], digests: [], truncated: false }
    const marks = ids.map(() => '?').join(',')
    const rows = db
      .prepare(
        `SELECT id, space_id, title, type, topic_key, importance, authority, status, source_type, valid_from,
                valid_to, created_at, superseded_by, superseded_at, archived_at, archive_reason, pinned, tainted,
                confirm_count, last_confirmed_at, access_count
         FROM memories WHERE space_id IN (${marks})
         ORDER BY COALESCE(valid_from, created_at) DESC LIMIT ?`,
      )
      .all(...ids, safeLimit + 1)
    const truncated = rows.length > safeLimit
    const facts = rows.slice(0, safeLimit).map((row) => ({
      id: row.id,
      spaceId: row.space_id,
      title: row.title,
      type: row.type,
      topicKey: row.topic_key || '',
      importance: Number(row.importance || 0),
      authority: Number(row.authority || 0),
      status: row.status,
      sourceType: row.source_type,
      validFrom: row.valid_from || row.created_at,
      validTo: row.valid_to || '',
      createdAt: row.created_at,
      supersededBy: row.superseded_by || '',
      supersededAt: row.superseded_at || '',
      archivedAt: row.archived_at || '',
      archiveReason: row.archive_reason || '',
      pinned: Boolean(row.pinned),
      tainted: Boolean(row.tainted),
      confirmCount: Number(row.confirm_count || 1),
      lastConfirmedAt: row.last_confirmed_at || '',
      accessCount: Number(row.access_count || 0),
    }))
    const digests = db
      .prepare(
        `SELECT id, space_id, level, window_start, window_end FROM memory_digests
         WHERE space_id IN (${marks}) AND dirty < 2 ORDER BY window_start`,
      )
      .all(...ids)
      .map((row) => ({
        id: row.id,
        spaceId: row.space_id,
        level: row.level,
        windowStart: row.window_start,
        windowEnd: row.window_end,
      }))
    return { spaceId: spaceId || 'all', now: this.isoNow(), spaces, facts, digests, truncated }
  }

  // 一条事实的版本链：从最早到最新，包括它自己。
  getMemoryHistory(id) {
    const db = this.requireDb()
    const self = db.prepare('SELECT * FROM memories WHERE id = ?').get(id)
    if (!self) return null
    const seen = new Set([self.id])
    const older = []
    const frontier = [self.id]
    const previous = db.prepare('SELECT * FROM memories WHERE superseded_by = ?')
    while (frontier.length && older.length < 50) {
      for (const row of previous.all(frontier.pop())) {
        if (seen.has(row.id)) continue
        seen.add(row.id)
        older.push(row)
        frontier.push(row.id)
      }
    }
    const newer = []
    let cursor = self.superseded_by
    while (cursor && !seen.has(cursor) && newer.length < 50) {
      const row = db.prepare('SELECT * FROM memories WHERE id = ?').get(cursor)
      if (!row) break
      seen.add(row.id)
      newer.push(row)
      cursor = row.superseded_by
    }
    const chain = [...older, self, ...newer].sort((left, right) =>
      String(factTimestamp(left)).localeCompare(String(factTimestamp(right))),
    )
    return { id, versions: chain.map(rowMemory) }
  }

  listEvents({ spaceId = '', limit = 50, before = '' } = {}) {
    const safeLimit = Math.min(200, Math.max(1, Number(limit) || 50))
    return this.requireDb()
      .prepare(
        `SELECT id, at, space_id, actor, action, memory_ids, reason, model, undone_at,
                before_json <> '' AS undoable
         FROM memory_events
         WHERE (? = '' OR space_id = ?) AND (? = '' OR at < ?)
         ORDER BY at DESC, id DESC LIMIT ?`,
      )
      .all(spaceId, spaceId, before, before, safeLimit)
      .map((row) => ({
        id: row.id,
        at: row.at,
        spaceId: row.space_id,
        actor: row.actor,
        action: row.action,
        memoryIds: JSON.parse(row.memory_ids || '[]'),
        reason: row.reason,
        model: row.model,
        undoneAt: row.undone_at || '',
        undoable: Boolean(row.undoable) && !row.undone_at,
      }))
  }

  recordEvent({
    actor = 'system',
    action,
    memoryIds = [],
    spaceId = '',
    before = null,
    reason = '',
    model = '',
  }) {
    const id = randomUUID()
    this.requireDb()
      .prepare(
        `INSERT INTO memory_events (id, at, space_id, actor, action, memory_ids, before_json, reason, model)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(
        id,
        this.isoNow(),
        spaceId,
        actor,
        action,
        JSON.stringify(memoryIds),
        before ? JSON.stringify(before) : '',
        cleanText(reason, 500),
        cleanText(model, 120),
      )
    return id
  }

  // 摘要和画像是事实派生出来的。来源被用户删除时 purged=true：重算之前不能再被检索到。
  markDerivedDirty(ids, { spaceId = '', purged = false } = {}) {
    const db = this.requireDb()
    const level = purged ? 2 : 1
    const update = db.prepare(
      'UPDATE memory_digests SET dirty = MAX(dirty, ?) WHERE dirty < ? AND source_ids LIKE ?',
    )
    for (const id of ids) update.run(level, level, `%"${id}"%`)
    if (spaceId) db.prepare('UPDATE memory_profiles SET dirty = 1 WHERE space_id = ?').run(spaceId)
  }

  forgetFingerprint(spaceId, content) {
    const salt = readMemoryMeta(this.requireDb(), 'forget_salt') || ''
    return createHmac('sha256', salt)
      .update(`${spaceId}\0${normalizeKey(content, 12_000)}`)
      .digest('hex')
  }

  /**
   * 写入一条事实。不需要确认，也不会抛回“待审核”：结果写在返回值的 outcome 里。
   * added 新增 | superseded 新增并替换了旧事实 | reinforced 已有相同内容，只刷新证实时间
   * archived_conflict 和更高权威的事实矛盾，进归档可找回 | forgotten 用户删过同样内容，丢弃
   */
  remember(input = {}) {
    const title = safeText(input.title, 140)
    const content = safeText(input.content, 12_000)
    if (!title || !content) throw new Error('记忆的标题和内容不能为空。')
    const spaceId = String(input.spaceId || '')
    if (!this.getSpace(spaceId)) throw new Error('记忆空间不存在。')
    const db = this.requireDb()
    const fingerprint = this.forgetFingerprint(spaceId, content)
    // 用户亲手再加一次，说明又想记住了：解除指纹。自动抽取到的一律丢弃。
    if (input.ignoreForgotten === true)
      db.prepare('DELETE FROM memory_forgotten WHERE fingerprint = ?').run(fingerprint)
    else if (db.prepare('SELECT 1 FROM memory_forgotten WHERE fingerprint = ?').get(fingerprint))
      return { id: '', spaceId, title, content, status: 'forgotten', outcome: 'forgotten' }
    const type = MEMORY_TYPES.has(input.type) ? input.type : 'concept'
    const sourceType = cleanText(input.sourceType || 'manual', 40)
    const tainted = Boolean(input.tainted) || sourceType === 'external'
    const evidenceRole = cleanText(input.evidenceRole, 20) || defaultEvidenceRole(sourceType)
    // 对话抽取里，证据只来自助手的话时不能算用户说的。
    let authority = SOURCE_AUTHORITIES[sourceType] ?? SOURCE_AUTHORITIES.agent
    if (sourceType === 'conversation' && evidenceRole !== 'user')
      authority = SOURCE_AUTHORITIES.agent
    if (tainted) authority = Math.min(authority, SOURCE_AUTHORITIES.external)
    const topicKey = normalizeKey(input.topicKey || input.topic || title)
    const identityKey = topicIdentity(input.topicKey || input.topic, title)
    const now = this.isoNow()
    const normalizedContent = normalizeKey(content, 12_000)
    const sameFact = db
      .prepare(
        "SELECT * FROM memories WHERE space_id = ? AND status IN ('active', 'stale') AND identity_key = ?",
      )
      .all(spaceId, identityKey)
    // 内容相同就只刷新证实时间：不论主题起得一不一样，都算同一件事。
    const duplicate =
      sameFact.find((row) => normalizeKey(row.content, 12_000) === normalizedContent) ||
      this.findSameContent(spaceId, normalizedContent)
    if (duplicate && input.dedupe !== false) {
      // 用户亲口重申模型猜过的内容：来源和证据升级成用户的。
      const stronger = authority > Number(duplicate.authority || 0)
      db.prepare(
        `UPDATE memories SET confirm_count = confirm_count + 1, last_confirmed_at = ?,
           status = CASE WHEN status = 'stale' THEN 'active' ELSE status END,
           source_type = ?, authority = ?, evidence = ?, evidence_role = ?, tainted = ?
         WHERE id = ?`,
      ).run(
        now,
        stronger ? sourceType : duplicate.source_type,
        stronger ? authority : duplicate.authority,
        stronger ? safeText(input.evidence, 2000) || duplicate.evidence : duplicate.evidence,
        stronger ? evidenceRole : duplicate.evidence_role,
        stronger ? (tainted ? 1 : 0) : duplicate.tainted,
        duplicate.id,
      )
      return { ...this.getMemory(duplicate.id), outcome: 'reinforced' }
    }
    const blockedBy = sameFact.find(
      (row) => authorityTier(Number(row.authority || 0)) > authorityTier(authority),
    )
    const validFrom = cleanIso(input.validFrom) || cleanIso(input.sourceTimestamp) || now
    const id = cleanText(input.id, 180) || randomUUID()
    const superseded = blockedBy ? [] : sameFact
    this.transaction(() => {
      db.prepare(
        `
        INSERT INTO memories (id, space_id, title, content, type, topic_key, identity_key, source_type, source_id, source_path,
          session_id, cwd, evidence, source_timestamp, importance, authority, status, revision, verified_at, expires_at,
          semantic_text, semantic_status, valid_from, last_confirmed_at, confirm_count, pinned, evidence_role, tainted,
          episode_id, anchor_hash, archived_at, archive_reason, reconcile_status, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, '', 'pending', ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `,
      ).run(
        id,
        spaceId,
        title,
        content,
        type,
        topicKey,
        identityKey,
        sourceType,
        safeText(input.sourceId, 180),
        safeText(input.sourcePath, 1000),
        safeText(input.sessionId, 100),
        safeText(input.cwd, 1000),
        safeText(input.evidence, 2000),
        safeText(input.sourceTimestamp, 80),
        Math.min(1, Math.max(0, Number(input.importance ?? 0.5))),
        authority,
        blockedBy ? 'archived' : 'active',
        now,
        cleanIso(input.expiresAt) || null,
        validFrom,
        now,
        input.pinned ? 1 : 0,
        evidenceRole,
        tainted ? 1 : 0,
        cleanText(input.episodeId, 180),
        computeAnchorHash(input.sourcePath, input.cwd),
        blockedBy ? now : null,
        blockedBy ? 'lower_authority_conflict' : '',
        // 已归档的、被污染的不参与对账：它们不能影响现有事实。
        blockedBy || tainted ? 'done' : 'pending',
        now,
        now,
      )
      const supersede = db.prepare(
        "UPDATE memories SET status = 'superseded', superseded_by = ?, superseded_at = ?, valid_to = ?, updated_at = ? WHERE id = ?",
      )
      for (const row of superseded) supersede.run(id, now, validFrom, now, row.id)
      db.prepare('UPDATE memory_spaces SET updated_at = ? WHERE id = ?').run(now, spaceId)
      this.indexFact(id)
      for (const row of superseded) this.indexFact(row.id)
      this.recordEvent({
        actor: input.actor || sourceType,
        action: blockedBy ? 'archive' : superseded.length ? 'supersede' : 'add',
        memoryIds: [id, ...superseded.map((row) => row.id)],
        spaceId,
        before: superseded.length
          ? Object.fromEntries(superseded.map((row) => [row.id, memorySnapshot(row)]))
          : null,
        reason: blockedBy ? 'lower_authority_conflict' : superseded.length ? 'same_identity' : '',
      })
    })
    if (superseded.length)
      this.markDerivedDirty(
        superseded.map((row) => row.id),
        { spaceId },
      )
    else if (spaceId) this.markDerivedDirty([], { spaceId })
    if (!blockedBy) this.scheduleSemantic([id])
    return {
      ...this.getMemory(id),
      outcome: blockedBy ? 'archived_conflict' : superseded.length ? 'superseded' : 'added',
    }
  }

  findSameContent(spaceId, normalizedContent) {
    // 没有内容索引：只看标题或内容开头相近的少量候选，避免每次写入都全表扫描。
    const db = this.requireDb()
    const rows = db
      .prepare(
        "SELECT * FROM memories WHERE space_id = ? AND status IN ('active', 'stale') AND length(content) BETWEEN ? AND ? LIMIT 400",
      )
      .all(spaceId, Math.floor(normalizedContent.length * 0.5), normalizedContent.length * 2 + 8)
    return rows.find((row) => normalizeKey(row.content, 12_000) === normalizedContent) || null
  }

  updateMemory(id, input = {}) {
    const db = this.requireDb()
    const currentRow = db.prepare('SELECT * FROM memories WHERE id = ?').get(id)
    if (!currentRow) return null
    const current = rowMemory(currentRow)
    const nextSpaceId = input.spaceId || current.spaceId
    if (!this.getSpace(nextSpaceId)) throw new Error('记忆空间不存在。')
    const title = safeText(input.title ?? current.title, 140)
    const content = safeText(input.content ?? current.content, 12_000)
    if (!title || !content) throw new Error('记忆的标题和内容不能为空。')
    const type = MEMORY_TYPES.has(input.type) ? input.type : current.type
    const explicitTopic = Object.hasOwn(input, 'topicKey') || Object.hasOwn(input, 'topic')
    const topicKey = normalizeKey(
      explicitTopic ? input.topicKey || input.topic || title : current.topicKey || title,
    )
    const identityKey = explicitTopic
      ? topicIdentity(input.topicKey || input.topic, title)
      : current.identityKey || topicIdentity('', title)
    const now = this.isoNow()
    const contentChanged = title !== current.title || content !== current.content
    const sourcePath = safeText(input.sourcePath ?? current.sourcePath, 1000)
    this.transaction(() => {
      db.prepare(
        `
      UPDATE memories SET space_id = ?, title = ?, content = ?, type = ?, topic_key = ?, identity_key = ?, source_type = 'manual',
        source_path = ?, evidence = ?, source_timestamp = ?, importance = ?, authority = 100, status = 'active', revision = revision + 1,
        superseded_by = '', superseded_at = NULL, valid_to = NULL, verified_at = ?, expires_at = ?, last_confirmed_at = ?,
        archived_at = NULL, archive_reason = '', evidence_role = 'manual', tainted = 0, pinned = ?, anchor_hash = ?,
        valid_from = COALESCE(?, valid_from), semantic_text = '', semantic_status = ?, semantic_updated_at = NULL, updated_at = ?
      WHERE id = ?
    `,
      ).run(
        nextSpaceId,
        title,
        content,
        type,
        topicKey,
        identityKey,
        sourcePath,
        safeText(input.evidence ?? current.evidence, 2000),
        safeText(input.sourceTimestamp ?? current.sourceTimestamp, 80),
        Math.min(1, Math.max(0, Number(input.importance ?? current.importance))),
        now,
        cleanIso(input.expiresAt ?? current.expiresAt) || null,
        now,
        (input.pinned ?? current.pinned) ? 1 : 0,
        computeAnchorHash(sourcePath, input.cwd ?? current.cwd),
        cleanIso(input.validFrom) || null,
        contentChanged ? 'pending' : current.semanticStatus,
        now,
        id,
      )
      this.indexFact(id)
      this.recordEvent({
        actor: 'user',
        action: 'edit',
        memoryIds: [id],
        spaceId: nextSpaceId,
        before: { [id]: memorySnapshot(currentRow) },
      })
    })
    this.markDerivedDirty([id], { spaceId: nextSpaceId })
    if (contentChanged) this.scheduleSemantic([id])
    return this.getMemory(id)
  }

  setPinned(id, pinned) {
    const db = this.requireDb()
    const row = db.prepare('SELECT * FROM memories WHERE id = ?').get(id)
    if (!row) return null
    const now = this.isoNow()
    this.transaction(() => {
      db.prepare('UPDATE memories SET pinned = ?, updated_at = ? WHERE id = ?').run(
        pinned ? 1 : 0,
        now,
        id,
      )
      this.recordEvent({
        actor: 'user',
        action: pinned ? 'pin' : 'unpin',
        memoryIds: [id],
        spaceId: row.space_id,
        before: { [id]: memorySnapshot(row) },
      })
    })
    this.markDerivedDirty([], { spaceId: row.space_id })
    return this.getMemory(id)
  }

  // 用户删除：物理清除这条和它的历史版本（版本链在界面上是同一条记忆），连同证据、事件快照；
  // 留下内容指纹防止再次抽取时复活；派生的摘要在重算前不再参与检索。
  forget(id, reasonCode = 'user_deleted') {
    const db = this.requireDb()
    const row = db.prepare('SELECT * FROM memories WHERE id = ?').get(id)
    if (!row) return false
    const now = this.isoNow()
    const chain = [row]
    const frontier = [row.id]
    const previous = db.prepare('SELECT * FROM memories WHERE superseded_by = ?')
    while (frontier.length && chain.length < 200) {
      for (const older of previous.all(frontier.pop())) {
        chain.push(older)
        frontier.push(older.id)
      }
    }
    const ids = chain.map((item) => item.id)
    this.transaction(() => {
      // 原文里也有这段内容：不删的话 memory_search 还能从原文层搜到，批量抽取也可能再抽出来。
      deleteEpisodesOfFacts(this.requireStore(), chain)
      const insertForgotten = db.prepare(
        'INSERT OR REPLACE INTO memory_forgotten (fingerprint, expires_at) VALUES (?, ?)',
      )
      const forgottenUntil = dateAfterDays(this.now(), FORGOTTEN_RETENTION_DAYS)
      const scrubEvents = db.prepare('DELETE FROM memory_events WHERE memory_ids LIKE ?')
      for (const item of chain) {
        insertForgotten.run(this.forgetFingerprint(item.space_id, item.content), forgottenUntil)
        scrubEvents.run(`%"${item.id}"%`)
        this.index.remove(item.id)
        db.prepare('DELETE FROM memories WHERE id = ?').run(item.id)
        this.insertTombstone(item.id, 'memory', 'deleted', '', reasonCode, now)
      }
      db.prepare("UPDATE memories SET superseded_by = '' WHERE superseded_by = ?").run(id)
      this.recordEvent({
        actor: 'user',
        action: 'delete',
        memoryIds: [],
        spaceId: row.space_id,
        reason: reasonCode,
      })
    })
    this.markDerivedDirty(ids, { spaceId: row.space_id, purged: true })
    return true
  }

  insertTombstone(id, entityType, action, replacementId, reasonCode, now = this.isoNow()) {
    this.requireDb()
      .prepare(
        `
      INSERT OR REPLACE INTO memory_tombstones (id, entity_type, action, replacement_id, reason_code, created_at, expires_at)
      VALUES (?, ?, ?, ?, ?, ?, ?)
    `,
      )
      .run(
        id,
        entityType,
        action,
        replacementId || '',
        reasonCode || '',
        now,
        dateAfterDays(this.now(), TOMBSTONE_RETENTION_DAYS),
      )
  }

  // 到期、长期不用、超保留期：不是每次读都跑，一小时最多一次。
  cleanupRetention({ force = false } = {}) {
    const now = this.now()
    if (!force && now - this.lastLifecycleSweep < LIFECYCLE_SWEEP_INTERVAL_MS) return
    this.lastLifecycleSweep = now
    sweepMemoryLifecycle(this.requireStore())
    const db = this.requireDb()
    const iso = this.isoNow()
    db.prepare('DELETE FROM memory_tombstones WHERE expires_at <= ?').run(iso)
    db.prepare('DELETE FROM memory_forgotten WHERE expires_at <= ?').run(iso)
  }

  resolveSpaceIds({ cwd = '', spaceIds = null } = {}) {
    const ids = Array.isArray(spaceIds) ? spaceIds.filter(Boolean) : ['global']
    if (!spaceIds && cwd) ids.push(stableProjectId(cwd))
    return [...new Set(ids)]
  }

  applyScopePrecedence(hits, cwd) {
    if (!cwd) return hits
    const projectId = stableProjectId(cwd)
    const projectIdentities = new Set(
      hits
        .filter((hit) => hit.kind === 'fact' && hit.row.space_id === projectId)
        .map((hit) => hit.row.identity_key),
    )
    return hits.filter(
      (hit) =>
        hit.kind !== 'fact' ||
        hit.row.space_id !== 'global' ||
        !projectIdentities.has(hit.row.identity_key),
    )
  }

  trackAccess(items) {
    const facts = items.filter((item) => item.kind === 'fact')
    if (!facts.length) return
    const now = this.isoNow()
    const update = this.requireDb().prepare(
      'UPDATE memories SET access_count = access_count + 1, last_accessed_at = ? WHERE id = ?',
    )
    for (const item of facts) update.run(now, item.id)
  }

  /**
   * 检索。默认只查当前有效的事实；options 见 memory-retrieval.mjs 的 retrieve。
   * minScore 沿用旧名字，含义是“查询词至少命中多少比例”。
   */
  search(
    query,
    {
      cwd = '',
      spaceIds = null,
      limit = 6,
      minScore = 0.08,
      trackAccess = true,
      kinds = ['fact'],
      ...options
    } = {},
  ) {
    const text = cleanText(query, 4000)
    const ids = this.resolveSpaceIds({ cwd, spaceIds })
    if (!ids.length) return []
    const safeLimit = Math.min(30, Math.max(1, Number(limit) || 6))
    const hits = this.applyScopePrecedence(
      retrieve(
        { db: this.requireDb(), index: this.index },
        {
          ...options,
          query: text,
          spaceIds: ids,
          kinds,
          limit: safeLimit,
          minCoverage: minScore,
          now: this.now(),
        },
      ),
      cwd,
    )
    checkMemoryAnchors(this.requireStore(), hits)
    const items = hits.map(itemFromHit)
    if (trackAccess) this.trackAccess(items)
    return items
  }

  async searchRelevant(query, options = {}) {
    return this.search(query, options)
  }

  // 每轮自动回忆：不调 LLM。门槛见 passesRecallGate；被污染的内容不自动进上下文。
  // spaceIds 由运行时指定（Bot 只能看到自己的空间），不传就是全局 + 当前项目。
  async relevantContext(query, cwd, limit = 4, { excludeIds = [], spaceIds = null } = {}) {
    if (!isInformativeQuery(query)) return { text: '', memories: [] }
    if (!spaceIds) await this.ensureWorkspaceSpace(cwd)
    const excluded = new Set(excludeIds)
    const hits = this.applyScopePrecedence(
      retrieve(
        { db: this.requireDb(), index: this.index },
        {
          query: cleanText(query, 4000),
          spaceIds: this.resolveSpaceIds({ cwd, spaceIds }),
          kinds: ['fact'],
          includeTainted: false,
          limit: limit + excluded.size + 4,
          now: this.now(),
        },
      ),
      spaceIds ? '' : cwd,
    )
    const gated = hits.filter((hit) => passesRecallGate(hit) && !excluded.has(hit.row.id))
    checkMemoryAnchors(this.requireStore(), gated)
    const candidates = gated
      .slice(0, limit)
      .map((hit) => ({ ...rowMemory(hit.row), scopeLabel: scopeLabelFor(hit.row.space_id) }))
    const context = formatMemoryContext(candidates, { now: this.now() })
    if (context.memories.length) this.trackAccess(context.memories.map(asFact))
    return context
  }

  // L3 画像：常驻系统提示。只在会话开始和压缩后读一次，保持提示缓存稳定。
  profileContext(spaceIds) {
    return loadProfileContext(this.requireStore(), spaceIds)
  }

  openMemory(id) {
    return openMemoryDocument(this.requireStore(), id)
  }

  confirmMemory(id, options) {
    return confirmMemory(this.requireStore(), id, options)
  }

  restoreMemory(id) {
    return restoreMemory(this.requireStore(), id)
  }

  undoEvent(id) {
    return undoMemoryEvent(this.requireStore(), id)
  }

  recordEpisode(input) {
    return recordEpisode(this.requireStore(), input)
  }

  pendingEpisodeBatches(options) {
    return pendingEpisodeBatches(this.requireStore(), options)
  }

  saveSessionCursor(sessionId, episodeAt) {
    return saveSessionCursor(this.requireStore(), sessionId, episodeAt)
  }

  forgetSession(sessionId) {
    return deleteSessionEpisodes(this.requireStore(), sessionId)
  }

  // 用户清除对话原文：事实保留，只是不再能追溯到原话。
  clearEpisodes({ spaceId = '' } = {}) {
    return clearEpisodes(this.requireStore(), { spaceId })
  }

  // 实验开关关着时 Bot 空间不做后台整理，不能为看不见的东西花钱。
  setBackgroundExclusions(getExcludedSpaceIds) {
    this.getExcludedSpaceIds = getExcludedSpaceIds
  }

  excludedSpaceIds() {
    try {
      return this.getExcludedSpaceIds?.(this.listSpaces({ includeBots: true })) || []
    } catch {
      return []
    }
  }

  async consolidate({ force = false, signal } = {}) {
    return runMemoryConsolidation(this.requireStore(), {
      force,
      signal,
      excludeSpaceIds: this.excludedSpaceIds(),
    })
  }

  // 导出成 Markdown：记忆存在 SQLite 里，但用户随时能看到、带走完整内容。
  exportMarkdown({ spaceId = '' } = {}) {
    const spaces = spaceId ? [this.getSpace(spaceId)].filter(Boolean) : this.listSpaces()
    const sections = spaces.map((space) => {
      const rows = this.requireDb()
        .prepare(
          `SELECT * FROM memories WHERE space_id = ? AND status IN ('active', 'stale')
           ORDER BY COALESCE(valid_from, created_at) DESC`,
        )
        .all(space.id)
        .map(rowMemory)
      return [
        `## ${space.name}`,
        '',
        ...rows.map(
          (memory) =>
            `- **${memory.title}**（${memory.type} · ${String(memory.validFrom).slice(0, 10)}）：${memory.content.replace(/\s+/g, ' ')}`,
        ),
        '',
      ].join('\n')
    })
    return ['# Pisper 记忆', '', ...sections].join('\n')
  }

  setSemanticSummarizer(summarizer) {
    this.semanticSummarizer = summarizer || null
    this.semanticGeneration += 1
    this.semanticQueue.clear()
    this.semanticLastError = ''
    if (this.db && summarizer) {
      const ids = this.requireDb()
        .prepare(
          "SELECT id FROM memories WHERE status IN ('active', 'stale') AND semantic_status IN ('pending', 'error')",
        )
        .all()
        .map((row) => row.id)
      if (ids.length) this.scheduleSemantic(ids)
    }
    return this.semanticStatus()
  }

  // 语义扩展队列：后台让模型写一行同义词和相关概念，索引到 expansion 列。
  scheduleSemantic(ids) {
    if (!this.semanticSummarizer || !this.db) return
    for (const id of ids) if (id) this.semanticQueue.add(id)
    if (!this.semanticPromise && this.semanticQueue.size) {
      const generation = this.semanticGeneration
      this.semanticPromise = Promise.resolve()
        .then(() => this.processSemanticQueue(generation))
        .finally(() => {
          this.semanticPromise = null
          if (this.semanticQueue.size && this.semanticSummarizer) this.scheduleSemantic([])
        })
    }
  }

  async processSemanticQueue(generation) {
    while (
      this.semanticQueue.size &&
      generation === this.semanticGeneration &&
      this.semanticSummarizer &&
      this.db
    ) {
      const ids = [...this.semanticQueue].slice(0, SEMANTIC_BATCH_SIZE)
      ids.forEach((id) => this.semanticQueue.delete(id))
      const placeholders = ids.map(() => '?').join(',')
      const rows = this.requireDb()
        .prepare(
          `SELECT id, title, content, space_id FROM memories WHERE id IN (${placeholders}) AND status IN ('active', 'stale')`,
        )
        .all(...ids)
      if (!rows.length) continue
      try {
        const summaries = await this.semanticSummarizer.summarize(
          rows.map((row) => ({ title: row.title, content: row.content })),
        )
        if (generation !== this.semanticGeneration || !this.db) return
        const now = this.isoNow()
        const update = this.requireDb().prepare(
          "UPDATE memories SET semantic_text = ?, semantic_status = ?, semantic_updated_at = ? WHERE id = ? AND status IN ('active', 'stale')",
        )
        this.transaction(() => {
          for (const [index, row] of rows.entries()) {
            const summary = redactSecretText(cleanText(String(summaries?.[index] || ''), 2000))
            update.run(summary, 'ready', now, row.id)
            this.indexFact(row.id)
          }
        })
        this.semanticLastError = ''
      } catch (error) {
        // 保留原始错误会把请求内容带进状态接口，这里只留一条通用说明。
        this.semanticLastError = error instanceof Error ? error.message : String(error)
        const markError = this.requireDb().prepare(
          "UPDATE memories SET semantic_status = 'error' WHERE id = ? AND status IN ('active', 'stale')",
        )
        for (const row of rows) markError.run(row.id)
        await new Promise((resolvePromise) => setImmediate(resolvePromise))
      }
    }
  }

  semanticStatus() {
    if (!this.db)
      return {
        enabled: Boolean(this.semanticSummarizer),
        pending: 0,
        ready: 0,
        failed: 0,
        running: false,
        error: '',
      }
    const counts = this.requireDb()
      .prepare(
        `
      SELECT semantic_status AS status, COUNT(*) AS count FROM memories WHERE status IN ('active', 'stale') GROUP BY semantic_status
    `,
      )
      .all()
    const byStatus = Object.fromEntries(counts.map((row) => [row.status, Number(row.count)]))
    return {
      enabled: Boolean(this.semanticSummarizer),
      pending: Number(byStatus.pending || 0),
      ready: Number(byStatus.ready || 0),
      failed: Number(byStatus.error || 0),
      running: Boolean(this.semanticPromise),
      error: this.semanticLastError,
    }
  }

  async drainSemanticQueue() {
    while (this.semanticPromise) await this.semanticPromise
    return this.semanticStatus()
  }

  // 后台任务：对账、批量抽取、整理都是一步一步轮流跑，同一时间只跑一个。
  // 运行时持有定时器和 AbortController，dispose 时取消并等它收尾。
  startBackground({ steps = [], intervalMs = BACKGROUND_INTERVAL_MS } = {}) {
    this.backgroundSteps = steps
    if (this.backgroundTimer) return
    this.backgroundTimer = setInterval(() => void this.runBackgroundOnce(), intervalMs)
    this.backgroundTimer.unref?.()
  }

  stopBackground() {
    if (this.backgroundTimer) clearInterval(this.backgroundTimer)
    this.backgroundTimer = null
    this.backgroundController?.abort()
  }

  runBackgroundOnce() {
    if (this.backgroundPromise || !this.db) return this.backgroundPromise || Promise.resolve()
    const controller = new AbortController()
    this.backgroundController = controller
    const steps = [
      (signal) => reconcilePendingMemories(this.requireStore(), { signal }),
      ...this.backgroundSteps,
      (signal) =>
        runMemoryConsolidation(this.requireStore(), {
          signal,
          excludeSpaceIds: this.excludedSpaceIds(),
        }),
    ]
    this.backgroundPromise = (async () => {
      for (const step of steps) {
        if (controller.signal.aborted || !this.db) return
        // 一步失败不影响下一步，也不能留下未处理的 rejection；日志不带内容。
        await Promise.resolve()
          .then(() => step(controller.signal))
          .catch(() => {
            if (!controller.signal.aborted) console.warn('[memory] 后台任务没能完成。')
          })
      }
    })().finally(() => {
      this.backgroundPromise = null
      if (this.backgroundController === controller) this.backgroundController = null
    })
    return this.backgroundPromise
  }

  schemaRevision() {
    return Number(readMemoryMeta(this.requireDb(), 'schema_revision') || MEMORY_SCHEMA_REVISION)
  }
}

function defaultEvidenceRole(sourceType) {
  if (sourceType === 'manual') return 'manual'
  if (sourceType === 'agent') return 'assistant'
  if (sourceType === 'tool_verified') return 'tool'
  if (sourceType === 'external') return 'external'
  return 'user'
}

function cleanIso(value) {
  const text = cleanText(value, 80)
  if (!text) return ''
  const time = Date.parse(text)
  return Number.isNaN(time) ? '' : new Date(time).toISOString()
}

function statusFilter(status) {
  if (status === 'archived') return ['archived']
  if (status === 'history') return ['superseded']
  if (status === 'all') return ['active', 'stale', 'superseded', 'archived']
  return ['active', 'stale']
}

function scopeLabelFor(spaceId) {
  if (spaceId === 'global') return 'global'
  if (String(spaceId).startsWith('bot:')) return 'bot'
  return 'project'
}

const asFact = (memory) => ({ ...memory, kind: 'fact' })

export {
  canonicalProjectPath,
  initializeMemoryFullTextSearch,
  memorySnapshot,
  stableProjectId,
  topicIdentity,
}
