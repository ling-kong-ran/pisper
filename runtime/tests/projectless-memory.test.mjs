import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
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

test('legacy per-chat project spaces are merged into the single chats space on startup', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-memory-legacy-chats-'))
  const path = join(directory, 'memory.sqlite')
  const chatRoot = join(directory, 'chats', '2026-11-09-AAA')
  const realProject = join(directory, 'real-project')
  await mkdir(chatRoot, { recursive: true })
  await mkdir(realProject, { recursive: true })
  // 先用旧语义（目录各成一个项目空间）写两条记忆，再按新语义重开库，触发归并。
  const before = new LocalMemoryRuntime({ path, cwd: chatRoot, isProjectless: () => false })
  await before.init()
  let legacyFact
  let projectFact
  let renamedFact
  try {
    const legacySpace = await before.ensureWorkspaceSpace(chatRoot)
    const realSpace = await before.ensureWorkspaceSpace(realProject)
    assert.notEqual(legacySpace, realSpace)
    legacyFact = before.remember({ ...fact, spaceId: legacySpace })
    projectFact = before.remember({ ...fact, spaceId: realSpace })
    // 用户改过名的空间含义不明，按旧语义保留，不参与归并。
    const renamed = before.createSpace({
      kind: 'project',
      rootPath: join(directory, 'chats', '2026-11-10-BBB'),
      name: 'Renamed chat',
    })
    renamedFact = before.remember({ ...fact, spaceId: renamed.id })
    assert.equal(legacyFact.sessionId, '')
    const db = before.requireDb()
    db.prepare(
      `INSERT INTO memory_digests (id, space_id, level, window_start, window_end, text, source_ids, dirty, generated_at, session_id)
       VALUES (?, ?, 'day', ?, ?, ?, ?, 0, ?, '')`,
    ).run(
      'legacy-digest',
      legacySpace,
      '2026-11-09T00:00:00.000Z',
      '2026-11-10T00:00:00.000Z',
      'legacy day digest',
      JSON.stringify([legacyFact.id]),
      '2026-11-10T00:00:00.000Z',
    )
  } finally {
    await before.dispose()
  }

  const after = new LocalMemoryRuntime({ path, cwd: chatRoot, isProjectless: () => true })
  await after.init()
  try {
    const merged = after.getMemory(legacyFact.id)
    assert.equal(merged.spaceId, 'chats')
    // 没记 session 的老记忆用原会话目录名兜底，否则并进来后哪个会话都查不到。
    assert.equal(merged.sessionId, '2026-11-09-AAA')
    const spaces = after.listSpaces().map((space) => space.id)
    assert.ok(spaces.includes('chats'))
    const kept = new Set([projectFact.spaceId, renamedFact.spaceId])
    assert.ok(!spaces.some((id) => id.startsWith('project-') && !kept.has(id)))
    // 真项目空间和用户改过名的空间都保持原样。
    assert.equal(after.getMemory(projectFact.id).spaceId, projectFact.spaceId)
    assert.equal(after.getMemory(renamedFact.id).spaceId, renamedFact.spaceId)
    assert.ok(spaces.includes(renamedFact.spaceId))
    const digest = after
      .requireDb()
      .prepare("SELECT space_id AS spaceId, dirty FROM memory_digests WHERE id = 'legacy-digest'")
      .get()
    assert.equal(digest.spaceId, 'chats')
    // 来源事实换了空间，schema 触发器会把旧摘要置 2（重算前不得参与检索）。
    assert.equal(digest.dirty, 2)
  } finally {
    await after.dispose()
    await rm(directory, { recursive: true, force: true })
  }
})
