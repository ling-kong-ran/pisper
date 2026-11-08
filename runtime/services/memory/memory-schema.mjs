// 记忆库 schema：只做加法。
// user_version 故意冻结在 4：已发布的旧版本一看到不同的 user_version 就 DROP 全部记忆表，
// 用户回退版本会丢数据。真实版本记在 memory_meta.schema_revision；新增的列都有默认值，
// 旧代码只读具名列、只认 status='active'，新增内容对它不可见但不会被破坏。
import { randomBytes, randomUUID } from 'node:crypto'
import { existsSync } from 'node:fs'
import { pisperError } from '../../../shared/errors.mjs'

export const FROZEN_USER_VERSION = 4
export const MEMORY_SCHEMA_REVISION = 5

const BASE_SCHEMA = `
  CREATE TABLE IF NOT EXISTS memory_spaces (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    root_path TEXT NOT NULL DEFAULT '',
    root_key TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
  );
  CREATE UNIQUE INDEX IF NOT EXISTS memory_spaces_root_key ON memory_spaces(root_key) WHERE root_key <> '';
  CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL REFERENCES memory_spaces(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    type TEXT NOT NULL,
    topic_key TEXT NOT NULL,
    identity_key TEXT NOT NULL,
    source_type TEXT NOT NULL,
    source_id TEXT NOT NULL DEFAULT '',
    source_path TEXT NOT NULL DEFAULT '',
    session_id TEXT NOT NULL DEFAULT '',
    cwd TEXT NOT NULL DEFAULT '',
    evidence TEXT NOT NULL DEFAULT '',
    source_timestamp TEXT NOT NULL DEFAULT '',
    importance REAL NOT NULL DEFAULT 0.5,
    authority INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    revision INTEGER NOT NULL DEFAULT 1,
    superseded_by TEXT NOT NULL DEFAULT '',
    superseded_at TEXT,
    verified_at TEXT,
    expires_at TEXT,
    semantic_text TEXT NOT NULL DEFAULT '',
    semantic_status TEXT NOT NULL DEFAULT 'pending',
    semantic_updated_at TEXT,
    access_count INTEGER NOT NULL DEFAULT 0,
    last_accessed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
  );
  CREATE INDEX IF NOT EXISTS memories_active_space ON memories(space_id, status, importance DESC, updated_at DESC);
  CREATE INDEX IF NOT EXISTS memories_active_identity ON memories(space_id, status, identity_key);
  CREATE INDEX IF NOT EXISTS memories_semantic_queue ON memories(status, semantic_status, updated_at);
  CREATE TABLE IF NOT EXISTS memory_links (
    id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL REFERENCES memory_spaces(id) ON DELETE CASCADE,
    source_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    target_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    relation TEXT NOT NULL,
    weight REAL NOT NULL DEFAULT 0.5,
    created_at TEXT NOT NULL,
    UNIQUE(source_id, target_id, relation)
  );
  CREATE TABLE IF NOT EXISTS memory_candidates (
    id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL REFERENCES memory_spaces(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    type TEXT NOT NULL,
    topic_key TEXT NOT NULL,
    identity_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    source_type TEXT NOT NULL,
    source_id TEXT NOT NULL DEFAULT '',
    session_id TEXT NOT NULL DEFAULT '',
    cwd TEXT NOT NULL DEFAULT '',
    importance REAL NOT NULL DEFAULT 0.5,
    evidence TEXT NOT NULL DEFAULT '',
    confidence REAL NOT NULL DEFAULT 0.5,
    source_timestamp TEXT NOT NULL DEFAULT '',
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(space_id, identity_key)
  );
  CREATE INDEX IF NOT EXISTS memory_candidates_created ON memory_candidates(created_at DESC);
  CREATE TABLE IF NOT EXISTS memory_tombstones (
    id TEXT PRIMARY KEY,
    entity_type TEXT NOT NULL,
    action TEXT NOT NULL,
    replacement_id TEXT NOT NULL DEFAULT '',
    reason_code TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
  );
`

