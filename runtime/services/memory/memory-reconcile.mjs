// 写入对账：新事实和已有事实矛盾时，让辅助模型判定要不要替换旧事实。
// topic 相同的替换在写入时就做完了（见 remember）；这里解决模型两次起的 topic 不一样、对不上的情况。
// 只走辅助模型：没配时什么都不做，行保持 pending，读取时的时间排序兜底。
// 判定不打断任何人，但每次都写 memory_events，带理由和原值，看得到也改得回。
// 任何不确定的情况都退回“都留着”：宁可多一条，不能把对的事实替换掉。
import { authorityTier, memorySnapshot } from './memory-lifecycle.mjs'
import { parseModelJson } from './memory-model-client.mjs'
import { factTimestamp } from './memory-retrieval.mjs'
import { RECONCILE_SYSTEM, reconcileUser } from '../../prompts/memory.mjs'

const NEIGHBOUR_LIMIT = 8
// 检索的 MMR 会压低和查询几乎一样的结果，而那正是要找的矛盾事实，所以多取一些再自己挑。
const NEIGHBOUR_POOL = 24
const BATCH_SIZE = 6
const MIN_COVERAGE = 0.25
const MIN_COVERAGE_WITH_SIGNAL = 0.1
const CONTENT_LIMIT = 600
const ACTOR = 'reconcile'
// 这些错误下一轮可能就好了，行保持 pending；其余错误（输出不可用）重试也没意义。
const RETRYABLE_ERRORS = new Set([
  'aborted',
  'budget_exhausted',
  'model_error',
  'model_unavailable',
])
// 用户在说“事情变了”，词面重合再少也值得问一次模型。
const CHANGE_SIGNAL = /不再|改用|改成|换成|迁到|搬到|现在|no longer|instead|switched|moved to/iu

function describe(row) {
  return {
    id: row.id,
    title: row.title,
    content: String(row.content).slice(0, CONTENT_LIMIT),
    type: row.type,
    date: String(factTimestamp(row) || '').slice(0, 10),
    authority: Number(row.authority || 0),
  }
}

// 同空间里可能和它说同一件事的事实。只看不晚于它的：后写入的事实轮到自己时会反过来看这条，
// 不然模型可能把新事实判成被旧事实替换。
function findNeighbours(store, row) {
  const threshold = CHANGE_SIGNAL.test(`${row.title} ${row.content}`)
    ? MIN_COVERAGE_WITH_SIGNAL
    : MIN_COVERAGE
  const from = String(factTimestamp(row) || '')
  return store
    .retrieve({
      query: `${row.title}\n${row.content}`,
      spaceIds: [row.space_id],
      kinds: ['fact'],
      limit: NEIGHBOUR_POOL,
      statuses: ['active', 'stale'],
      includeHistory: false,
      includeTainted: false,
      now: store.now(),
    })
    .filter((hit) => hit.row.id !== row.id)
    .filter((hit) => String(factTimestamp(hit.row) || '') <= from)
    .filter((hit) => hit.entityHit || hit.coverage >= threshold)
    .sort((left, right) => right.coverage - left.coverage)
    .slice(0, NEIGHBOUR_LIMIT)
    .map((hit) => hit.row)
}

function markDone(store, id) {
  store.db.prepare("UPDATE memories SET reconcile_status = 'done' WHERE id = ?").run(id)
}

function isoDate(value) {
  const time = Date.parse(String(value || ''))
  return Number.isNaN(time) ? '' : new Date(time).toISOString()
}

// 模型的输出是不可信输入：id 和 targets 都必须是这批给出去的，不合格的一律当 ADD。
function validDecision(raw, entry, store) {
  const action = String(raw?.action || '').toUpperCase()
  if (action !== 'SUPERSEDE' && action !== 'REINFORCE') return null
  const known = new Set(entry.neighbours.map((row) => row.id))
  const targets = Array.isArray(raw?.targets) ? raw.targets.map(String) : []
  if (!targets.length || !targets.every((id) => known.has(id))) return null
  const reason = store.safeText(raw?.reason, 200)
  if (!reason) return null
  return { action, targets: [...new Set(targets)], reason, validFrom: isoDate(raw?.valid_from) }
}

