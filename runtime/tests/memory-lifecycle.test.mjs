// 记忆生命周期：到期、引用失效、长期不用、保留期，以及确认、恢复和撤销。时间用可控时钟，不靠等待。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import { computeAnchorHash } from '../services/memory/memory-lifecycle.mjs'

const DAY_MS = 86_400_000
const START = Date.parse('2026-06-01T00:00:00.000Z')

async function withMemory(run) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-memory-lifecycle-'))
  const cwd = join(directory, 'project')
  await mkdir(cwd)
  const clock = { now: START }
  const memory = new LocalMemoryRuntime({
    path: join(directory, 'memory.sqlite'),
    cwd,
    now: () => clock.now,
  })
  const sweep = () => memory.cleanupRetention({ force: true })
  try {
    await memory.init()
    await run({ memory, clock, cwd, sweep, db: memory.requireDb() })
  } finally {
    await memory.dispose()
    await rm(directory, { recursive: true, force: true })
  }
}

const fact = (extra = {}) => ({
  spaceId: 'global',
  title: '部署平台',
  content: '项目部署到阿里云。',
  topic: 'project.deploy.platform',
  ...extra,
})

test('computeAnchorHash reads only small regular files and resolves against cwd', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-memory-anchor-'))
  try {
    await writeFile(join(directory, 'a.md'), 'hello')
    await writeFile(join(directory, 'big.bin'), Buffer.alloc(2 * 1024 * 1024 + 1))
    const hash = computeAnchorHash('a.md', directory)
    assert.match(hash, /^[0-9a-f]{16}$/)
    assert.equal(computeAnchorHash(join(directory, 'a.md')), hash)
    assert.equal(computeAnchorHash('a.md'), '')
    assert.equal(computeAnchorHash('', directory), '')
    assert.equal(computeAnchorHash('missing.md', directory), '')
    assert.equal(computeAnchorHash(directory), '')
    assert.equal(computeAnchorHash('big.bin', directory), '')
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

test('expired memories move to the archive instead of being deleted, and undoing keeps them', async () => {
  await withMemory(async ({ memory, clock, sweep }) => {
    const item = memory.remember(fact({ expiresAt: new Date(START + DAY_MS).toISOString() }))
    sweep()
    assert.equal(memory.getMemory(item.id).status, 'active')
    clock.now += 2 * DAY_MS
    sweep()
    const archived = memory.getMemory(item.id)
    assert.equal(archived.status, 'archived')
    assert.equal(archived.archiveReason, 'expired')
    assert.equal(archived.validTo, archived.expiresAt)
    assert.equal(memory.search('部署平台', { spaceIds: ['global'], trackAccess: false }).length, 0)
    const event = memory.listEvents({}).find((item) => item.action === 'archive')
    assert.equal(event.reason, 'expired')
    assert.equal(event.undoable, true)
    memory.undoEvent(event.id)
    assert.equal(memory.getMemory(item.id).status, 'active')
    // 否则下一轮清扫又会把它归档。
    sweep()
    assert.equal(memory.getMemory(item.id).status, 'active')
  })
})

test('current search, automatic recall and the profile switch at each effective date', async () => {
  await withMemory(async ({ memory, clock, cwd }) => {
    const iso = (day) => new Date(START + day * DAY_MS).toISOString()
    const add = (day, content) =>
      memory.remember(fact({ validFrom: iso(day), content, type: 'decision' }))
    const first = add(0, '原先在甲区运行。')
    const last = add(2, '改为在丙区运行。')
    const middle = add(1, '中间一天在乙区运行。')
    // 较晚写进来的回溯事实，以及提前写下的两次变更，都要按生效日接进链。
    const early = add(-1, '最初在丁区运行。')
    memory.updateMemory(first.id, { importance: 0.8 })
    assert.deepEqual(
      memory.getMemoryHistory(last.id).versions.map((row) => row.id),
      [early.id, first.id, middle.id, last.id],
    )
    for (const [day, expected] of [
      [-1, early],
      [0, first],
      [1, middle],
      [2, last],
    ]) {
      clock.now = Date.parse(iso(day))
      const search = (options = {}) =>
        memory.search('部署平台', { cwd, ...options }).map((row) => row.id)
      assert.deepEqual(search(), [expected.id])
      assert.deepEqual(search({ asOf: iso(day) }), [expected.id])
      const recall = await memory.relevantContext('部署平台', cwd)
      assert.deepEqual(
        recall.memories.map((row) => [row.id, row.status]),
        [[expected.id, 'active']],
      )
      const profile = memory.profileContext(['global'])
      for (const fact of [early, first, middle, last])
        assert.equal(profile.includes(fact.content), fact.id === expected.id)
      assert.deepEqual(
        memory.listMemories({ spaceId: 'global' }).map((row) => row.id),
        [expected.id],
      )
    }
  })
})

test('editing the effective date moves the handover and undo restores both boundaries', async () => {
  await withMemory(async ({ memory, clock }) => {
    const old = memory.remember(fact())
    const scheduled = new Date(START + DAY_MS).toISOString()
    const later = new Date(START + 2 * DAY_MS).toISOString()
    const next = memory.remember(fact({ content: '项目部署到上海机房。', validFrom: scheduled }))
    memory.updateMemory(next.id, { validFrom: later })
    assert.equal(memory.getMemory(old.id).validTo, later)
    clock.now += DAY_MS
    const search = () => memory.search('部署平台').map((row) => row.id)
    assert.deepEqual(search(), [old.id])
    const event = memory.listEvents().find((row) => row.action === 'edit')
    memory.undoEvent(event.id)
    assert.equal(memory.getMemory(old.id).validTo, scheduled)
    assert.deepEqual(search(), [next.id])
  })
})

test('expiry takes effect on read before the background sweep; history survives the sweep', async () => {
  await withMemory(async ({ memory, clock, cwd }) => {
    const before = new Date(START).toISOString()
    const until = new Date(START + DAY_MS).toISOString()
    const expired = memory.remember(fact({ expiresAt: until, pinned: true }))
    const search = (asOf) => memory.search('部署平台', { cwd, asOf }).map((row) => row.id)
    assert.deepEqual(search(), [expired.id])
    assert.ok(memory.profileContext(['global']).includes(expired.content))
    clock.now += DAY_MS
    assert.equal(memory.getMemory(expired.id).status, 'active')
    assert.deepEqual(search(), [])
    assert.deepEqual(search(until), [])
    assert.deepEqual(search(before), [expired.id])
    assert.equal((await memory.relevantContext('以前的部署平台', cwd)).text, '')
    assert.equal(memory.profileContext(['global']), '')
    await memory.runBackgroundOnce()
    assert.equal(memory.getMemory(expired.id).archiveReason, 'expired')
    assert.deepEqual(search(before), [expired.id])
    assert.deepEqual(search(until), [])
    assert.equal(memory.statusCounts('global').current, 0)
  })
})

test('a changed or missing referenced file flags the memory stale in the same result', async () => {
  await withMemory(async ({ memory, clock, cwd }) => {
    const path = join(cwd, 'deploy.md')
    await writeFile(path, 'v1')
    const item = memory.remember(fact({ sourcePath: 'deploy.md', cwd }))
    const search = () => memory.search('部署平台', { spaceIds: ['global'], trackAccess: false })[0]
    assert.equal(search().status, 'active')
    await writeFile(path, 'v2')
    // 一小时内查过的不再读文件。
    assert.equal(search().status, 'active')
    clock.now += 2 * 3_600_000
    const flagged = search()
    assert.equal(flagged.id, item.id)
    assert.equal(flagged.status, 'stale')
    assert.equal(memory.getMemory(item.id).status, 'stale')
    const event = memory.listEvents({}).find((entry) => entry.action === 'stale')
    assert.equal(event.reason, 'anchor_changed')

    // 确认后恢复，锚点刷新：同样的内容不会再被标记。
    const confirmed = memory.confirmMemory(item.id)
    assert.equal(confirmed.status, 'active')
    assert.equal(confirmed.confirmCount, 2)
    clock.now += 2 * 3_600_000
    assert.equal(search().status, 'active')

    await rm(path)
    clock.now += 2 * 3_600_000
    assert.equal(search().status, 'stale')
    assert.equal(
      memory.listEvents({}).find((entry) => entry.action === 'stale').reason,
      'anchor_missing',
    )
  })
})

test('auto recall marks a stale memory as such, and memories without a file are not checked', async () => {
  await withMemory(async ({ memory, clock, cwd, db }) => {
    const path = join(cwd, 'notes.md')
    await writeFile(path, 'v1')
    memory.remember(fact({ sourcePath: 'notes.md', cwd }))
    const plain = memory.remember(
      fact({ title: '发版限制', content: '周五不发版。', topic: 'a.b' }),
    )
    await writeFile(path, 'v2')
    clock.now += 2 * 3_600_000
    const { text } = await memory.relevantContext('部署平台是哪个', cwd, 4, {
      spaceIds: ['global'],
    })
    assert.match(text, /status="stale"/)
    assert.equal(
      db.prepare('SELECT anchor_checked_at AS at FROM memories WHERE id = ?').get(plain.id).at,
      null,
    )
  })
})

test('one retrieval reads at most eight files', async () => {
  await withMemory(async ({ memory, clock, cwd, db }) => {
    for (let index = 0; index < 12; index += 1) {
      await writeFile(join(cwd, `f${index}.md`), 'v1')
      memory.remember({
        spaceId: 'global',
        title: `部署说明 ${index}`,
        content: `部署平台第 ${index} 条说明。`,
        topic: `deploy.note.${index}`,
        sourcePath: `f${index}.md`,
        cwd,
      })
    }
    clock.now += 2 * 3_600_000
    memory.search('部署平台说明', { spaceIds: ['global'], limit: 12, trackAccess: false })
    const checked = db
      .prepare('SELECT COUNT(*) AS c FROM memories WHERE anchor_checked_at IS NOT NULL')
      .get().c
    assert.equal(checked, 8)
  })
})

test('restoring an archived memory brings it back and replaces what took its place', async () => {
  await withMemory(async ({ memory, db }) => {
    const told = memory.remember(fact({ sourceType: 'manual' }))
    const inferred = memory.remember(
      fact({ content: '猜测部署到腾讯云。', sourceType: 'agent', evidence: '猜的' }),
    )
    assert.equal(inferred.status, 'archived')
    const restored = memory.restoreMemory(inferred.id)
    assert.equal(restored.status, 'active')
    assert.equal(restored.archivedAt, '')
    assert.equal(restored.archiveReason, '')
    // 用户选了它，权威度按用户说的算。
    assert.equal(restored.authority, 90)
    assert.equal(
      db.prepare('SELECT reconcile_status AS s FROM memories WHERE id = ?').get(inferred.id).s,
      'done',
    )
    const previous = memory.getMemory(told.id)
    assert.equal(previous.status, 'superseded')
    assert.equal(previous.supersededBy, inferred.id)
    assert.ok(previous.validTo)
    const event = memory.listEvents({}).find((item) => item.action === 'restore')
    assert.deepEqual(event.memoryIds, [inferred.id, told.id])
    assert.equal(memory.restoreMemory('missing'), null)
    // 已经有效的不用恢复，也不产生事件。
    const count = memory.listEvents({}).length
    assert.equal(memory.restoreMemory(inferred.id).status, 'active')
    assert.equal(memory.listEvents({}).length, count)
  })
})

test('restoring a replaced version makes it current again, and undo reverses that', async () => {
  await withMemory(async ({ memory }) => {
    const old = memory.remember(fact())
    const current = memory.remember(fact({ content: '项目部署到腾讯云。' }))
    assert.equal(memory.getMemory(old.id).status, 'superseded')
    const restored = memory.restoreMemory(old.id)
    assert.equal(restored.status, 'active')
    assert.equal(restored.supersededBy, '')
    assert.equal(restored.validTo, '')
    assert.equal(memory.getMemory(current.id).status, 'superseded')
    assert.equal(memory.getMemory(current.id).supersededBy, old.id)

    const event = memory.listEvents({}).find((item) => item.action === 'restore')
    memory.undoEvent(event.id)
    assert.equal(memory.getMemory(old.id).status, 'superseded')
    assert.equal(memory.getMemory(old.id).supersededBy, current.id)
    assert.equal(memory.getMemory(current.id).status, 'active')
  })
})

test('undoing a same-topic replacement restores the old fact and archives the new one', async () => {
  await withMemory(async ({ memory }) => {
    const old = memory.remember(fact())
    const current = memory.remember(fact({ content: '项目部署到腾讯云。' }))
    const event = memory.listEvents({}).find((item) => item.action === 'supersede')
    const undone = memory.undoEvent(event.id)
    assert.deepEqual([...undone.memoryIds].sort(), [old.id, current.id].sort())
    assert.ok(undone.undoneAt)
    assert.equal(memory.getMemory(old.id).status, 'active')
    assert.equal(memory.getMemory(old.id).supersededBy, '')
    assert.equal(memory.getMemory(current.id).status, 'archived')
    assert.equal(memory.getMemory(current.id).archiveReason, 'undone')
    const found = memory.search('部署平台', { spaceIds: ['global'], trackAccess: false })
    assert.deepEqual(
      found.map((item) => item.id),
      [old.id],
    )
    // 不能撤销第二次；事件列表也说明了这一点。
    assert.equal(memory.undoEvent(event.id), null)
    assert.equal(memory.listEvents({}).find((item) => item.id === event.id).undoable, false)
    assert.equal(memory.undoEvent('missing'), null)
  })
})

test('undoing an edit puts the old text back into the record and the index', async () => {
  await withMemory(async ({ memory }) => {
    const item = memory.remember(fact())
    memory.updateMemory(item.id, { content: '项目部署到腾讯云。' })
    assert.equal(memory.search('腾讯云', { spaceIds: ['global'], trackAccess: false }).length, 1)
    const event = memory.listEvents({}).find((entry) => entry.action === 'edit')
    memory.undoEvent(event.id)
    assert.equal(memory.getMemory(item.id).content, '项目部署到阿里云。')
    assert.equal(memory.search('腾讯云', { spaceIds: ['global'], trackAccess: false }).length, 0)
    assert.equal(memory.search('阿里云', { spaceIds: ['global'], trackAccess: false }).length, 1)
  })
})

test('adding has nothing to undo', async () => {
  await withMemory(async ({ memory }) => {
    memory.remember(fact())
    const event = memory.listEvents({}).find((entry) => entry.action === 'add')
    assert.equal(event.undoable, false)
    assert.equal(memory.undoEvent(event.id), null)
  })
})

test('undo snapshots expire after thirty days and events after one hundred and eighty', async () => {
  await withMemory(async ({ memory, clock, sweep, db }) => {
    memory.remember(fact())
    memory.remember(fact({ content: '项目部署到腾讯云。' }))
    const event = memory.listEvents({}).find((item) => item.action === 'supersede')
    assert.equal(event.undoable, true)
    clock.now += 31 * DAY_MS
    sweep()
    assert.equal(memory.listEvents({}).find((item) => item.id === event.id).undoable, false)
    assert.equal(memory.undoEvent(event.id), null)
    assert.equal(
      db.prepare('SELECT before_json AS b FROM memory_events WHERE id = ?').get(event.id).b,
      '',
    )
    clock.now += 150 * DAY_MS
    sweep()
    assert.equal(
      memory.listEvents({}).some((item) => item.id === event.id),
      false,
    )
  })
})

test('replaced versions are purged after a year but the newest five stay, and the chain stays linked', async () => {
  await withMemory(async ({ memory, clock, sweep, db }) => {
    const ids = []
    for (let index = 0; index < 8; index += 1) {
      ids.push(memory.remember(fact({ content: `第 ${index} 版部署说明。` })).id)
      clock.now += DAY_MS
    }
    sweep()
    assert.equal(
      db.prepare("SELECT COUNT(*) AS c FROM memories WHERE status = 'superseded'").get().c,
      7,
    )
    clock.now += 370 * DAY_MS
    const before = memory.getMemoryHistory(ids[7]).versions.length
    assert.equal(before, 8)
    sweep()
    const remaining = db
      .prepare("SELECT id FROM memories WHERE status = 'superseded' ORDER BY valid_from")
      .all()
      .map((row) => row.id)
    assert.deepEqual(remaining, ids.slice(2, 7))
    assert.equal(memory.getMemory(ids[7]).status, 'active')
    assert.deepEqual(
      memory.getMemoryHistory(ids[7]).versions.map((item) => item.id),
      ids.slice(2, 8),
    )
    // 物理清除同时清掉索引，但不算用户删除：同样的内容还能再记。
    assert.equal(
      db.prepare('SELECT COUNT(*) AS c FROM memory_docs WHERE doc_id = ?').get(ids[0]).c,
      0,
    )
    assert.equal(db.prepare('SELECT COUNT(*) AS c FROM memory_forgotten').get().c, 0)
    assert.equal(memory.remember(fact({ content: '第 0 版部署说明。' })).status, 'active')
  })
})

test('archived memories are purged after one hundred and eighty days, not before', async () => {
  await withMemory(async ({ memory, clock, sweep }) => {
    memory.remember(fact({ sourceType: 'manual' }))
    const conflict = memory.remember(fact({ content: '猜测部署到腾讯云。', sourceType: 'agent' }))
    assert.equal(conflict.status, 'archived')
    clock.now += 179 * DAY_MS
    sweep()
    assert.equal(memory.getMemory(conflict.id).status, 'archived')
    clock.now += 2 * DAY_MS
    sweep()
    assert.equal(memory.getMemory(conflict.id), null)
  })
})

test('over the soft limit the least useful go first; pinned and user-stated memories never do', async () => {
  await withMemory(async ({ memory, sweep, db }) => {
    const spaceId = memory.ensureBotSpace('bot_abcdef')
    const insert = db.prepare(`
      INSERT INTO memories (id, space_id, title, content, type, topic_key, identity_key, source_type, authority, importance, status, pinned, access_count, valid_from, last_confirmed_at, created_at, updated_at)
      VALUES (?, ?, ?, ?, 'task', ?, ?, 'agent', ?, ?, 'active', ?, ?, ?, ?, ?, ?)
    `)
    const row = (
      id,
      { authority = 40, importance = 0.5, pinned = 0, access = 0, age = 10 } = {},
    ) => {
      const at = new Date(START - age * DAY_MS).toISOString()
      insert.run(
        id,
        spaceId,
        id,
        `内容 ${id}`,
        id,
        id,
        authority,
        importance,
        pinned,
        access,
        at,
        at,
        at,
        at,
      )
    }
    memory.transaction(() => {
      // 一共 501 条，软上限 500：只有效用最低的那一条会被归档。
      for (let index = 0; index < 497; index += 1) row(`ordinary-${index}`)
      row('least-useful', { importance: 0.1, age: 300 })
      row('pinned-low', { importance: 0.1, age: 300, pinned: 1 })
      row('stated-low', { importance: 0.1, age: 300, authority: 100 })
      row('often-used', { importance: 0.1, age: 300, access: 1000 })
    })
    sweep()
    const status = (id) =>
      db.prepare('SELECT status, archive_reason AS r FROM memories WHERE id = ?').get(id)
    assert.deepEqual({ ...status('least-useful') }, { status: 'archived', r: 'unused' })
    assert.equal(status('pinned-low').status, 'active')
    assert.equal(status('stated-low').status, 'active')
    assert.equal(
      db
        .prepare("SELECT COUNT(*) AS c FROM memories WHERE space_id = ? AND status = 'active'")
        .get(spaceId).c,
      500,
    )
    // 同样老、同样不重要，用得多的留下来。
    assert.equal(status('often-used').status, 'active')
  })
})

test('archiving for lack of use is bounded per sweep', async () => {
  await withMemory(async ({ memory, sweep, db }) => {
    const spaceId = memory.ensureBotSpace('bot_bounded')
    const insert = db.prepare(`
      INSERT INTO memories (id, space_id, title, content, type, topic_key, identity_key, source_type, authority, status, created_at, updated_at)
      VALUES (?, ?, ?, ?, 'task', ?, ?, 'agent', 40, 'active', ?, ?)
    `)
    const at = new Date(START).toISOString()
    memory.transaction(() => {
      for (let index = 0; index < 900; index += 1) {
        const id = `n-${index}`
        insert.run(id, spaceId, id, `内容 ${id}`, id, id, at, at)
      }
    })
    sweep()
    assert.equal(
      db.prepare("SELECT COUNT(*) AS c FROM memories WHERE status = 'archived'").get().c,
      200,
    )
    sweep()
    assert.equal(
      db.prepare("SELECT COUNT(*) AS c FROM memories WHERE status = 'archived'").get().c,
      400,
    )
  })
})

test('every archive and stale transition is reversible through its event', async () => {
  await withMemory(async ({ memory, clock, cwd }) => {
    const path = join(cwd, 'x.md')
    await writeFile(path, 'v1')
    const item = memory.remember(fact({ sourcePath: 'x.md', cwd }))
    await writeFile(path, 'v2')
    clock.now += 2 * 3_600_000
    memory.search('部署平台', { spaceIds: ['global'], trackAccess: false })
    const event = memory.listEvents({}).find((entry) => entry.action === 'stale')
    memory.undoEvent(event.id)
    assert.equal(memory.getMemory(item.id).status, 'active')
  })
})
