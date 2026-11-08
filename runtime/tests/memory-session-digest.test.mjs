// L2 会话摘要：一次对话做了什么。模型都是假的，不依赖网络。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import test from 'node:test'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import { runMemoryConsolidation } from '../services/memory/memory-consolidator.mjs'

const NOW = new Date(2026, 10, 5, 12).getTime()
const MINUTE = 60_000
const DAY = 86_400_000
const ago = (minutes) => new Date(NOW - minutes * MINUTE).toISOString()

function fakeClient({ reply = () => '用户要修登录，已经修好。', available = true } = {}) {
  const calls = []
  return {
    calls,
    available: async () => available,
    complete: async (request) => {
      calls.push(request)
      const answer = await reply(request)
      return answer && typeof answer === 'object' ? answer : { text: answer, model: 'fake/aux' }
    },
  }
}

async function withMemory(run, { client = null } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-digest-'))
  const cwd = join(directory, 'project')
  await mkdir(cwd, { recursive: true })
  const clock = { value: NOW }
  const memory = new LocalMemoryRuntime({
    path: join(directory, 'memory.sqlite'),
    cwd,
    modelClient: client,
    now: () => clock.value,
  })
  try {
    await memory.init()
    await run(memory, { clock, cwd, directory })
  } finally {
    await memory.dispose()
    await rm(directory, { recursive: true, force: true })
  }
}

function turn(memory, sessionId, minutesAgo, user, assistant = '好的。', spaceId = 'global') {
  return memory.recordEpisode({
    sessionId,
    spaceId,
    user,
    assistant,
    at: ago(minutesAgo),
  }).id
}

const sessionDigests = (memory) =>
  memory
    .requireDb()
    .prepare("SELECT * FROM memory_digests WHERE level = 'session' ORDER BY window_start")
    .all()

const searchDigests = (memory, query) =>
  memory.search(query, {
    spaceIds: ['global'],
    kinds: ['digest'],
    minScore: 0,
    trackAccess: false,
  })

test('the sessions list shows newest first, pages by last turn, and hides what must not surface', async () => {
  await withMemory(async (memory, { clock }) => {
    for (const [session, minutes] of [
      ['old', 300],
      ['middle', 200],
      ['new', 100],
    ]) {
      turn(memory, session, minutes + 5, `${session} 第一句`)
      turn(memory, session, minutes, `${session} 第二句`)
    }
    const db = memory.requireDb()
    await runMemoryConsolidation(memory.requireStore(), { now: clock.value, client: null })
    const list = (input) => memory.listSessionDigests(input)

    assert.deepEqual(
      list().map((digest) => digest.sessionId),
      ['new', 'middle', 'old'],
    )
    const [newest] = list({ limit: 1 })
    assert.equal(newest.windowEnd, ago(100))
    assert.equal(newest.spaceId, 'global')
    assert.match(newest.id, /./)
    // 规则版第一行的时间标签是给检索用的，界面自己写时间。
    assert.doesNotMatch(newest.text, /^Session /)
    assert.match(newest.text, /new 第一句/)
    assert.deepEqual(
      list({ before: newest.windowEnd, limit: 1 }).map((digest) => digest.sessionId),
      ['middle'],
    )
    assert.deepEqual(list({ spaceId: 'nothing-here' }), [])

    // 待重算（来源被用户删了）的不露面，Bot 的空间也不混进来。
    db.prepare("UPDATE memory_digests SET dirty = 2 WHERE session_id = 'middle'").run()
    assert.deepEqual(
      list().map((digest) => digest.sessionId),
      ['new', 'old'],
    )
    db.prepare(
      "INSERT OR IGNORE INTO memory_spaces (id, kind, name, created_at, updated_at) VALUES ('bot:x', 'custom', 'x', '', '')",
    ).run()
    db.prepare("UPDATE memory_digests SET space_id = 'bot:x' WHERE session_id = 'old'").run()
    assert.deepEqual(
      list().map((digest) => digest.sessionId),
      ['new'],
    )
  })
})