function loadFact(store, id) {
  return store.db.prepare('SELECT * FROM memories WHERE id = ?').get(id)
}

function archiveNewFact(store, row, archiveReason, event, now) {
  store.db
    .prepare(
      "UPDATE memories SET status = 'archived', archived_at = ?, archive_reason = ?, reconcile_status = 'done', updated_at = ? WHERE id = ?",
    )
    .run(now, archiveReason, now, row.id)
  store.indexFact(row.id)
  store.recordEvent(event)
}

function applySupersede(store, row, decision, model, now) {
  const targets = decision.targets
    .map((id) => loadFact(store, id))
    .filter((target) => target && (target.status === 'active' || target.status === 'stale'))
  if (!targets.length) {
    markDone(store, row.id)
    return 'added'
  }
  const event = {
    actor: ACTOR,
    memoryIds: [row.id, ...targets.map((target) => target.id)],
    spaceId: row.space_id,
    reason: decision.reason,
    model,
    before: Object.fromEntries([row, ...targets].map((item) => [item.id, memorySnapshot(item)])),
  }
  // 权威守卫：模型的推断不能替换用户亲口说的。有一条替换不了，这条新事实就不生效。
  const tier = authorityTier(Number(row.authority || 0))
  if (targets.some((target) => authorityTier(Number(target.authority || 0)) > tier)) {
    archiveNewFact(store, row, 'lower_authority_conflict', { ...event, action: 'archive' }, now)
    return 'archived'
  }
  // 模型说得出“从什么时候起”时用它，否则就是新事实自己的时间。
  const validFrom =
    decision.validFrom && decision.validFrom < now ? decision.validFrom : factTimestamp(row)
  store.db
    .prepare(
      "UPDATE memories SET valid_from = ?, reconcile_status = 'done', updated_at = ? WHERE id = ?",
    )
    .run(validFrom, now, row.id)
  const supersede = store.db.prepare(
    "UPDATE memories SET status = 'superseded', superseded_by = ?, superseded_at = ?, valid_to = ?, updated_at = ? WHERE id = ?",
  )
  for (const target of targets) {
    // valid_to 不能早于这条旧事实自己开始的时间。
    const from = factTimestamp(target)
    supersede.run(row.id, now, from && from > validFrom ? from : validFrom, now, target.id)
    store.indexFact(target.id)
  }
  store.indexFact(row.id)
  store.recordEvent({ ...event, action: 'supersede' })
  store.markDerivedDirty(
    targets.map((target) => target.id),
    { spaceId: row.space_id },
  )
  return 'superseded'
}

function applyReinforce(store, row, decision, model, now) {
  const target = decision.targets
    .map((id) => loadFact(store, id))
    .find((item) => item && (item.status === 'active' || item.status === 'stale'))
  if (!target) {
    markDone(store, row.id)
    return 'added'
  }
  // 用户亲口重申模型猜过的内容：来源和证据升级成用户的，和 remember 的去重路径一致。
  const stronger = Number(row.authority || 0) > Number(target.authority || 0)
  store.db
    .prepare(
      `UPDATE memories SET confirm_count = confirm_count + 1, last_confirmed_at = ?, updated_at = ?,
         status = CASE WHEN status = 'stale' THEN 'active' ELSE status END,
         source_type = ?, authority = ?, evidence = ?, evidence_role = ?, tainted = ?
       WHERE id = ?`,
    )
    .run(
      now,
      now,
      stronger ? row.source_type : target.source_type,
      stronger ? row.authority : target.authority,
      stronger ? row.evidence || target.evidence : target.evidence,
      stronger ? row.evidence_role : target.evidence_role,
      stronger ? row.tainted : target.tainted,
      target.id,
    )
  store.indexFact(target.id)
  archiveNewFact(
    store,
    row,
    'duplicate',
    {
      actor: ACTOR,
      action: 'reinforce',
      memoryIds: [row.id, target.id],
      spaceId: row.space_id,
      reason: decision.reason,
      model,
      before: Object.fromEntries([row, target].map((item) => [item.id, memorySnapshot(item)])),
    },
    now,
  )
  return 'reinforced'
}

