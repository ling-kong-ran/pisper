import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import { createSemanticMemorySummarizer } from '../services/memory/semantic-memory.mjs'

function remember(memory, id) {
  return memory.remember({
    id,
    spaceId: 'global',
    title: `Title ${id}`,
    content: `Content for ${id}`,
    topic: `fixture.${id}`,
  })
}

test('keyword expansion follows input ids through reordering, missing and duplicate replies', async () => {
  const entries = ['A', 'B', 'C', 'D'].map((title) => ({ title, content: title }))
  const summarizer = createSemanticMemorySummarizer({
    modelClient: {
      available: async () => true,
      complete: async ({ user }) => {
        const inputs = JSON.parse(user.split('\n\n').at(-1))
        assert.deepEqual(
          inputs.map(({ id, title }) => [id, title]),
          [
            [1, 'A'],
            [2, 'B'],
            [3, 'C'],
            [4, 'D'],
          ],
        )
        return {
          text: JSON.stringify([
            { id: 3, keywords: 'synonym C' },
            { id: 1, keywords: 'synonym A' },
            { id: 4, keywords: 'first D' },
            { id: 4, keywords: 'second D' },
            { id: 5, keywords: 'unknown id' },
          ]),
        }
      },
    },
  })
  assert.deepEqual(await summarizer.summarize(entries), ['synonym A', '', 'synonym C', ''])
})

test('unavailable expansion stays pending and a later background tick repairs the same fact', async () => {
  let now = Date.parse('2026-09-01T12:00:00Z')
  let available = false
  let calls = 0
  const semanticSummarizer = createSemanticMemorySummarizer({
    modelClient: {
      available: async () => available,
      complete: async () => {
        calls += 1
        return { text: '[{"id":1,"keywords":"recoveredkeyword"}]' }
      },
    },
  })
  const memory = new LocalMemoryRuntime({
    path: ':memory:',
    cwd: process.cwd(),
    now: () => now,
    semanticSummarizer,
  })
  try {
    await memory.init()
    const item = remember(memory, 'offline')
    await memory.drainSemanticQueue()
    assert.equal(memory.getMemory(item.id).semanticStatus, 'pending')
    assert.equal(memory.search('offline')[0].id, item.id)
    available = true
    await memory.runBackgroundOnce()
    assert.equal(calls, 0)
    now += 2 * 60_000
    await memory.runBackgroundOnce()
    assert.equal(calls, 1)
    assert.equal(memory.getMemory(item.id).semanticStatus, 'ready')
    assert.equal(memory.search('recoveredkeyword')[0].id, item.id)
  } finally {
    await memory.dispose()
  }
})

test('opening an old library backfills empty ready rows in bounded batches and rotates past failures', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-semantic-'))
  const options = { path: join(directory, 'memory.sqlite'), cwd: directory }
  const memory = new LocalMemoryRuntime(options)
  let reopened
  try {
    await memory.init()
    for (let i = 0; i < 20; i++) remember(memory, `legacy${i}`)
    memory.requireDb().exec("UPDATE memories SET semantic_status = 'ready'")
    await memory.dispose()
    const batches = []
    let now = Date.now()
    reopened = new LocalMemoryRuntime({
      ...options,
      now: () => now,
      semanticSummarizer: {
        async summarize(entries) {
          batches.push(entries.length)
          if (batches.length === 1) throw new Error('private model error detail')
          return entries.map(() => 'indexed')
        },
      },
    })
    await reopened.init()
    await reopened.drainSemanticQueue()
    assert.deepEqual(batches, [16])
    assert.equal(reopened.semanticStatus().failed, 16)
    assert.equal(reopened.semanticStatus().pending, 4)
    assert.equal(reopened.semanticStatus().error, 'semantic_expansion_failed')
    await reopened.runBackgroundOnce()
    assert.deepEqual(batches, [16, 4])
    assert.equal(reopened.semanticStatus().ready, 4)
    now += 2 * 60_000
    await reopened.runBackgroundOnce()
    assert.deepEqual(batches, [16, 4, 16])
    assert.equal(reopened.semanticStatus().ready, 20)
  } finally {
    await reopened?.dispose()
    await memory.dispose()
    await rm(directory, { recursive: true, force: true })
  }
})