test('a session digest waits until the session has gone quiet, and one turn is not enough', async () => {
  await withMemory(async (memory, { clock }) => {
    turn(memory, 'busy', 5, '修一下登录页')
    turn(memory, 'busy', 2, '再加个测试')
    turn(memory, 'single', 300, '这是只有一轮的会话')
    await memory.consolidate({ force: true })
    assert.equal(sessionDigests(memory).length, 0)

    clock.value = NOW + 31 * MINUTE
    await memory.consolidate({ force: true })
    const rows = sessionDigests(memory)
    assert.equal(rows.length, 1)
    assert.equal(rows[0].session_id, 'busy')
    assert.equal(rows[0].window_start, ago(5))
    assert.equal(rows[0].window_end, ago(2))
    assert.match(rows[0].text, /^Session 2026-11-05 /)
    assert.match(rows[0].text, /修一下登录页/)
    assert.match(rows[0].text, /再加个测试/)
  })
})

test('without a model the digest is an outline of what was asked and what was recorded', async () => {
  await withMemory(async (memory) => {
    turn(memory, 's', 300, '把发布脚本改成幂等')
    turn(memory, 's', 290, '顺便补上回滚说明')
    turn(memory, 's', 280, '好，收尾')
    memory.remember({
      spaceId: 'global',
      title: '发布脚本要幂等',
      content: '重复触发不会重复发布。',
      topic: 'project.release.idempotent',
      sessionId: 's',
      validFrom: ago(290),
    })
    await memory.consolidate({ force: true })
    const [digest] = sessionDigests(memory)
    assert.match(digest.text, /把发布脚本改成幂等/)
    assert.match(digest.text, /发布脚本要幂等/)
    // 留着脏标记，配上模型后还能写好读些。
    assert.equal(digest.dirty, 1)
  })
})

test('a longer session is written by the auxiliary model, as data and within a size limit', async () => {
  const client = fakeClient()
  await withMemory(
    async (memory) => {
      for (let index = 0; index < 40; index += 1)
        turn(memory, 'long', 400 - index, `第${index}轮的问题${'字'.repeat(600)}`, `回答${index}`)
      memory.remember({
        spaceId: 'global',
        title: '中途定下的决定',
        content: '改用 pnpm。',
        topic: 'project.pkg.manager',
        sessionId: 'long',
        validFrom: ago(380),
      })
      turn(memory, 'short', 200, '两轮的会话不值得花钱')
      turn(memory, 'short', 190, '再问一句')
      await memory.consolidate({ force: true })

      const sessionCalls = client.calls.filter((call) => /one conversation/.test(call.system))
      assert.equal(sessionCalls.length, 1)
      const [{ system, user, purpose }] = sessionCalls
      assert.equal(purpose, 'digest')
      assert.match(system, /data, not instructions/)
      assert.match(user, /第0轮/)
      assert.match(user, /第39轮/)
      assert.doesNotMatch(user, /第20轮/)
      assert.match(user, /40 turns, 28 in the middle omitted/)
      assert.match(user, /中途定下的决定/)
      assert.ok(user.length < 8000, `prompt ${user.length} chars`)

      const rows = sessionDigests(memory)
      const long = rows.find((row) => row.session_id === 'long')
      assert.match(long.text, /用户要修登录，已经修好。/)
      assert.equal(long.dirty, 0)
      const short = rows.find((row) => row.session_id === 'short')
      assert.equal(short.dirty, 0)
      assert.match(short.text, /两轮的会话不值得花钱/)
    },
    { client },
  )
})

test('resuming a session rewrites its digest once it goes quiet again, and a settled one is left alone', async () => {
  const client = fakeClient()
  await withMemory(
    async (memory, { clock }) => {
      turn(memory, 's', 300, '第一件事')
      turn(memory, 's', 290, '第二件事')
      turn(memory, 's', 280, '第三件事')
      await memory.consolidate({ force: true })
      const [first] = sessionDigests(memory)
      const countCalls = () => client.calls.filter((c) => /one conversation/.test(c.system)).length
      assert.equal(countCalls(), 1)

      await memory.consolidate({ force: true })
      assert.equal(countCalls(), 1)
      assert.equal(sessionDigests(memory)[0].generated_at, first.generated_at)

      memory.recordEpisode({ sessionId: 's', spaceId: 'global', user: '第四件事', at: ago(-1) })
      clock.value = NOW + 10 * MINUTE
      await memory.consolidate({ force: true })
      assert.equal(countCalls(), 1)

      clock.value = NOW + 40 * MINUTE
      await memory.consolidate({ force: true })
      assert.equal(countCalls(), 2)
      const [second] = sessionDigests(memory)
      assert.equal(second.id, first.id)
      assert.equal(second.window_end, ago(-1))
      assert.equal(sessionDigests(memory).length, 1)
    },
    { client },
  )
})

