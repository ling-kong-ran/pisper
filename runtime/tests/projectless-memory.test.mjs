import assert from 'node:assert/strict'
import { resolve, join } from 'node:path'
import test from 'node:test'
import { isProjectlessDirectory } from '../runtime/projectless-chats.mjs'
import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import { runMemoryConsolidation } from '../services/memory/memory-consolidator.mjs'
import {
  createMemoryConfirmTool,
  createMemoryForgetTool,
  createMemoryOpenTool,
  createMemoryRememberTool,
  createMemorySearchTool,
} from '../tools/app/memory.mjs'

const project = resolve('sample-project')
const root = resolve('sample-data', 'chats')
const chatDir = join(root, 'first')
const otherDir = join(root, 'second')
const now = Date.parse('2026-06-04T12:00:00Z')
const query = 'Mirror schedule'
const fact = { title: query, content: 'Run at nine', topic: 'mirror.schedule', type: 'decision' }
const globalFact = {
  title: 'Reply language',
  content: 'Use French in every chat',
  type: 'preference',
  scope: 'global',
}

test('projectless fact detail links the originating chat while it is in the visible session catalog', async () => {
  const memory = new LocalMemoryRuntime({
    path: ':memory:',
    cwd: chatDir,
    isProjectless: () => true,
  })
  const sessions = new Map([
    ['a', 'Launch timing'],
    ['b', 'Review timing'],
  ])
  let reads = 0
  const facade = Object.assign(Object.create(AgentRuntimeFacade.prototype), {
    memory,
    async listSessions({ ids }) {
      reads++
      assert.equal(ids.length, 1)
      return ids.filter((id) => sessions.has(id)).map((id) => ({ id, name: sessions.get(id) }))
    },
  })
  await memory.init()
  try {
    const a = memory.remember({ ...fact, spaceId: 'chats', sessionId: 'a' })
    const b = memory.remember({ ...fact, spaceId: 'chats', sessionId: 'b' })
    const global = memory.remember({ ...fact, spaceId: 'global' })
    assert.equal((await facade.getMemoryDetail(global.id)).sourceSession, undefined)
    assert.equal(reads, 0)
    assert.equal((await facade.getMemoryDetail(a.id)).sourceSession.sessionName, 'Launch timing')
    sessions.set('b', 'Renamed chat')
    assert.deepEqual((await facade.getMemoryDetail(b.id)).sourceSession, {
      sessionId: 'b',
      sessionName: 'Renamed chat',
      sessionAvailable: true,
    })
    sessions.delete('a')
    const deleted = await facade.getMemoryDetail(a.id)
    assert.equal(deleted.item.id, a.id)
    assert.deepEqual(deleted.sourceSession, {
      sessionId: 'a',
      sessionName: '',
      sessionAvailable: false,
    })
    facade.listSessions = async () => {
      throw new Error('catalog unavailable')
    }
    const offline = await facade.getMemoryDetail(b.id)
    assert.equal(offline.item.id, b.id)
    assert.equal(offline.sourceSession, undefined)
    assert.equal(await facade.getMemoryDetail('missing'), null)
  } finally {
    await memory.dispose()
  }
})