test('a late expansion cannot index old text after an edit, and metadata edits retain the result', async () => {
  const original = Promise.withResolvers()
  const revised = Promise.withResolvers()
  const entered = Promise.withResolvers()
  const reentered = Promise.withResolvers()
  const memory = new LocalMemoryRuntime({
    path: ':memory:',
    cwd: process.cwd(),
    semanticSummarizer: {
      summarize([entry]) {
        if (entry.content === 'Revised content') {
          reentered.resolve()
          return revised.promise
        }
        entered.resolve()
        return original.promise
      },
    },
  })
  try {
    await memory.init()
    const item = remember(memory, 'editing')
    await entered.promise
    memory.updateMemory(item.id, { content: 'Revised content' })
    original.resolve(['obsoletekeyword'])
    await reentered.promise
    assert.equal(memory.getMemory(item.id).semanticStatus, 'pending')
    assert.deepEqual(memory.search('obsoletekeyword'), [])
    revised.resolve(['correctkeyword'])
    await memory.drainSemanticQueue()
    memory.updateMemory(item.id, { importance: 0.9 })
    assert.equal(memory.getMemory(item.id).semanticStatus, 'ready')
    assert.equal(memory.search('correctkeyword')[0].id, item.id)
  } finally {
    await memory.dispose()
  }
})

test('backfill still expands the version in effect until its scheduled successor takes over', async () => {
  const memory = new LocalMemoryRuntime({
    path: ':memory:',
    cwd: process.cwd(),
    now: () => Date.parse('2026-11-23T00:00:00Z'),
  })
  try {
    await memory.init()
    const base = { spaceId: 'global', title: 'Mirror job', topic: 'mirror.schedule' }
    const current = memory.remember({ ...base, content: 'Nine oclock' })
    memory.remember({ ...base, content: 'Midnight', validFrom: '2026-12-01T00:00:00Z' })
    memory.setSemanticSummarizer({
      summarize: async (entries) =>
        entries.map((entry) => (entry.content === 'Nine oclock' ? 'dayjob' : 'nightjob')),
    })
    await memory.drainSemanticQueue()
    assert.equal(memory.semanticStatus().ready, 2)
    assert.equal(memory.search('dayjob')[0].id, current.id)
    assert.deepEqual(memory.search('nightjob'), [])
  } finally {
    await memory.dispose()
  }
})

test('keyword requests stay within one memory space or projectless session, including retries', async () => {
  const memory = new LocalMemoryRuntime({
    path: ':memory:',
    cwd: process.cwd(),
    isProjectless: () => true,
  })
  const calls = []
  const add = (spaceId, sessionId, title) =>
    memory.remember({ spaceId, sessionId, title, content: title, topic: `fixture.${title}` })
  try {
    await memory.init()
    add('chats', 'first', 'first-a')
    add('chats', 'second', 'second-b')
    add('global', '', 'global-c')
    add('chats', 'first', 'first-d')
    memory.setSemanticSummarizer({
      summarize: async (entries) => {
        calls.push(entries.map((entry) => entry.title))
        return entries.map((entry) => `expanded ${entry.title}`)
      },
    })
    await memory.drainSemanticQueue()
    assert.equal(calls.length, 1)
    await memory.runBackgroundOnce()
    assert.equal(calls.length, 2)
    await memory.runBackgroundOnce()
    assert.equal(calls.length, 3)
    assert.equal(calls.flat().length, 4)
    add('chats', 'first', 'first-e')
    add('chats', 'second', 'second-f')
    add('global', '', 'global-g')
    await memory.drainSemanticQueue()
    assert.equal(calls.flat().length, 7)
    for (const titles of calls)
      assert.equal(new Set(titles.map((title) => title.split('-')[0])).size, 1)
  } finally {
    await memory.dispose()
  }
})