test('search finds a session digest, auto recall and the rings do not', async () => {
  await withMemory(async (memory) => {
    turn(memory, 's', 300, '把语音告警的阈值调低')
    turn(memory, 's', 290, '阈值调好了再跑一遍')
    await memory.consolidate({ force: true })
    const [hit] = searchDigests(memory, '语音告警阈值')
    assert.equal(hit.kind, 'digest')
    assert.equal(hit.level, 'session')
    assert.equal(hit.sessionId, 's')

    const { memories } = await memory.relevantContext('语音告警的阈值是多少', '', 4, {
      spaceIds: ['global'],
    })
    assert.equal(memories.length, 0)
    const facts = memory.search('语音告警阈值', {
      spaceIds: ['global'],
      minScore: 0,
      trackAccess: false,
    })
    assert.equal(facts.length, 0)
    assert.equal(memory.getEvolution({ spaceId: 'global' }).digests.length, 0)
  })
})

test('many sessions do not push the week digest out of a question about that week', async () => {
  await withMemory(async (memory) => {
    const inLastWeek = (day, hour, minute = 0) => new Date(2026, 9, day, hour, minute).toISOString()
    for (const day of [27, 29]) {
      memory.remember({
        spaceId: 'global',
        title: `第${day}号的决定`,
        content: `第${day}号定下来的事。`,
        type: 'decision',
        topic: `test.decision.d${day}`,
        validFrom: inLastWeek(day, 9),
      })
    }
    for (let index = 0; index < 20; index += 1) {
      for (const minute of [0, 10])
        memory.recordEpisode({
          sessionId: `session-${index}`,
          spaceId: 'global',
          user: `第${index}个会话，第${minute}分钟`,
          at: inLastWeek(27 + (index % 3), 10 + index, minute),
        })
    }
    for (let pass = 0; pass < 6; pass += 1) await memory.consolidate({ force: true })
    assert.equal(sessionDigests(memory).length, 20)
    const found = memory.search('上周干了什么', {
      spaceIds: ['global'],
      kinds: ['digest', 'fact'],
      minScore: 0,
      trackAccess: false,
    })
    assert.equal(found[0].kind, 'digest')
    assert.equal(found[0].level, 'week')
  })
})

test('deleting the session or clearing conversations removes the digest and its index entry', async () => {
  await withMemory(async (memory) => {
    turn(memory, 'gone', 300, '这段对话讲的是蓝色大象')
    turn(memory, 'gone', 290, '继续讲蓝色大象')
    turn(memory, 'cleared', 300, '这段对话讲的是红色狐狸')
    turn(memory, 'cleared', 290, '继续讲红色狐狸')
    await memory.consolidate({ force: true })
    assert.equal(sessionDigests(memory).length, 2)

    memory.forgetSession('gone')
    assert.deepEqual(
      sessionDigests(memory).map((row) => row.session_id),
      ['cleared'],
    )
    assert.equal(searchDigests(memory, '蓝色大象').length, 0)

    memory.clearEpisodes({ spaceId: 'global' })
    assert.equal(sessionDigests(memory).length, 0)
    assert.equal(searchDigests(memory, '红色狐狸').length, 0)
    await memory.consolidate({ force: true })
    assert.equal(sessionDigests(memory).length, 0)
  })
})

