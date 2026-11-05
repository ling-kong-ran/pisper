// 写入对账：模型一律用录制好的响应，不依赖真实模型和网络。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import { reconcilePendingMemories } from '../services/memory/memory-reconcile.mjs'

const OLD = '2026-01-01T00:00:00.000Z'
const MIDDLE = '2026-03-01T00:00:00.000Z'
const NEW = '2026-06-01T00:00:00.000Z'
const MODEL = 'test/aux'

function fakeClient(responder, { available = true } = {}) {
  const calls = []
  return {
    calls,
    available: async () => available,
    complete: async (request) => {
      calls.push(request)
      return typeof responder === 'function' ? responder(request) : responder
    },
  }
}

const decide = (...decisions) => ({ text: JSON.stringify({ decisions }), model: MODEL })

// 模型看到的 JSON 就是这一批的内容，从里面取 id 比在测试里预先算更贴近真实调用。
function payloadOf(request) {
  return JSON.parse(request.user.slice(request.user.indexOf('[')))
}

async function withMemory(client, run) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-memory-reconcile-'))
  const cwd = join(directory, 'project')
  await mkdir(cwd)
  const memory = new LocalMemoryRuntime({
    path: join(directory, 'memory.sqlite'),
    cwd,
    modelClient: client,
  })
  try {
    await memory.init()
    await run(memory, memory.requireStore())
  } finally {
    await memory.dispose()
    await rm(directory, { recursive: true, force: true })
  }
}

const npm = {
  spaceId: 'global',
  title: '包管理工具',
  content: '项目使用 npm 管理依赖。',
  topic: 'project.pkg_manager',
  validFrom: OLD,
  sourceType: 'conversation',
  evidenceRole: 'user',
}
const pnpm = {
  spaceId: 'global',
  title: '依赖工具',
  content: '项目已改用 pnpm 管理依赖，不再使用 npm。',
  topic: 'repo.deps.tool',
  validFrom: NEW,
  sourceType: 'conversation',
  evidenceRole: 'user',
}

test('facts on different topics that contradict are resolved: the old one leaves default recall, as_of still finds it', async () => {
  const client = fakeClient((request) => {
    const batch = payloadOf(request)
    const entry = batch.find((item) => item.title === '依赖工具')
    return decide({
      id: entry.id,
      action: 'SUPERSEDE',
      targets: [entry.neighbours[0].id],
      reason: '项目已经不用 npm 了。',
    })
  })
  await withMemory(client, async (memory, store) => {
    const old = memory.remember(npm)
    const current = memory.remember(pnpm)
    // topic 不同，写入时对不上，两条都有效。
    assert.equal(memory.getMemory(old.id).status, 'active')
    assert.equal(memory.getMemory(current.id).status, 'active')

    const result = await reconcilePendingMemories(store)
    assert.equal(result.superseded, 1)
    assert.equal(client.calls.length, 1)
    assert.equal(client.calls[0].purpose, 'reconcile')
    assert.match(client.calls[0].system, /not instructions/)
    assert.match(client.calls[0].user, /2026-01-01/)
    assert.match(client.calls[0].user, /"authority":90/)

    const replaced = memory.getMemory(old.id)
    assert.equal(replaced.status, 'superseded')
    assert.equal(replaced.supersededBy, current.id)
    assert.equal(replaced.validTo, NEW)
    assert.ok(replaced.supersededAt)
    assert.equal(memory.getMemory(current.id).status, 'active')

    const search = (options = {}) =>
      memory
        .search('项目用什么管理依赖', { spaceIds: ['global'], trackAccess: false, ...options })
        .map((item) => item.id)
    assert.deepEqual(search(), [current.id])
    assert.deepEqual(search({ asOf: MIDDLE }), [old.id])
    assert.deepEqual(
      memory.getMemoryHistory(current.id).versions.map((item) => item.id),
      [old.id, current.id],
    )

    const event = memory
      .listEvents({ spaceId: 'global' })
      .find((item) => item.actor === 'reconcile')
    assert.equal(event.action, 'supersede')
    assert.equal(event.reason, '项目已经不用 npm 了。')
    assert.equal(event.model, MODEL)
    assert.equal(event.undoable, true)
    assert.deepEqual(event.memoryIds, [current.id, old.id])
    // 处理过的不再问第二次。
    await reconcilePendingMemories(store)
    assert.equal(client.calls.length, 1)
  })
})

