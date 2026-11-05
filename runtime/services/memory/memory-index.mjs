// 记忆检索索引：事实、摘要、原文共用一张 FTS5 表，内容由 JS 预分词后写入。
// memory_docs 把 doc_id 映射到稳定的 rowid：memories 的隐式 rowid 在 VACUUM 后可能变，不能拿来当外键。
// SQLite 没编译 FTS5 时退回扫描 memory_docs.scan_tokens，功能不变，只是更慢。
import { ftsExpression, indexText } from './memory-tokenizer.mjs'

function isFts5UnavailableError(error) {
  return String(error?.message || error)
    .toLowerCase()
    .includes('no such module: fts5')
}

export function initializeMemoryFullTextSearch(db) {
  try {
    db.exec(`
      CREATE VIRTUAL TABLE IF NOT EXISTS memory_search USING fts5(title, body, expansion, tokenize='unicode61');
      CREATE VIRTUAL TABLE IF NOT EXISTS memory_search_vocab USING fts5vocab(memory_search, row);
    `)
    return true
  } catch (error) {
    if (!isFts5UnavailableError(error)) throw error
    return false
  }
}

const placeholders = (values) => values.map(() => '?').join(',')
const ABSENT_TOKEN_WEIGHT = 0.25

export class MemorySearchIndex {
  constructor(db, { ftsAvailable }) {
    this.db = db
    this.ftsAvailable = ftsAvailable
  }

  upsert({ kind, id, spaceId, title = '', body = '', expansion = '' }) {
    const titleTokens = indexText(title)
    const bodyTokens = indexText(body)
    const expansionTokens = indexText(expansion)
    const existing = this.db.prepare('SELECT rowid FROM memory_docs WHERE doc_id = ?').get(id)
    const scanTokens = this.ftsAvailable
      ? ''
      : `${titleTokens} ${bodyTokens} ${expansionTokens}`.trim()
    let rowid = existing?.rowid
    if (rowid) {
      this.db
        .prepare('UPDATE memory_docs SET kind = ?, space_id = ?, scan_tokens = ? WHERE rowid = ?')
        .run(kind, spaceId, scanTokens, rowid)
      if (this.ftsAvailable) this.db.prepare('DELETE FROM memory_search WHERE rowid = ?').run(rowid)
    } else {
      rowid = this.db
        .prepare(
          'INSERT INTO memory_docs (doc_id, kind, space_id, scan_tokens) VALUES (?, ?, ?, ?)',
        )
        .run(id, kind, spaceId, scanTokens).lastInsertRowid
    }
    if (this.ftsAvailable) {
      this.db
        .prepare('INSERT INTO memory_search (rowid, title, body, expansion) VALUES (?, ?, ?, ?)')
        .run(rowid, titleTokens, bodyTokens, expansionTokens)
    }
  }

  remove(id) {
    const row = this.db.prepare('SELECT rowid FROM memory_docs WHERE doc_id = ?').get(id)
    if (!row) return
    if (this.ftsAvailable)
      this.db.prepare('DELETE FROM memory_search WHERE rowid = ?').run(row.rowid)
    this.db.prepare('DELETE FROM memory_docs WHERE rowid = ?').run(row.rowid)
  }

  // 删作用域之前调用：memory_docs 没有外键，FTS 虚表也不会级联。
  removeSpace(spaceId) {
    if (this.ftsAvailable) {
      this.db
        .prepare(
          'DELETE FROM memory_search WHERE rowid IN (SELECT rowid FROM memory_docs WHERE space_id = ?)',
        )
        .run(spaceId)
    }
    this.db.prepare('DELETE FROM memory_docs WHERE space_id = ?').run(spaceId)
  }

  // 按 BM25 排序的候选 [{ id, kind }]。token 为空时返回空。
  match(tokens, { kinds, spaceIds, limit = 200 }) {
    if (!tokens.length || !spaceIds.length || !kinds.length) return []
    if (!this.ftsAvailable) return this.scan(tokens, { kinds, spaceIds, limit })
    try {
      return this.db
        .prepare(
          `SELECT docs.doc_id AS id, docs.kind AS kind
           FROM memory_search JOIN memory_docs docs ON docs.rowid = memory_search.rowid
           WHERE memory_search MATCH ?
             AND docs.space_id IN (${placeholders(spaceIds)})
             AND docs.kind IN (${placeholders(kinds)})
           ORDER BY bm25(memory_search, 8.0, 4.0, 2.0)
           LIMIT ?`,
        )
        .all(ftsExpression(tokens), ...spaceIds, ...kinds, limit)
    } catch {
      return this.scan(tokens, { kinds, spaceIds, limit })
    }
  }

  scan(tokens, { kinds, spaceIds, limit }) {
    const rows = this.db
      .prepare(
        `SELECT doc_id AS id, kind, scan_tokens FROM memory_docs
         WHERE space_id IN (${placeholders(spaceIds)}) AND kind IN (${placeholders(kinds)})`,
      )
      .all(...spaceIds, ...kinds)
    const scored = []
    for (const row of rows) {
      const present = new Set(row.scan_tokens.split(' '))
      let hits = 0
      for (const token of tokens) if (present.has(token)) hits += 1
      if (hits) scored.push({ id: row.id, kind: row.kind, hits })
    }
    return scored.sort((left, right) => right.hits - left.hits).slice(0, limit)
  }

  // 每个 token 的稀有度权重（0-1]：高频的“项目”“配置”不该和“团子”“safari16”一样重要。
  // 语料里没出现过的 token 给低权重：中文二元组里这类多半是跨词切出来的碎片（“的包”“定是”）。
  // 取值在评测集上校准：0.25 时自动回忆多留下约一半的正确结果，误注入没有增加。
  tokenWeights(tokens) {
    const weights = new Map(tokens.map((token) => [token, 1]))
    if (!this.ftsAvailable || !tokens.length) return weights
    try {
      const total = Number(this.db.prepare('SELECT COUNT(*) AS c FROM memory_docs').get()?.c || 0)
      if (!total) return weights
      const lookup = this.db.prepare('SELECT doc FROM memory_search_vocab WHERE term = ?')
      for (const token of tokens) {
        const df = Number(lookup.get(token)?.doc || 0)
        weights.set(
          token,
          df ? Math.log(1 + total / df) / Math.log(1 + total) : ABSENT_TOKEN_WEIGHT,
        )
      }
    } catch {
      // 词表查询失败时退回等权，不影响检索。
    }
    return weights
  }
}