// 在 memories 上追加的列：全部可空或有默认值，旧代码的 INSERT 不受影响。
const MEMORY_COLUMNS = [
  // 双时态：valid_* 是事实何时为真；created_at / superseded_at 是系统何时知道。
  ['valid_from', 'TEXT'],
  ['valid_to', 'TEXT'],
  ['last_confirmed_at', 'TEXT'],
  ['confirm_count', 'INTEGER NOT NULL DEFAULT 1'],
  ['pinned', 'INTEGER NOT NULL DEFAULT 0'],
  // 证据来自谁的话：user | assistant | tool | external | manual
  ['evidence_role', "TEXT NOT NULL DEFAULT 'manual'"],
  // 来自网页、工具输出或外部渠道：不进自动回忆、摘要和画像
  ['tainted', 'INTEGER NOT NULL DEFAULT 0'],
  ['episode_id', "TEXT NOT NULL DEFAULT ''"],
  // source_path 指向内容的哈希：用前校验引用是否失效
  ['anchor_hash', "TEXT NOT NULL DEFAULT ''"],
  ['anchor_checked_at', 'TEXT'],
  ['archived_at', 'TEXT'],
  ['archive_reason', "TEXT NOT NULL DEFAULT ''"],
  // pending 表示还没和已有事实对账；状态在库里，重启不丢。
  ['reconcile_status', "TEXT NOT NULL DEFAULT 'done'"],
  ['indexed_at', 'TEXT'],
]

// memory_digests.dirty：0 干净；1 需要重算但可读；2 来源被用户删除，重算前不得参与检索。
// memory_events：审计与撤销，只追加；物理删除时连同快照一起清除。
// memory_forgotten：用户删掉的内容的 HMAC 指纹，不存明文：再次抽到同样的内容时直接丢弃，不让它复活。
// memory_episodes.captured：1 表示这一轮已经抽取过（即时路径或批量），批量不再处理。
// memory_episodes.bytes：正文字节数，算总量用；放在正文前面，SUM 时不用读溢出页。
// （说明写在这里而不是 SQL 注释里：SQL 文本里只放英文。）
const V5_TABLES = `
  CREATE TABLE IF NOT EXISTS memory_docs (
    rowid INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    space_id TEXT NOT NULL,
    scan_tokens TEXT NOT NULL DEFAULT ''
  );
  CREATE INDEX IF NOT EXISTS memory_docs_space ON memory_docs(space_id, kind);
  CREATE TABLE IF NOT EXISTS memory_entities (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    entity TEXT NOT NULL,
    PRIMARY KEY (memory_id, entity)
  );
  CREATE INDEX IF NOT EXISTS memory_entities_entity ON memory_entities(entity);
  CREATE TABLE IF NOT EXISTS memory_episodes (
    id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL REFERENCES memory_spaces(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    entry_id TEXT NOT NULL DEFAULT '',
    cwd TEXT NOT NULL DEFAULT '',
    captured INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,
    at TEXT NOT NULL,
    user_text TEXT NOT NULL,
    assistant_text TEXT NOT NULL
  );
  CREATE INDEX IF NOT EXISTS memory_episodes_session ON memory_episodes(session_id, at);
  CREATE INDEX IF NOT EXISTS memory_episodes_pending ON memory_episodes(session_id, at) WHERE captured = 0;
  CREATE INDEX IF NOT EXISTS memory_episodes_oldest ON memory_episodes(at);
  CREATE INDEX IF NOT EXISTS memory_episodes_time ON memory_episodes(space_id, at DESC);
  CREATE TABLE IF NOT EXISTS memory_digests (
    id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL REFERENCES memory_spaces(id) ON DELETE CASCADE,
    level TEXT NOT NULL,
    window_start TEXT NOT NULL,
    window_end TEXT NOT NULL,
    text TEXT NOT NULL,
    source_ids TEXT NOT NULL DEFAULT '[]',
    dirty INTEGER NOT NULL DEFAULT 0,
    generated_at TEXT NOT NULL,
    UNIQUE(space_id, level, window_start)
  );
  CREATE INDEX IF NOT EXISTS memory_digests_window ON memory_digests(space_id, window_start DESC);
  CREATE TABLE IF NOT EXISTS memory_profiles (
    space_id TEXT PRIMARY KEY REFERENCES memory_spaces(id) ON DELETE CASCADE,
    text TEXT NOT NULL,
    source_ids TEXT NOT NULL DEFAULT '[]',
    dirty INTEGER NOT NULL DEFAULT 1,
    generated_at TEXT NOT NULL
  );
  CREATE TABLE IF NOT EXISTS memory_events (
    id TEXT PRIMARY KEY,
    at TEXT NOT NULL,
    space_id TEXT NOT NULL DEFAULT '',
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    memory_ids TEXT NOT NULL,
    before_json TEXT NOT NULL DEFAULT '',
    reason TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    undone_at TEXT
  );
  CREATE INDEX IF NOT EXISTS memory_events_time ON memory_events(space_id, at DESC);
  CREATE TABLE IF NOT EXISTS memory_session_cursors (
    session_id TEXT PRIMARY KEY,
    episode_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
  );
  CREATE TABLE IF NOT EXISTS memory_forgotten (
    fingerprint TEXT PRIMARY KEY,
    expires_at TEXT NOT NULL
  );
  CREATE TABLE IF NOT EXISTS memory_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
  );
`

