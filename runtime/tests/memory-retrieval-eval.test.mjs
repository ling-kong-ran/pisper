// 检索效果的回归基线。评测集是合成的（见 fixtures/memory-eval-dataset.mjs），只看方向，不代表真实使用；
// 其中 LLM 扩展词是人工按 semantic-memory 的 prompt 写的。下面的下限低于当前实测，只拦明显回退。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import { distractors, negatives, queries, targetRows } from './fixtures/memory-eval-dataset.mjs'

const DAY_MS = 86_400_000

async function withLibrary(size, expansions, run) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-memory-eval-'))
  const cwd = join(directory, 'project')
  await mkdir(cwd)
  const memory = new LocalMemoryRuntime({ path: join(directory, 'memory.sqlite'), cwd })
  try {
    await memory.init()
    const db = memory.requireDb()
    const keys = new Map()
    const stamp = db.prepare(
      'UPDATE memories SET created_at = ?, updated_at = ?, valid_from = ?, last_confirmed_at = ?, semantic_text = ? WHERE id = ?',
    )
    const now = Date.now()
    memory.transaction(() => {
      for (const row of [...targetRows, ...distractors(size)]) {
        const item = memory.remember({
          spaceId: 'global',
          title: row.title,
          content: row.content,
          type: row.type,
          topic: `eval.key.${row.key}`,
          importance: row.importance,
          dedupe: false,
        })
        keys.set(item.id, row.key)
        const at = new Date(now - row.daysAgo * DAY_MS).toISOString()
        stamp.run(at, at, at, at, expansions ? row.exp : '', item.id)
        memory.indexFact(item.id)
      }
    })
    await run(memory, keys)
  } finally {
    await memory.dispose()
    await rm(directory, { recursive: true, force: true })
  }
}

function measure(memory, keys) {
  const ranked = (text) =>
    memory
      .search(text, { spaceIds: ['global'], limit: 10, minScore: 0, trackAccess: false })
      .map((item) => keys.get(item.id))
  const within = (count) =>
    queries.filter(([, text, expected]) => {
      const rank = ranked(text).indexOf(expected)
      return rank >= 0 && rank < count
    }).length
  const staleFirst = queries.filter(([, text, expected, old]) => {
    if (!old) return false
    const order = ranked(text)
    return (
      order.indexOf(old) >= 0 &&
      (order.indexOf(expected) < 0 || order.indexOf(old) < order.indexOf(expected))
    )
  }).length
  return { top1: within(1), top5: within(5), staleFirst }
}

async function injected(memory, keys, text) {
  const { memories } = await memory.relevantContext(text, '', 5, { spaceIds: ['global'] })
  return memories.map((item) => keys.get(item.id))
}

test('a small library keeps the recall it had before the rewrite', async () => {
  await withLibrary(0, true, async (memory, keys) => {
    const result = measure(memory, keys)
    assert.ok(result.top1 >= 38, `top1 ${result.top1}`)
    assert.ok(result.top5 >= 42, `top5 ${result.top5}`)
    assert.equal(result.staleFirst, 0)
  })
})

test('a library of a thousand memories does not lose recall (the old code fell to 14 of 44)', async () => {
  await withLibrary(1000, true, async (memory, keys) => {
    const result = measure(memory, keys)
    assert.ok(result.top1 >= 35, `top1 ${result.top1}`)
    assert.ok(result.top5 >= 40, `top5 ${result.top5}`)
    assert.equal(result.staleFirst, 0)
  })
})

test('without any model, keyword search still finds what shares words and prefers newer facts', async () => {
  await withLibrary(1000, false, async (memory, keys) => {
    const result = measure(memory, keys)
    assert.ok(result.top5 >= 28, `top5 ${result.top5}`)
    assert.ok(result.staleFirst <= 1, `staleFirst ${result.staleFirst}`)
  })
})

test('automatic recall injects the right memory often and unrelated questions never', async () => {
  await withLibrary(1000, true, async (memory, keys) => {
    let kept = 0
    for (const [, text, expected] of queries)
      if ((await injected(memory, keys, text)).includes(expected)) kept += 1
    assert.ok(kept >= 30, `kept ${kept}`)
    for (const text of negatives)
      assert.deepEqual(await injected(memory, keys, text), [], `误注入：${text}`)
  })
})
