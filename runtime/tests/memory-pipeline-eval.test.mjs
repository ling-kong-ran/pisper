import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { resolve } from 'node:path'
import test from 'node:test'
import {
  EXTRACT_BATCH_SYSTEM,
  EXTRACT_TURN_SYSTEM,
  SEMANTIC_EXPANSION_SYSTEM,
} from '../prompts/memory.mjs'
import { ConversationMemoryCapture } from '../runtime/conversation-memory-capture.mjs'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import { passesRecallGate, retrieve } from '../services/memory/memory-retrieval.mjs'
import { isInformativeQuery } from '../services/memory/memory-tokenizer.mjs'
import { createMemoryModelClient } from '../services/memory/memory-model-client.mjs'
import { createSemanticMemorySummarizer } from '../services/memory/semantic-memory.mjs'
import { measureFollowups } from './fixtures/memory-followup-eval.mjs'
import {
  answers,
  questions,
  spellingPairs,
  turns,
  unanswered,
} from './fixtures/memory-pipeline-cases.mjs'
import replay from './fixtures/memory-pipeline-model.json' with { type: 'json' }

const now = Date.parse('2026-06-11T12:00:00Z')
const paths = Object.fromEntries(
  ['drill', 'archive', 'chat'].map((key) => [key, resolve('memory-eval', key)]),
)

// 只替换 SDK 的模型调用。录制时模型只看生产提示词与源对话，未提供查询或答案。
// 用请求中的原文识别回放，避免按调用次序配对掩盖漏传或串用对话的问题。
function replayModel() {
  const systems = [EXTRACT_TURN_SYSTEM, EXTRACT_BATCH_SYSTEM, SEMANTIC_EXPANSION_SYSTEM]
  assert.equal(
    createHash('sha256').update(JSON.stringify(systems)).digest('hex'),
    replay.systemPromptsSha256,
    '记忆提示词已变，请用新提示词重新盲测并更新回放。',
  )
  const keywords = new Map()
  for (const { items, keywords: terms } of Object.values(replay.responses)) {
    assert.equal(items.length, terms.length)
    items.forEach((item, i) => keywords.set(JSON.stringify([item.title, item.content]), terms[i]))
  }
  const calls = { immediate: [], batch: [] }
  return {
    calls,
    async completeSimple(_model, { systemPrompt, messages }) {
      const prompt = messages[0].content
      let items
      if (systemPrompt === SEMANTIC_EXPANSION_SYSTEM) {
        const blocks = JSON.parse(prompt.slice(prompt.indexOf('[')))
        items = blocks.map(({ id, title, content }) => {
          const terms = keywords.get(JSON.stringify([title, content]))
          assert.ok(terms, 'missing expansion replay')
          return { id, keywords: terms }
        })
      } else {
        const mode = systemPrompt === EXTRACT_BATCH_SYSTEM ? 'batch' : 'immediate'
        if (mode === 'immediate') assert.equal(systemPrompt, EXTRACT_TURN_SYSTEM)
        const sources = turns.filter(
          ({ user, assistant }) => prompt.includes(user) && prompt.includes(assistant),
        )
        assert.equal(sources.length, 1, 'extraction request must match one recorded turn')
        const [{ key }] = sources
        calls[mode].push(key)
        items = replay.responses[key].items
      }
      return {
        content: [{ type: 'text', text: JSON.stringify(items) }],
        stopReason: 'stop',
        timestamp: now,
      }
    },
  }
}

function containsAnswer(answer, text) {
  return answer.patterns.every((pattern) => pattern.test(text))
}

// 格式化会按 3000 字符整条舍弃，也会截断正文。只给真正送进提示词的答案计分。
function renderedFacts({ text, memories }) {
  assert.ok(text.length <= 3000)
  assert.ok(memories.length <= 4)
  const blocks = [
    ...text.matchAll(
      /<memory\b[^>]*\bid="([^"]+)"[^>]*>\s*<title>([^<]*)<\/title>\s*<content>([^<]*)<\/content>\s*(?:<evidence>([^<]*)<\/evidence>\s*)?<\/memory>/gu,
    ),
  ]
  assert.deepEqual(
    blocks.map(([, id]) => id),
    memories.map(({ id }) => id),
  )
  return blocks.map(([, , title, content, evidence = ''], i) => ({
    row: memories[i],
    text: [title, content, evidence].join(' '),
  }))
}