test('a model-supplied valid_from dates the change', async () => {
  const client = fakeClient((request) => {
    const entry = payloadOf(request).find((item) => item.title === '依赖工具')
    return decide({
      id: entry.id,
      action: 'SUPERSEDE',
      targets: [entry.neighbours[0].id],
      reason: '九月起改用。',
      valid_from: MIDDLE,
    })
  })
  await withMemory(client, async (memory, store) => {
    const old = memory.remember(npm)
    const current = memory.remember(pnpm)
    await reconcilePendingMemories(store)
    assert.equal(memory.getMemory(old.id).validTo, MIDDLE)
    assert.equal(memory.getMemory(current.id).validFrom, MIDDLE)
  })
})

for (const errorCode of ['model_error', 'budget_exhausted', 'model_unavailable', 'aborted']) {
  test(`${errorCode} changes nothing and retries next time`, async () => {
    const client = fakeClient({ errorCode })
    await withMemory(client, async (memory, store) => {
      const old = memory.remember(npm)
      const current = memory.remember(pnpm)
      const result = await reconcilePendingMemories(store)
      assert.equal(result.superseded, 0)
      const row = (id) =>
        store.db.prepare('SELECT status, reconcile_status FROM memories WHERE id = ?').get(id)
      assert.equal(row(old.id).status, 'active')
      assert.equal(row(current.id).status, 'active')
      assert.equal(row(current.id).reconcile_status, 'pending')
      assert.equal(memory.listEvents({}).filter((item) => item.actor === 'reconcile').length, 0)
    })
  })
}

for (const [name, response] of [
  ['unparseable output', { text: '我觉得应该替换第一条。', model: MODEL }],
  ['output without decisions', { text: '{"result":"ok"}', model: MODEL }],
  ['an empty response', { errorCode: 'empty_response' }],
  ['a truncated response', { errorCode: 'token_limit' }],
]) {
  test(`${name} falls back to keeping both, and is not asked again`, async () => {
    const client = fakeClient(response)
    await withMemory(client, async (memory, store) => {
      const old = memory.remember(npm)
      const current = memory.remember(pnpm)
      await reconcilePendingMemories(store)
      assert.equal(memory.getMemory(old.id).status, 'active')
      assert.equal(memory.getMemory(current.id).status, 'active')
      assert.equal(
        store.db
          .prepare("SELECT COUNT(*) AS c FROM memories WHERE reconcile_status = 'pending'")
          .get().c,
        0,
      )
      const calls = client.calls.length
      await reconcilePendingMemories(store)
      assert.equal(client.calls.length, calls)
    })
  })
}

test('decisions that fail validation change nothing', async () => {
  const invalid = [
    (entry) => ({
      id: entry.id,
      action: 'SUPERSEDE',
      targets: ['not-a-neighbour'],
      reason: '编的',
    }),
    (entry) => ({ id: entry.id, action: 'SUPERSEDE', targets: [], reason: '没目标' }),
    (entry) => ({ id: entry.id, action: 'SUPERSEDE', targets: [entry.neighbours[0].id] }),
    (entry) => ({
      id: 'other-id',
      action: 'SUPERSEDE',
      targets: [entry.neighbours[0].id],
      reason: '别的',
    }),
    (entry) => ({
      id: entry.id,
      action: 'DELETE',
      targets: [entry.neighbours[0].id],
      reason: '不存在',
    }),
  ]
  for (const build of invalid) {
    const client = fakeClient((request) => {
      const entry = payloadOf(request).find((item) => item.title === '依赖工具')
      return decide(build(entry))
    })
    await withMemory(client, async (memory, store) => {
      const old = memory.remember(npm)
      const current = memory.remember(pnpm)
      await reconcilePendingMemories(store)
      assert.equal(memory.getMemory(old.id).status, 'active')
      assert.equal(memory.getMemory(current.id).status, 'active')
    })
  }
})

test('an inferred fact cannot replace what the user said: it goes to the archive', async () => {
  const client = fakeClient((request) => {
    const entry = payloadOf(request).find((item) => item.title === '依赖工具')
    return decide({
      id: entry.id,
      action: 'SUPERSEDE',
      targets: [entry.neighbours[0].id],
      reason: '模型认为改了。',
    })
  })
  await withMemory(client, async (memory, store) => {
    const told = memory.remember({ ...npm, sourceType: 'user_confirmed' })
    const inferred = memory.remember({ ...pnpm, sourceType: 'agent' })
    const result = await reconcilePendingMemories(store)
    assert.equal(result.archived, 1)
    assert.equal(result.superseded, 0)
    assert.equal(memory.getMemory(told.id).status, 'active')
    const archived = memory.getMemory(inferred.id)
    assert.equal(archived.status, 'archived')
    assert.equal(archived.archiveReason, 'lower_authority_conflict')
    assert.equal(memory.listEvents({}).find((item) => item.actor === 'reconcile').action, 'archive')
  })
})