test('a fact deleted by the user hides the digest of its session at once, and it comes back without it', async () => {
  const client = fakeClient({
    reply: (request) => `总结：${request.user}`,
  })
  await withMemory(
    async (memory) => {
      const secretTurn = turn(memory, 's', 300, '我的前公司是灰狼科技，记一下')
      turn(memory, 's', 290, '再帮我改一下简历')
      turn(memory, 's', 280, '第三轮')
      turn(memory, 's', 270, '第四轮')
      const secret = memory.remember({
        spaceId: 'global',
        title: '前公司',
        content: '用户的前公司是灰狼科技。',
        evidence: '我的前公司是灰狼科技',
        topic: 'user.employer.previous',
        sessionId: 's',
        episodeId: secretTurn,
        validFrom: ago(300),
      })
      await memory.consolidate({ force: true })
      assert.equal(searchDigests(memory, '灰狼科技').length, 1)
      const [before] = sessionDigests(memory)

      memory.forget(secret.id)
      assert.equal(searchDigests(memory, '灰狼科技').length, 0)
      assert.equal(sessionDigests(memory)[0].dirty, 2)
      assert.equal(memory.openMemory(before.id), null)

      await memory.consolidate({ force: true })
      const [after] = sessionDigests(memory)
      assert.doesNotMatch(after.text, /灰狼/)
      assert.match(after.text, /简历/)
      assert.equal(searchDigests(memory, '灰狼科技').length, 0)
    },
    { client },
  )
})

test('a digest left with too few turns after a deletion is removed', async () => {
  await withMemory(async (memory) => {
    const first = turn(memory, 's', 300, '只有两轮的会话，第一轮讲橙色鲸鱼')
    turn(memory, 's', 290, '第二轮')
    const fact = memory.remember({
      spaceId: 'global',
      title: '橙色鲸鱼',
      content: '讲了橙色鲸鱼。',
      topic: 'test.orange.whale',
      sessionId: 's',
      episodeId: first,
      validFrom: ago(300),
    })
    await memory.consolidate({ force: true })
    assert.equal(sessionDigests(memory).length, 1)
    memory.forget(fact.id)
    await memory.consolidate({ force: true })
    assert.equal(sessionDigests(memory).length, 0)
    assert.equal(searchDigests(memory, '橙色鲸鱼').length, 0)
  })
})

test('whatever the user deletes while the model is writing does not come back', async () => {
  let memoryRef
  let afterCall = () => {}
  const client = fakeClient({
    reply: () => {
      afterCall()
      return '总结里提到了紫色章鱼。'
    },
  })
  await withMemory(
    async (memory) => {
      memoryRef = memory
      for (const minutes of [300, 290, 280]) turn(memory, 'deleted', minutes, '紫色章鱼')
      afterCall = () => memoryRef.forgetSession('deleted')
      await memory.consolidate({ force: true })
      assert.equal(sessionDigests(memory).length, 0)
      assert.equal(searchDigests(memory, '紫色章鱼').length, 0)
    },
    { client },
  )
  await withMemory(
    async (memory) => {
      memoryRef = memory
      const [start] = [turn(memory, 'edited', 300, '紫色章鱼')]
      turn(memory, 'edited', 290, '第二轮')
      turn(memory, 'edited', 280, '第三轮')
      turn(memory, 'edited', 270, '第四轮')
      const fact = memory.remember({
        spaceId: 'global',
        title: '紫色章鱼',
        content: '讲了紫色章鱼。',
        topic: 'test.purple.octopus',
        sessionId: 'edited',
        episodeId: start,
        validFrom: ago(300),
      })
      afterCall = () => {
        if (memoryRef.getMemory(fact.id)) memoryRef.forget(fact.id)
      }
      await memory.consolidate({ force: true })
      afterCall = () => {}
      assert.equal(searchDigests(memory, '紫色章鱼').length, 0)
      assert.doesNotMatch(sessionDigests(memory)[0]?.text || '', /紫色章鱼/)
    },
    { client },
  )
})

test('a fact deleted while its day digest is being written stays out of that digest', async () => {
  let deleteDuringCall = () => {}
  const client = fakeClient({
    reply: () => {
      deleteDuringCall()
      return '总结里提到了青色海豚。'
    },
  })
  await withMemory(
    async (memory) => {
      const yesterday = new Date(2026, 10, 4, 9).toISOString()
      const secret = memory.remember({
        spaceId: 'global',
        title: '青色海豚',
        content: '讲了青色海豚。',
        type: 'decision',
        topic: 'test.cyan.dolphin',
        validFrom: yesterday,
      })
      memory.remember({
        spaceId: 'global',
        title: '另一件事',
        content: '换了更新通道。',
        type: 'decision',
        topic: 'test.update.channel',
        validFrom: yesterday,
      })
      deleteDuringCall = () => {
        if (memory.getMemory(secret.id)) memory.forget(secret.id)
      }
      await memory.consolidate({ force: true })
      deleteDuringCall = () => {}
      assert.equal(searchDigests(memory, '青色海豚').length, 0)
      const rows = memory.requireDb().prepare('SELECT text FROM memory_digests').all()
      for (const row of rows) assert.doesNotMatch(row.text, /青色海豚/)
    },
    { client },
  )
})