function auditStorage(memory, stored) {
  const scopes = {
    global: 'global',
    chat: 'chats',
    drill: memory.workspaceSpaceId(paths.drill),
    archive: memory.workspaceSpaceId(paths.archive),
  }
  const missing = []
  const misplaced = []
  for (const answer of answers) {
    const matches = stored.filter((row) => containsAnswer(answer, row.title + ' ' + row.content))
    if (!matches.length) missing.push(answer.key)
    else if (
      matches.some((row) => row.spaceId !== scopes[answer.scope] || row.sessionId !== answer.source)
    )
      misplaced.push(answer.key)
  }
  const unexpectedWrites = stored
    .filter(
      (row) =>
        !answers.some(
          (answer) =>
            answer.source === row.sessionId &&
            containsAnswer(answer, row.title + ' ' + row.content),
        ),
    )
    .map((row) => row.sessionId)
  return { missing, misplaced, unexpectedWrites }
}

function visibleToQuestion(row, q) {
  const origin = answers.find(({ source }) => source === row.sessionId)
  if (!origin) return false
  if (origin.scope === 'global') return true
  return origin.scope === q.workspace && (origin.scope !== 'chat' || origin.source === q.sessionId)
}

// 按自动入口实际使用的候选参数回放，区分覆盖率门槛与通过门槛后的四条/字符预算。
function diagnoseMiss(memory, q, answer) {
  if (!isInformativeQuery(q.text)) return { stage: 'uninformative' }
  const cwd = paths[q.workspace]
  const hits = memory.applyScopePrecedence(
    retrieve(memory.requireStore(), {
      query: q.text,
      spaceIds: memory.resolveSpaceIds({ cwd, sessionId: q.sessionId }),
      sessionId: q.sessionId,
      kinds: ['fact'],
      includeHistory: false,
      includeTainted: false,
      limit: 8,
      now,
    }),
    cwd,
  )
  const index = hits.findIndex(
    ({ row }) =>
      row.session_id === answer.source && containsAnswer(answer, row.title + ' ' + row.content),
  )
  if (index < 0) return { stage: 'absent-from-candidates' }
  const hit = hits[index]
  return {
    stage: passesRecallGate(hit) ? 'after-gate' : 'coverage-gate',
    rank: index + 1,
    matched: hit.matched,
    coverage: Number(hit.coverage.toFixed(3)),
  }
}

async function measureSpelling(memory) {
  const results = []
  for (const [key, workspace, raw, corrected, suffix, target] of spellingPairs) {
    const q = { workspace, sessionId: 'new-session' }
    const answer = target && answers.find((item) => item.key === target)
    const pair = []
    for (const term of [raw, corrected]) {
      const context = await memory.relevantContext(term + suffix, paths[workspace], undefined, {
        sessionId: q.sessionId,
      })
      const rows = renderedFacts(context)
      assert.ok(
        rows.every(({ row }) => visibleToQuestion(row, q)),
        key,
      )
      pair.push(
        answer
          ? rows.some(
              ({ row, text }) => row.sessionId === answer.source && containsAnswer(answer, text),
            )
          : rows.length > 0,
      )
    }
    results.push({ key, positive: Boolean(target), raw: pair[0], corrected: pair[1] })
  }
  return results
}

async function measureRecall(memory, { missing, misplaced }) {
  const misses = []
  const falseInjections = []
  const leaks = []
  const manualLeaks = []
  for (const q of questions) {
    const cwd = paths[q.workspace]
    const rendered = renderedFacts(
      await memory.relevantContext(q.text, cwd, undefined, { sessionId: q.sessionId }),
    )
    if (rendered.some(({ row }) => !visibleToQuestion(row, q))) leaks.push(q.key)
    const manual = memory.search(q.text, { cwd, sessionId: q.sessionId, trackAccess: false })
    if (manual.some((row) => !visibleToQuestion(row, q))) manualLeaks.push(q.key)
    if (!q.answer) {
      if (rendered.length) falseInjections.push(q.key)
      continue
    }
    const answer = answers.find(({ key }) => key === q.answer)
    if (
      rendered.some(
        ({ row, text }) => row.sessionId === answer.source && containsAnswer(answer, text),
      )
    )
      continue

    // 和同一可见范围的手动搜索对照，以区分“搜得到，没自动带入”和候选缺失。
    const rank = manual.findIndex(
      (row) =>
        row.sessionId === answer.source && containsAnswer(answer, row.title + ' ' + row.content),
    )
    let diagnosis
    if (missing.includes(answer.key)) diagnosis = { stage: 'capture' }
    else if (misplaced.includes(answer.key)) diagnosis = { stage: 'scope' }
    else diagnosis = diagnoseMiss(memory, q, answer)
    misses.push({ query: q.key, ...diagnosis, manualRank: rank < 0 ? null : rank + 1 })
  }
  const relatedButUnanswered = []
  for (const [key, text] of unanswered) {
    const q = { workspace: 'drill', sessionId: 'new-session' }
    const rendered = renderedFacts(
      await memory.relevantContext(text, paths.drill, undefined, { sessionId: q.sessionId }),
    )
    if (rendered.some(({ row }) => !visibleToQuestion(row, q))) leaks.push(key)
    if (rendered.length) relatedButUnanswered.push(key)
  }
  const positive = questions.filter((q) => q.answer).length
  return {
    recallAt4: [positive - misses.length, positive],
    misses,
    falseInjections,
    relatedButUnanswered,
    leaks,
    manualLeaks,
  }
}