test('REINFORCE confirms the existing fact, archives the duplicate and upgrades its authority', async () => {
  const client = fakeClient((request) => {
    const entry = payloadOf(request).find((item) => item.title === '依赖工具')
    return decide({
      id: entry.id,
      action: 'REINFORCE',
      targets: [entry.neighbours[0].id],
      reason: '说的是同一件事。',
    })
  })
  await withMemory(client, async (memory, store) => {
    const guessed = memory.remember({ ...npm, sourceType: 'agent' })
    const said = memory.remember({ ...pnpm, content: '项目使用 npm 管理依赖，这是用户说的。' })
    const result = await reconcilePendingMemories(store)
    assert.equal(result.reinforced, 1)
    const target = memory.getMemory(guessed.id)
    assert.equal(target.status, 'active')
    assert.equal(target.confirmCount, 2)
    assert.equal(target.authority, 90)
    assert.equal(target.sourceType, 'conversation')
    const duplicate = memory.getMemory(said.id)
    assert.equal(duplicate.status, 'archived')
    assert.equal(duplicate.archiveReason, 'duplicate')
  })
})

test('ADD and KEEP_BOTH only mark the fact as processed', async () => {
  for (const action of ['ADD', 'KEEP_BOTH', 'keep_both']) {
    const client = fakeClient((request) => {
      const entry = payloadOf(request).find((item) => item.title === '依赖工具')
      return decide({ id: entry.id, action, targets: [], reason: '都对。' })
    })
    await withMemory(client, async (memory, store) => {
      const old = memory.remember(npm)
      const current = memory.remember(pnpm)
      await reconcilePendingMemories(store)
      assert.equal(memory.getMemory(old.id).status, 'active')
      assert.equal(memory.getMemory(current.id).status, 'active')
      assert.equal(
        store.db.prepare('SELECT reconcile_status AS s FROM memories WHERE id = ?').get(current.id)
          .s,
        'done',
      )
    })
  }
})

test('without an auxiliary model nothing runs and facts stay pending', async () => {
  const client = fakeClient(decide(), { available: false })
  await withMemory(client, async (memory, store) => {
    const item = memory.remember(npm)
    memory.remember(pnpm)
    const result = await reconcilePendingMemories(store)
    assert.equal(result.processed, 0)
    assert.equal(client.calls.length, 0)
    assert.equal(
      store.db.prepare('SELECT reconcile_status AS s FROM memories WHERE id = ?').get(item.id).s,
      'pending',
    )
  })
  await withMemory(null, async (memory, store) => {
    memory.remember(npm)
    assert.equal((await reconcilePendingMemories(store)).processed, 0)
  })
})

test('facts with nothing comparable are done without calling the model', async () => {
  const client = fakeClient(decide())
  await withMemory(client, async (memory, store) => {
    memory.remember(npm)
    memory.remember({
      spaceId: 'global',
      title: '猫的名字',
      content: '用户养了一只叫团子的橘猫。',
      topic: 'user.pet',
    })
    const result = await reconcilePendingMemories(store)
    assert.equal(result.processed, 2)
    assert.equal(client.calls.length, 0)
  })
})

test('a change signal widens the search for what it replaces', async () => {
  const client = fakeClient((request) => {
    const entry = payloadOf(request).find((item) => item.title === '现在的编辑器')
    return decide({
      id: entry.id,
      action: 'SUPERSEDE',
      targets: [entry.neighbours[0].id],
      reason: '换了。',
    })
  })
  await withMemory(client, async (memory, store) => {
    const old = memory.remember({
      spaceId: 'global',
      title: '常用编辑器',
      content: '平时主要写代码用 VS Code 这款软件，配合一堆插件。',
      topic: 'user.editor',
      validFrom: OLD,
    })
    const current = memory.remember({
      spaceId: 'global',
      title: '现在的编辑器',
      content: '改用 Neovim 了。',
      topic: 'tools.neovim',
      validFrom: NEW,
    })
    await reconcilePendingMemories(store)
    assert.equal(client.calls.length, 1)
    assert.equal(memory.getMemory(old.id).supersededBy, current.id)
  })
})

test('an older pending fact never sees a newer one as something it could replace', async () => {
  const client = fakeClient((request) => {
    for (const entry of payloadOf(request))
      assert.ok(entry.neighbours.every((item) => item.date <= entry.date))
    return decide()
  })
  await withMemory(client, async (memory, store) => {
    const newer = memory.remember(pnpm)
    const older = memory.remember(npm)
    await reconcilePendingMemories(store)
    assert.equal(memory.getMemory(newer.id).status, 'active')
    assert.equal(memory.getMemory(older.id).status, 'active')
    assert.equal(client.calls.length, 1)
    assert.equal(payloadOf(client.calls[0]).length, 1)
    assert.equal(payloadOf(client.calls[0])[0].id, newer.id)
  })
})