function applyDecision(store, entry, decision, model) {
  // 同一批里前面的判定可能已经把这条替换掉了。
  const row = loadFact(store, entry.row.id)
  if (!row || row.status !== 'active') {
    if (row) markDone(store, row.id)
    return 'added'
  }
  if (!decision) {
    markDone(store, row.id)
    return 'added'
  }
  const now = store.isoNow()
  return store.transaction(() =>
    decision.action === 'SUPERSEDE'
      ? applySupersede(store, row, decision, model, now)
      : applyReinforce(store, row, decision, model, now),
  )
}

function diagnose(code) {
  // 只留稳定诊断码，SDK 错误和模型输出都可能含记忆正文。
  console.warn('[memory] reconcile', code)
}

/**
 * 处理 reconcile_status = 'pending' 的事实。没有辅助模型时什么都不做。
 * @param {object} store createStore() 给的窄接口
 * @param {{ signal?: AbortSignal, limit?: number }} [options]
 * @returns {Promise<{ processed: number, superseded: number, reinforced: number, archived: number }>}
 */
export async function reconcilePendingMemories(store, { signal, limit = 12 } = {}) {
  const totals = { processed: 0, superseded: 0, reinforced: 0, archived: 0 }
  const client = store.modelClient()
  if (!client || !(await client.available())) return totals
  if (signal?.aborted) return totals
  const rows = store.db
    .prepare(
      `SELECT * FROM memories
       WHERE reconcile_status = 'pending' AND status = 'active' AND tainted = 0
       ORDER BY COALESCE(valid_from, created_at), created_at LIMIT ?`,
    )
    .all(limit)
  const pending = []
  for (const row of rows) {
    const neighbours = findNeighbours(store, row)
    if (neighbours.length) pending.push({ row, neighbours })
    else {
      // 没有可比的就不用问模型。
      markDone(store, row.id)
      totals.processed += 1
    }
  }
  for (let start = 0; start < pending.length; start += BATCH_SIZE) {
    if (signal?.aborted) return totals
    const batch = pending.slice(start, start + BATCH_SIZE)
    const payload = batch.map((entry) => ({
      ...describe(entry.row),
      neighbours: entry.neighbours.map(describe),
    }))
    const result = await client.complete({
      purpose: 'reconcile',
      system: RECONCILE_SYSTEM,
      user: reconcileUser(payload),
      maxTokens: Math.max(800, batch.length * 300),
      signal,
    })
    if (signal?.aborted) return totals
    if (result.errorCode && RETRYABLE_ERRORS.has(result.errorCode)) {
      if (result.errorCode !== 'aborted') diagnose(result.errorCode)
      return totals
    }
    if (result.errorCode) diagnose(result.errorCode)
    const parsed = result.errorCode ? null : parseModelJson(result.text)
    const decisions = Array.isArray(parsed?.decisions) ? parsed.decisions : []
    if (!result.errorCode && !decisions.length) diagnose('invalid_response')
    const byId = new Map(decisions.map((item) => [String(item?.id || ''), item]))
    for (const entry of batch) {
      const raw = byId.get(entry.row.id)
      const outcome = applyDecision(
        store,
        entry,
        raw ? validDecision(raw, entry, store) : null,
        result.model || '',
      )
      totals.processed += 1
      if (outcome !== 'added') totals[outcome] += 1
    }
  }
  return totals
}