function userVersion(db) {
  return Number(db.prepare('PRAGMA user_version').get()?.user_version || 0)
}

function hasTable(db, name) {
  return Boolean(db.prepare('SELECT 1 FROM sqlite_master WHERE name = ?').get(name))
}

function hasRows(db, table) {
  return hasTable(db, table) && Boolean(db.prepare(`SELECT 1 FROM ${table} LIMIT 1`).get())
}

// 升级前留一份完整副本。备份失败不能挡住启动：迁移本身只做加法，备份只是多一层保险。
function backupDatabase(db, path, suffix, onError) {
  if (!path || path === ':memory:') return ''
  const target = `${path}.${suffix}.bak`
  if (existsSync(target)) return target
  try {
    db.prepare('VACUUM INTO ?').run(target)
    return target
  } catch (error) {
    onError?.(error)
    return ''
  }
}

function dropIncompatibleTables(db) {
  db.exec(`
    DROP TRIGGER IF EXISTS memories_ai;
    DROP TRIGGER IF EXISTS memories_ad;
    DROP TRIGGER IF EXISTS memories_au;
    DROP TABLE IF EXISTS memory_fts;
    DROP TABLE IF EXISTS memory_embedding_buckets;
    DROP TABLE IF EXISTS memory_links;
    DROP TABLE IF EXISTS memory_candidates;
    DROP TABLE IF EXISTS memories;
    DROP TABLE IF EXISTS memory_spaces;
    DROP TABLE IF EXISTS memory_tombstones;
    DROP TABLE IF EXISTS memory_search_vocab;
    DROP TABLE IF EXISTS memory_search;
    DROP TABLE IF EXISTS memory_docs;
    DROP TABLE IF EXISTS memory_entities;
    DROP TABLE IF EXISTS memory_episodes;
    DROP TABLE IF EXISTS memory_digests;
    DROP TABLE IF EXISTS memory_profiles;
    DROP TABLE IF EXISTS memory_events;
    DROP TABLE IF EXISTS memory_session_cursors;
    DROP TABLE IF EXISTS memory_forgotten;
    DROP TABLE IF EXISTS memory_meta;
  `)
}

export function readMemoryMeta(db, key) {
  return db.prepare('SELECT value FROM memory_meta WHERE key = ?').get(key)?.value ?? null
}