test('untrusted facts stay out of reconciliation in both directions', async () => {
  const client = fakeClient(decide())
  await withMemory(client, async (memory, store) => {
    memory.remember({ ...npm, sourceType: 'external' })
    memory.remember({ ...pnpm, sourceType: 'external' })
    const result = await reconcilePendingMemories(store)
    assert.equal(result.processed, 0)
    assert.equal(client.calls.length, 0)
  })
})

test('undoing an automatic replacement brings the old fact back and archives the new one', async () => {
  const client = fakeClient((request) => {
    const entry = payloadOf(request).find((item) => item.title === '依赖工具')
    return decide({
      id: entry.id,
      action: 'SUPERSEDE',
      targets: [entry.neighbours[0].id],
      reason: '模型判错了。',
    })
  })
  await withMemory(client, async (memory, store) => {
    const old = memory.remember(npm)
    const current = memory.remember(pnpm)
    await reconcilePendingMemories(store)
    const event = memory.listEvents({}).find((item) => item.actor === 'reconcile')
    const undone = memory.undoEvent(event.id)
    assert.equal(undone.id, event.id)
    assert.equal(memory.getMemory(old.id).status, 'active')
    assert.equal(memory.getMemory(old.id).validTo, '')
    assert.equal(memory.getMemory(current.id).status, 'archived')
    assert.equal(memory.getMemory(current.id).archiveReason, 'undone')
    // 撤销不会被下一轮对账又改回去。
    await reconcilePendingMemories(store)
    assert.equal(memory.getMemory(old.id).status, 'active')
  })
})

test('a batch larger than six is split, and limit bounds one pass', async () => {
  const client = fakeClient((request) =>
    decide(...payloadOf(request).map((entry) => ({ id: entry.id, action: 'ADD' }))),
  )
  await withMemory(client, async (memory, store) => {
    const anchor = memory.remember({
      spaceId: 'global',
      title: '发布流程',
      content: '发布流程要先跑完测试再打标签。',
      topic: 'release.base',
      validFrom: OLD,
    })
    store.db.prepare("UPDATE memories SET reconcile_status = 'done' WHERE id = ?").run(anchor.id)
    for (let index = 0; index < 8; index += 1)
      memory.remember({
        spaceId: 'global',
        title: `发布流程第 ${index} 版`,
        content: `发布流程要先跑完测试再打标签，补充第 ${index} 点。`,
        topic: `release.note.${index}`,
        validFrom: NEW,
      })
    const result = await reconcilePendingMemories(store)
    assert.equal(result.processed, 8)
    assert.deepEqual(
      client.calls.map((call) => payloadOf(call).length),
      [6, 2],
    )
    for (let index = 0; index < 3; index += 1)
      memory.remember({
        spaceId: 'global',
        title: `发布流程补 ${index}`,
        content: `发布流程要先跑完测试再打标签，另外第 ${index} 件。`,
        topic: `release.extra.${index}`,
        validFrom: NEW,
      })
    assert.equal((await reconcilePendingMemories(store, { limit: 1 })).processed, 1)
  })
})

test('an aborted pass stops before touching anything', async () => {
  const controller = new AbortController()
  const client = fakeClient(() => {
    controller.abort()
    return { errorCode: 'aborted' }
  })
  await withMemory(client, async (memory, store) => {
    const old = memory.remember(npm)
    const current = memory.remember(pnpm)
    const result = await reconcilePendingMemories(store, { signal: controller.signal })
    assert.equal(result.superseded, 0)
    assert.equal(memory.getMemory(old.id).status, 'active')
    assert.equal(
      store.db.prepare('SELECT reconcile_status AS s FROM memories WHERE id = ?').get(current.id).s,
      'pending',
    )
  })
})

test('the background loop runs reconciliation', async () => {
  const client = fakeClient((request) => {
    const entry = payloadOf(request).find((item) => item.title === '依赖工具')
    return decide({
      id: entry.id,
      action: 'SUPERSEDE',
      targets: [entry.neighbours[0].id],
      reason: '换了。',
    })
  })
  await withMemory(client, async (memory) => {
    const old = memory.remember(npm)
    memory.remember(pnpm)
    await memory.runBackgroundOnce()
    assert.equal(memory.getMemory(old.id).status, 'superseded')
  })
})