test('a model failure leaves an outline to search, keeps a written digest, and retries within a daily limit', async () => {
  let failing = true
  const client = fakeClient({
    reply: (request) => {
      if (!/one conversation/.test(request.system)) return '其他整理。'
      return failing ? { errorCode: 'model_error' } : '模型写好的总结。'
    },
  })
  const diagnostics = []
  await withMemory(
    async (memory, { clock }) => {
      for (const minutes of [300, 290, 280]) turn(memory, 's', minutes, '讲一只白色猫头鹰')
      const pass = () =>
        runMemoryConsolidation(memory.store, {
          force: true,
          diagnose: (code) => diagnostics.push(code),
        })
      const sessionCalls = () => client.calls.filter((c) => /one conversation/.test(c.system))

      await pass()
      assert.deepEqual(diagnostics, ['session_digest_model_error'])
      const [outline] = sessionDigests(memory)
      assert.match(outline.text, /白色猫头鹰/)
      assert.equal(outline.dirty, 1)
      assert.equal(searchDigests(memory, '白色猫头鹰').length, 1)

      // 模型一直不行时一天最多试 15 次，第二天额度重新算。
      for (let attempt = 0; attempt < 30; attempt += 1) await pass()
      assert.equal(sessionCalls().length, 15)
      clock.value = NOW + DAY
      failing = false
      await pass()
      assert.equal(sessionCalls().length, 16)
      const [written] = sessionDigests(memory)
      assert.match(written.text, /模型写好的总结/)
      assert.equal(written.dirty, 0)

      // 已经写好的不被失败盖掉。
      failing = true
      memory.recordEpisode({
        sessionId: 's',
        spaceId: 'global',
        user: '又聊了一句',
        at: new Date(NOW + DAY - 40 * MINUTE).toISOString(),
      })
      await pass()
      assert.match(sessionDigests(memory)[0].text, /模型写好的总结/)
    },
    { client },
  )
})

test('turning the model off stops the retries, and an outline is written only once', async () => {
  const client = fakeClient({ available: false })
  await withMemory(
    async (memory) => {
      for (const minutes of [300, 290, 280]) turn(memory, 's', minutes, '问题')
      await memory.consolidate({ force: true })
      const [first] = sessionDigests(memory)
      await memory.consolidate({ force: true })
      assert.equal(client.calls.length, 0)
      assert.equal(sessionDigests(memory)[0].generated_at, first.generated_at)
    },
    { client },
  )
})

test('a busy backlog is worked through a few sessions per run, newest first', async () => {
  await withMemory(async (memory) => {
    for (let index = 0; index < 6; index += 1) {
      turn(memory, `s${index}`, 300 - index * 10, `问题${index}`)
      turn(memory, `s${index}`, 295 - index * 10, `问题${index}`)
    }
    await memory.consolidate({ force: true })
    assert.deepEqual(
      sessionDigests(memory)
        .map((row) => row.session_id)
        .sort(),
      ['s2', 's3', 's4', 's5'],
    )
    await memory.consolidate({ force: true })
    assert.equal(sessionDigests(memory).length, 6)
  })
})

test('sessions older than the look-back window are not back-filled', async () => {
  await withMemory(async (memory, { clock }) => {
    turn(memory, 'old', 300, '很久以前的问题')
    turn(memory, 'old', 290, '很久以前的问题')
    clock.value = NOW + 15 * DAY
    await memory.consolidate({ force: true })
    assert.equal(sessionDigests(memory).length, 0)
  })
})