for (const scan of [false, true]) {
  test(`projectless chats share one management space and isolate memory tools and recall (scan=${scan})`, async () => {
    const memory = new LocalMemoryRuntime({
      path: ':memory:',
      cwd: project,
      now: () => now,
      isProjectless: (cwd) => isProjectlessDirectory(root, cwd),
      ...(scan ? { fullTextSearchInitializer: () => false } : {}),
    })
    await memory.init()
    try {
      const toolsFor = (cwd, sessionId) => {
        const context = {
          cwd,
          sessionId,
          memoryRuntime: memory,
          getUserMessage: () => 'remember this preference; forget this memory',
        }
        const call = (tool) => (params) => tool(context).execute('test-call', params)
        return {
          remember: call(createMemoryRememberTool),
          search: call(createMemorySearchTool),
          open: call(createMemoryOpenTool),
          confirm: call(createMemoryConfirmTool),
          forget: call(createMemoryForgetTool),
        }
      }
      const first = toolsFor(chatDir, 'first')
      const second = toolsFor(otherDir, 'second')
      // 两个会话共用 cwd 时也不能合并或互相召回。
      const fork = toolsFor(chatDir, 'fork')
      const read = async (tool, params = { query }) =>
        (await tool.search(params)).details.memories.map((item) => item.id)
      const recall = async (cwd, sessionId) =>
        (await memory.relevantContext(query, cwd, 4, { sessionId })).memories.map((item) => item.id)
      assert.deepEqual(await recall(chatDir, 'first'), [])
      assert.equal(memory.listSpaces().length, 2)
      const a = (await first.remember(fact)).details
      const b = (await second.remember({ ...fact, content: 'Run at noon' })).details
      const c = (await fork.remember(fact)).details
      assert.notEqual(a.id, c.id)
      assert.equal(memory.getMemory(a.id).status, 'active')
      assert.equal(memory.listSpaces().filter((space) => space.kind === 'chats').length, 1)
      assert.equal(memory.listSpaces().filter((space) => space.kind === 'project').length, 1)
      assert.deepEqual(await read(first), [a.id])
      assert.deepEqual(await recall(chatDir, 'fork'), [c.id])
      assert.deepEqual(await recall(project, 'project-session'), [])
      assert.deepEqual(await recall(project, 'first'), [a.id])
      assert.deepEqual(await recall(chatDir, ''), [])
      assert.equal((await first.open({ id: b.id })).details.item, undefined)
      assert.equal((await first.confirm({ id: b.id })).details.id, undefined)
      await first.forget({ id: b.id, userQuote: 'forget this memory' })
      assert.ok(memory.getMemory(b.id))
      const glob = (await first.remember({ ...globalFact, userQuote: 'remember this preference' }))
        .details
      const scopedTask = (
        await first.remember({
          title: 'One-time job',
          content: 'Check today',
          type: 'task',
          scope: 'global',
          userQuote: 'remember this preference',
        })
      ).details
      assert.equal(scopedTask.spaceId, 'chats')
      assert.ok((await read(second, { query: globalFact.title })).includes(glob.id))
      assert.ok(
        (await read(toolsFor(project, 'project-session'), { query: globalFact.title })).includes(
          glob.id,
        ),
      )
      const episodes = []
      for (const [sessionId, cwd] of [
        ['first', chatDir],
        ['second', otherDir],
      ]) {
        for (const day of ['01', '02']) {
          const entry = memory.recordEpisode({
            sessionId,
            cwd,
            spaceId: 'chats',
            user: 'Mirror schedule for this chat',
            at: `2026-06-${day}T10:00:00Z`,
          })
          episodes.push(entry.id)
        }
      }
      const globalFromChat = memory.remember({
        ...globalFact,
        topic: 'user.second.language',
        title: 'Other language',
        content: 'Also fluent in German',
        spaceId: 'global',
        episodeId: episodes[0],
        sessionId: 'first',
      })
      assert.equal((await second.open({ id: globalFromChat.id })).details.episode, undefined)
      assert.equal((await first.open({ id: globalFromChat.id })).details.episode.id, episodes[0])
      memory.setPinned(a.id, true)
      await runMemoryConsolidation(memory.requireStore(), { force: true })
      const digests = memory
        .requireDb()
        .prepare("SELECT id, level, session_id FROM memory_digests WHERE space_id = 'chats'")
        .all()
      assert.equal(digests.length, 2)
      assert.ok(digests.every((d) => d.level === 'session'))
      assert.equal(memory.profileContext(['chats']), '')
      const otherDigest = digests.find((d) => d.session_id === 'second')
      assert.equal((await first.open({ id: otherDigest.id })).details.item, undefined)
      const results = await read(first, {
        query,
        level: 'all',
        since: '2026-06-01',
        until: '2026-06-05',
      })
      assert.ok(results.includes(episodes[0]))
      assert.ok(
        ![b.id, c.id, otherDigest.id, ...episodes.slice(2)].some((id) => results.includes(id)),
      )
      await first.forget({ id: a.id, userQuote: 'forget this memory' })
      assert.equal((await first.remember(fact)).details.outcome, 'forgotten')
      assert.equal((await fork.remember(fact)).details.id, c.id)
    } finally {
      await memory.dispose()
    }
  })
}

test('existing projectless project spaces keep their original facts and forgotten fingerprints', async () => {
  const memory = new LocalMemoryRuntime({
    path: ':memory:',
    cwd: project,
    isProjectless: (cwd) => isProjectlessDirectory(root, cwd),
  })
  await memory.init()
  try {
    const legacy = memory.createSpace({ kind: 'project', rootPath: chatDir, name: 'Older chat' })
    const previous = memory.remember({ ...fact, spaceId: legacy.id })
    assert.equal(await memory.ensureWorkspaceSpace(chatDir), legacy.id)
    assert.ok(
      (await memory.relevantContext(query, chatDir, 4, { sessionId: 'first' })).memories.some(
        (m) => m.id === previous.id,
      ),
    )
    memory.forget(previous.id)
    assert.equal(
      memory.remember({ ...fact, spaceId: await memory.ensureWorkspaceSpace(chatDir) }).outcome,
      'forgotten',
    )
    assert.equal(await memory.ensureWorkspaceSpace(otherDir), 'chats')
    // 模拟回退到旧 Runtime 后，它又为该目录创建了一份项目空间。
    memory.createSpace({ kind: 'project', rootPath: otherDir, name: 'Created on rollback' })
    assert.equal(await memory.ensureWorkspaceSpace(otherDir), 'chats')
    assert.equal(
      memory.remember({ ...fact, spaceId: 'chats', sessionId: 'second' }).outcome,
      'added',
    )
  } finally {
    await memory.dispose()
  }
})