for (const expand of [false, true]) {
  test(`conversation capture to injected recall@4 (model keyword expansion=${expand})`, async (t) => {
    const sdk = replayModel()
    const model = { id: replay.model, provider: 'replay' }
    const client = createMemoryModelClient({
      getModelRuntime: () => sdk,
      getAuxiliaryModel: async () => model,
      now: () => now,
    })
    const memory = new LocalMemoryRuntime({
      path: ':memory:',
      cwd: paths.drill,
      isProjectless: (cwd) => cwd === paths.chat,
      now: () => now,
      semanticSummarizer: expand ? createSemanticMemorySummarizer({ modelClient: client }) : null,
    })
    const errors = []
    const capture = new ConversationMemoryCapture({
      memory,
      getModelRuntime: () => sdk,
      getAuxiliaryModel: async () => model,
      waitForInitialization: async () => {},
      modelClient: client,
      reportFailure: ({ code }) => errors.push(code),
    })
    try {
      await memory.init()
      for (const [i, { key, workspace, user, assistant }] of turns.entries()) {
        await capture.capture({
          sessionId: key,
          cwd: paths[workspace],
          user,
          assistant,
          recordTurn: true,
          sourceTimestamp: new Date(now - (60 - i) * 60_000).toISOString(),
        })
      }
      await capture.flushIdleSessions()
      const semantic = await memory.drainSemanticQueue()
      assert.deepEqual(errors, [])
      const batchKeys = ['incident', 'noise', 'search']
      assert.deepEqual(sdk.calls.batch.sort(), batchKeys)
      assert.deepEqual(
        sdk.calls.immediate.sort(),
        turns
          .map(({ key }) => key)
          .filter((key) => !batchKeys.includes(key))
          .sort(),
      )

      const stored = memory.listMemories({ spaceId: 'all' })
      assert.ok(
        !stored.some((row) =>
          [row.title, row.content, row.evidence, row.semanticText].some((value) =>
            value.includes('撤回蓝色棋子'),
          ),
        ),
        'one-time passphrase reached long-term recall',
      )
      assert.equal(semantic.ready, expand ? stored.length : 0, 'keywords indexed')
      const storage = auditStorage(memory, stored)
      const recall = await measureRecall(memory, storage)
      const spelling = await measureSpelling(memory)
      const followups = await measureFollowups(memory, renderedFacts)
      t.diagnostic(JSON.stringify({ spelling, followups }))
      assert.ok(
        spelling.some((item) => item.positive && item.corrected),
        'correctly spelled positive control',
      )
      assert.ok(followups.candidateHits.length >= 3, 'explicit-topic follow-ups')
      for (const group of [followups, followups.heldOut]) {
        assert.deepEqual(group.newWrongRows, [], 'new unrelated context after retry')
        assert.deepEqual(group.leaks, [], 'follow-up scope leakage')
      }
      t.diagnostic(
        JSON.stringify({
          captured: [answers.length - storage.missing.length, answers.length],
          ...storage,
          ...recall,
        }),
      )
      assert.deepEqual(storage.missing, [], 'capture')
      assert.deepEqual(storage.misplaced, [], 'storage scope')
      assert.deepEqual(storage.unexpectedWrites, [], 'unwanted extraction')
      assert.deepEqual(recall.leaks, [], 'scope leakage')
      assert.deepEqual(recall.manualLeaks, [], 'manual search scope leakage')
      assert.deepEqual(recall.falseInjections, [], 'negative queries')
      assert.ok(
        recall.recallAt4[0] >= (expand ? 14 : 11),
        `recall@4: ${recall.recallAt4.join('/')}`,
      )

      // 同时竞争的快照事实有五条，走与聊天相同的默认参数，确实触及四条上限。
      const crowded = renderedFacts(
        await memory.relevantContext('演练服 快照', paths.drill, undefined, {
          sessionId: 'new-session',
        }),
      )
      assert.equal(crowded.length, 4)
    } finally {
      await capture.dispose()
      await memory.dispose()
    }
  })
}