test('two sessions that start in the same millisecond both keep a digest', async () => {
  await withMemory(async (memory) => {
    for (const session of ['a', 'b']) {
      turn(memory, session, 300, `会话${session}的问题`)
      turn(memory, session, 290, `会话${session}的后续`)
    }
    await memory.consolidate({ force: true })
    await memory.consolidate({ force: true })
    const rows = sessionDigests(memory)
    assert.equal(rows.length, 2)
    assert.notEqual(rows[0].id, rows[1].id)
    assert.notEqual(rows[0].window_start, rows[1].window_start)
    assert.match(rows.find((row) => row.session_id === 'a').text, /会话a的问题/)
    assert.match(rows.find((row) => row.session_id === 'b').text, /会话b的问题/)
  })
})

test('excluded spaces are left alone and an aborted run writes nothing', async () => {
  await withMemory(async (memory, { cwd }) => {
    const project = await memory.ensureWorkspaceSpace(cwd)
    turn(memory, 'bot', 300, '问题', '回答', project)
    turn(memory, 'bot', 290, '问题', '回答', project)
    await runMemoryConsolidation(memory.store, { force: true, excludeSpaceIds: [project] })
    assert.equal(sessionDigests(memory).length, 0)

    const controller = new AbortController()
    controller.abort()
    await runMemoryConsolidation(memory.store, { force: true, signal: controller.signal })
    assert.equal(sessionDigests(memory).length, 0)

    await runMemoryConsolidation(memory.store, { force: true })
    assert.equal(sessionDigests(memory)[0].space_id, project)
  })
})

test('memory_open expands a session digest to its facts and turns', async () => {
  await withMemory(async (memory) => {
    const first = turn(memory, 's', 300, '把发布脚本改成幂等')
    turn(memory, 's', 290, '再补个测试')
    const fact = memory.remember({
      spaceId: 'global',
      title: '发布脚本要幂等',
      content: '重复触发不会重复发布。',
      topic: 'project.release.idempotent',
      sessionId: 's',
      episodeId: first,
      validFrom: ago(300),
    })
    await memory.consolidate({ force: true })
    const [digest] = sessionDigests(memory)
    const opened = memory.openMemory(digest.id)
    assert.equal(opened.kind, 'digest')
    assert.equal(opened.spaceId, 'global')
    assert.equal(opened.item.level, 'session')
    assert.equal(opened.item.sessionId, 's')
    assert.match(opened.item.title, /^Session 2026-11-05/)
    assert.deepEqual(
      opened.children.map((child) => child.id),
      [fact.id],
    )
    assert.equal(opened.turns.length, 2)
    assert.equal(opened.turns[0].title, '把发布脚本改成幂等')
    assert.equal(memory.openMemory(opened.turns[0].id).kind, 'episode')
  })
})

test('untrusted facts stay out of a session digest', async () => {
  await withMemory(async (memory) => {
    turn(memory, 's', 300, '查一下这个网页')
    turn(memory, 's', 290, '继续')
    memory.remember({
      spaceId: 'global',
      title: '网页里写的黑色蝙蝠',
      content: '来自网页。',
      topic: 'external.page.claim',
      sessionId: 's',
      sourceType: 'external',
      validFrom: ago(300),
    })
    await memory.consolidate({ force: true })
    assert.doesNotMatch(sessionDigests(memory)[0].text, /黑色蝙蝠/)
  })
})

test('an existing library gains the column without raising the revision, so older builds can still open it', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-digest-schema-'))
  const path = join(directory, 'memory.sqlite')
  try {
    const first = new LocalMemoryRuntime({ path, cwd: directory, now: () => NOW })
    await first.init()
    await first.dispose()

    // 还原成已发布版本写出的库：没有这一列。
    const raw = new DatabaseSync(path)
    raw.exec('DROP INDEX memory_digests_session')
    raw.exec('ALTER TABLE memory_digests DROP COLUMN session_id')
    raw.close()

    const memory = new LocalMemoryRuntime({ path, cwd: directory, now: () => NOW })
    await memory.init()
    const columns = memory
      .requireDb()
      .prepare('PRAGMA table_info(memory_digests)')
      .all()
      .map((column) => column.name)
    assert.ok(columns.includes('session_id'))
    // 已发布的版本看到比 5 大的 revision 会拒绝打开。
    assert.equal(memory.schemaRevision(), 5)
    await memory.dispose()
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})