export function writeMemoryMeta(db, key, value) {
  db.prepare(
    'INSERT INTO memory_meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value',
  ).run(key, String(value))
}

// 记忆不需要确认：还在候选箱里的条目直接转正。
// 和已有事实撞同一个主题的归档（低权威不能顶掉现有事实），可以在“已归档”里找回。
function promotePendingCandidates(db, now) {
  if (!hasRows(db, 'memory_candidates')) return 0
  const rows = db.prepare('SELECT * FROM memory_candidates ORDER BY created_at').all()
  const occupied = db.prepare(
    "SELECT 1 FROM memories WHERE space_id = ? AND identity_key = ? AND status = 'active' LIMIT 1",
  )
  const insert = db.prepare(`
    INSERT OR IGNORE INTO memories (id, space_id, title, content, type, topic_key, identity_key, source_type, source_id,
      session_id, cwd, evidence, source_timestamp, importance, authority, status, archived_at, archive_reason,
      evidence_role, semantic_status, reconcile_status, created_at, updated_at)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', 'pending', ?, ?)
  `)
  for (const row of rows) {
    const conflict = Boolean(occupied.get(row.space_id, row.identity_key))
    insert.run(
      row.id,
      row.space_id,
      row.title,
      row.content,
      row.type,
      row.topic_key,
      row.identity_key,
      row.source_type,
      row.source_id,
      row.session_id,
      row.cwd,
      row.evidence,
      row.source_timestamp,
      row.importance,
      row.source_type === 'agent' ? 40 : 90,
      conflict ? 'archived' : 'active',
      conflict ? now : null,
      conflict ? 'lower_authority_conflict' : '',
      row.source_type === 'agent' ? 'assistant' : 'user',
      row.created_at,
      now,
    )
  }
  db.exec('DELETE FROM memory_candidates')
  return rows.length
}

// 会话摘要要能按会话找回来。这一列不提高 schema_revision：已发布的版本看到更高的 revision 会拒绝打开记忆库，
// 而它们只读具名列，多一列不受影响，用户还能回退。
function ensureDigestSessionColumn(db) {
  const columns = db.prepare('PRAGMA table_info(memory_digests)').all()
  if (!columns.some((column) => column.name === 'session_id'))
    db.exec("ALTER TABLE memory_digests ADD COLUMN session_id TEXT NOT NULL DEFAULT ''")
  db.exec('CREATE INDEX IF NOT EXISTS memory_digests_session ON memory_digests(session_id)')
}

function applyRevision5(db, now) {
  const columns = new Set(
    db
      .prepare('PRAGMA table_info(memories)')
      .all()
      .map((column) => column.name),
  )
  for (const [name, definition] of MEMORY_COLUMNS) {
    if (!columns.has(name)) db.exec(`ALTER TABLE memories ADD COLUMN ${name} ${definition}`)
  }
  db.exec(V5_TABLES)
  // 新索引由 memory-index.mjs 用预分词写入；旧的 memory_fts 与触发器不再需要。
  // 回退旧版时，旧代码的 CREATE IF NOT EXISTS 会把它们重建出来。
  db.exec(`
    CREATE INDEX IF NOT EXISTS memories_timeline ON memories(space_id, status, valid_from DESC);
    CREATE INDEX IF NOT EXISTS memories_reconcile ON memories(reconcile_status, status);
    DROP TRIGGER IF EXISTS memories_ai;
    DROP TRIGGER IF EXISTS memories_ad;
    DROP TRIGGER IF EXISTS memories_au;
    DROP TABLE IF EXISTS memory_fts;
  `)
  db.prepare(
    `
    UPDATE memories SET
      valid_from = COALESCE(valid_from, NULLIF(source_timestamp, ''), created_at),
      valid_to = CASE WHEN status = 'superseded' THEN COALESCE(valid_to, superseded_at) ELSE valid_to END,
      last_confirmed_at = COALESCE(last_confirmed_at, verified_at, created_at),
      evidence_role = CASE
        WHEN source_type IN ('manual') THEN 'manual'
        WHEN source_type IN ('user_confirmed', 'conversation_confirmed', 'auto_approved', 'conversation') THEN 'user'
        WHEN source_type = 'agent' THEN 'assistant'
        WHEN source_type = 'tool_verified' THEN 'tool'
        ELSE evidence_role
      END
  `,
  ).run()
  // 旧版把全局空间写死成中文名；界面按 kind 翻译，库里统一存英文，导出 Markdown 时直接用它。
  db.prepare("UPDATE memory_spaces SET name = 'Global' WHERE id = 'global'").run()
  const moved = promotePendingCandidates(db, now)
  if (moved) {
    db.prepare(
      'INSERT INTO memory_events (id, at, actor, action, memory_ids, reason) VALUES (?, ?, ?, ?, ?, ?)',
    ).run(randomUUID(), now, 'system', 'migrate', '[]', `promoted ${moved} pending candidates`)
  }
  if (!readMemoryMeta(db, 'forget_salt'))
    writeMemoryMeta(db, 'forget_salt', randomBytes(32).toString('hex'))
  writeMemoryMeta(db, 'schema_revision', MEMORY_SCHEMA_REVISION)
}

// 打开或升级记忆库。返回 { upgraded, backupPath }；遇到不认识的未来版本直接抛错，绝不动数据。
export function initializeMemorySchema(
  db,
  { path = '', now = new Date().toISOString(), onBackupError } = {},
) {
  const version = userVersion(db)
  const existing = hasTable(db, 'memories') || hasTable(db, 'memory_spaces')
  let backupPath = ''
  if (existing && version !== FROZEN_USER_VERSION) {
    if (version > FROZEN_USER_VERSION) {
      throw pisperError(
        'unsupported',
        'memory',
        `database version ${version} is newer than this app`,
      )
    }
    // 更早的 schema 缺的列太多，和历史行为一样重建；但这次先留备份。
    backupPath = backupDatabase(db, path, `v${version}`, onBackupError)
    dropIncompatibleTables(db)
  }
  db.exec(BASE_SCHEMA)
  db.exec(`PRAGMA user_version = ${FROZEN_USER_VERSION}`)
  db.exec('CREATE TABLE IF NOT EXISTS memory_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)')
  const revision = Number(readMemoryMeta(db, 'schema_revision') || 0)
  if (revision > MEMORY_SCHEMA_REVISION) {
    throw pisperError(
      'unsupported',
      'memory',
      `database version ${revision} is newer than this app`,
    )
  }
  let upgraded = false
  if (revision < MEMORY_SCHEMA_REVISION) {
    if (hasRows(db, 'memories') || hasRows(db, 'memory_candidates')) {
      backupPath = backupDatabase(db, path, 'pre-v5', onBackupError) || backupPath
    }
    db.exec('BEGIN IMMEDIATE')
    try {
      applyRevision5(db, now)
      db.exec('COMMIT')
    } catch (error) {
      db.exec('ROLLBACK')
      throw error
    }
    upgraded = true
  }
  ensureDigestSessionColumn(db)
  // 回退旧版期间写入的行没有这些字段，下次打开时补上；条件查询很便宜，每次启动跑一次。
  db.prepare(
    `
    UPDATE memories SET
      valid_from = COALESCE(valid_from, NULLIF(source_timestamp, ''), created_at),
      last_confirmed_at = COALESCE(last_confirmed_at, verified_at, created_at),
      valid_to = CASE WHEN status = 'superseded' THEN COALESCE(valid_to, superseded_at) ELSE valid_to END
    WHERE valid_from IS NULL OR last_confirmed_at IS NULL OR (status = 'superseded' AND valid_to IS NULL)
  `,
  ).run()
  return { upgraded, backupPath }
}
