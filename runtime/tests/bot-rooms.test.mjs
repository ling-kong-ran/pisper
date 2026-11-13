// Bot 群聊：一份共用的聊天记录，Bot 不叫不说话；叫醒一次就是一轮，在自己的席位会话里读没读过的那段、
// 干活、把结果贴回群里。这里覆盖存储（撤回擦掉原文、坏文件只影响一个群）、叫醒规则、增量上下文、
// Bot 之间的链与上限，以及运行时的接线（用真实的 RoomService 和最小的假运行时）。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, writeFile, readdir } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { createApiHandler } from '../http/api-handler.mjs'
import { pisperError } from '../../shared/errors.mjs'
import { BoardEventHub } from '../services/board-event-hub.mjs'
import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { roomMethods } from '../runtime/room-methods.mjs'
import { BotLinkService } from '../services/bots/bot-links.mjs'
import { normalizeBotInput } from '../services/bots/bot-model.mjs'
import {
  botWakeTargets,
  routesToHost,
  selectUnread,
  userWakeTargets,
  wakeBudgetChars,
} from '../services/rooms/room-context.mjs'
import {
  ROOM_CHAIN_MAX_WAKES,
  ROOM_ROUTE_MAX_WAKES,
  isPassReply,
  mentionsEveryone,
  normalizeBrief,
  normalizeRoomAttachments,
  normalizeRoomInput,
  normalizeUserRoomMessage,
  roomRecipients,
  parseBriefReply,
  parseMentions,
  parseRouteReply,
} from '../services/rooms/room-model.mjs'
import { RoomService } from '../services/rooms/room-service.mjs'
import { ATTACHMENT_MARKER } from '../runtime/stream-projection.mjs'

const KE = 'bot_keAAAAAAAA'
const PRO = 'bot_proAAAAAAA'
const FISH = 'bot_fishAAAAAA'

async function tempDir() {
  return mkdtemp(join(tmpdir(), 'pisper-rooms-'))
}

function bot(id, name, overrides = {}) {
  return {
    id,
    ...normalizeBotInput({ name, instructions: `${name} duty`, workspace: { cwd: '/repo' } }),
    threadSessionId: `thread-${name}`,
    ...overrides,
  }
}

// 只装群聊方法用得到的那几样。runs 记下每次开新一轮，queued 记下插进正在跑的一轮的话。
async function fakeRuntime({
  bots = [bot(KE, '小克'), bot(PRO, '小克pro'), bot(FISH, '肥鱼')],
} = {}) {
  const dataDir = await tempDir()
  // 写入后的变化通知照真实接线转给运行时（init 不发通知，这时 runtime 还没建好也无妨）。
  const rooms = new RoomService({
    dataDir,
    onChange: (roomId, scope) => runtime.roomStoreChanged(roomId, scope),
  })
  await rooms.init()
  const registry = new Map(bots.map((item) => [item.id, item]))
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  let created = 0
  Object.assign(runtime, roomMethods, {
    dataDir,
    rooms,
    runs: [],
    queued: [],
    running: new Set(),
    failQueue: false,
    capabilities: { features: { bots: true } },
    isBetaFeatureEnabled: () => runtime.labsOn !== false,
    sessions: new Map(),
    pendingSessions: new Map(),
    startingRuns: new Map(),
    botPendingTrust: new Map(),
    botLinks: new BotLinkService(),
    // 看板事件流只记下 room 帧的请求（集线器自己的合并与节流另有测试）。
    roomFrames: [],
    boardEvents: {
      roomChanged: (roomId, change) => runtime.roomFrames.push({ roomId, ...change }),
    },
    bots: {
      list: () => [...registry.values()].map((item) => ({ ...item })),
      get: (id) => (registry.has(id) ? { ...registry.get(id) } : null),
      validateDirectory: async (path) => path,
    },
    permissions: { getPending: () => [] },
    questions: { getPending: () => [] },
    sessionCards: { entry: () => null },
    findSessionInfo: async () => null,
    createSession: async (name) => {
      created += 1
      return { id: `seat-${created}`, name }
    },
    applyBotSettings: async () => {},
    setSessionCwd: async () => {},
    sessionRunIsActive: (id) => runtime.running.has(id),
    queueSessionMessage: async (id, input) => {
      if (runtime.failQueue) throw new Error('the run has ended')
      runtime.queued.push({ id, ...input })
    },
    promptFromChannel: async (input) => {
      runtime.runs.push(input)
      return { sessionId: input.sessionId, text: '', error: '' }
    },
    // 资产库：和真的一样同内容只存一份，重复的记成引用。
    assets: new Map(),
    createAsset: async (input) => {
      const buffer =
        input.text !== undefined
          ? Buffer.from(String(input.text), 'utf8')
          : Buffer.from(String(input.data || ''), 'base64')
      const entry = { source: input.source, sessionId: input.sessionId || '' }
      const existing = [...runtime.assets.values()].find((asset) => asset.buffer.equals(buffer))
      if (existing) {
        existing.references.push(entry)
        return { id: existing.id }
      }
      const id = `asset-${runtime.assets.size + 1}`
      runtime.assets.set(id, { id, ...entry, name: input.name, buffer, references: [] })
      return { id }
    },
    findAsset: (id) => runtime.assets.get(id),
    getAssetDownload: async (id) => {
      const asset = runtime.assets.get(id)
      return asset ? { size: asset.buffer.length, buffer: asset.buffer } : null
    },
    deleteAsset: async (id) => runtime.assets.delete(id),
  })
  return runtime
}

const PNG = Buffer.from('fake png bytes').toString('base64')
const image = (name = 'shot.png', data = PNG) => ({
  kind: 'image',
  name,
  mimeType: 'image/png',
  size: 14,
  data,
})

// 模拟席位会话跑完一轮：开始 → 会话里留下这一轮的回复 → 结束。
async function finishTurn(runtime, sessionId, reply, { error = '' } = {}) {
  runtime.onSeatRunStart(sessionId)
  runtime.sessions.set(sessionId, {
    session: {
      messages: [
        { role: 'user', content: [{ type: 'text', text: 'wake' }] },
        { role: 'assistant', content: [{ type: 'text', text: reply }] },
      ],
      model: { provider: 'luna', id: 'm1' },
    },
  })
  await runtime.onSeatRunEnd(sessionId, { error })
}

async function settle(runtime) {
  // 没点名的话先在后台判断该谁答，等它叫完人再看席位。
  await Promise.all([...(runtime.roomRouteQueues?.values() || [])])
  await Promise.all([...(runtime.roomSeatQueues?.values() || [])])
  await new Promise((resolve) => setImmediate(resolve))
  await Promise.all([...(runtime.roomSeatQueues?.values() || [])])
}

test('mentions match the longest member name and need a boundary after it', () => {
  const members = [
    { id: KE, name: '小克' },
    { id: PRO, name: '小克pro' },
  ]
  assert.deepEqual(parseMentions('@小克pro 看看，@小克 也看看', members), [PRO, KE])
  assert.deepEqual(parseMentions('@小克proxy 不算', members), [])
  assert.deepEqual(parseMentions('邮箱 a@小克 不算吗', members), [KE])
  assert.equal(mentionsEveryone('@所有人 收尾前还有顾虑吗'), true)
  assert.equal(mentionsEveryone('hi @all'), true)
  assert.equal(mentionsEveryone('mail@allhands.com'), false)
  assert.equal(isPassReply('[pass]'), true)
  assert.equal(isPassReply(' PASS. '), true)
  assert.equal(isPassReply('pass the salt'), false)
})

test('a message needs text, mentions only members, and a reply target that exists', () => {
  assert.throws(() => normalizeUserRoomMessage({ text: '  ' }, [KE], () => true), /required/)
  const message = normalizeUserRoomMessage(
    { text: 'hi', mentions: [KE, PRO], recipients: [PRO, KE] },
    [KE],
    () => true,
  )
  assert.deepEqual(message.mentions, [KE])
  assert.deepEqual(message.recipients, [KE])
  assert.throws(
    () => normalizeUserRoomMessage({ text: 'hi', replyTo: 'msg_missing01' }, [KE], () => false),
    /gone/,
  )
})

test('who wakes up: mentions, recipients, a reply to a bot, everyone; paused bots are listed', () => {
  const members = [
    { id: KE, enabled: true },
    { id: PRO, enabled: true },
    { id: FISH, enabled: false },
  ]
  const nobody = userWakeTargets({ mentions: [], recipients: [], text: '先铺垫一下' }, members)
  assert.deepEqual(nobody, { wake: [], paused: [], triage: false })
  // 点了名就只叫点到的：“发给”里选的这次不叫。
  const some = userWakeTargets(
    { mentions: [PRO], recipients: [KE], replyToAuthor: FISH, text: 'x' },
    members,
  )
  assert.deepEqual(some.wake, [{ botId: PRO, kind: 'mention' }])
  assert.deepEqual(some.paused, [FISH])
  const named = userWakeTargets({ mentions: [KE], recipients: 'all', text: '@小克 看下' }, members)
  assert.deepEqual(named.wake, [{ botId: KE, kind: 'mention' }])
  const replied = userWakeTargets(
    { mentions: [], recipients: 'all', replyToAuthor: PRO, text: '接着说' },
    members,
  )
  assert.deepEqual(replied.wake, [{ botId: PRO, kind: 'reply' }])
  // 没点名才按“发给”：所有人 = 全部成员（暂停的单独列出），或选中的那几个。
  const unnamed = userWakeTargets({ mentions: [], recipients: 'all', text: '大家看看' }, members)
  assert.deepEqual(unnamed.wake, [
    { botId: KE, kind: 'everyone' },
    { botId: PRO, kind: 'everyone' },
  ])
  assert.deepEqual(unnamed.paused, [FISH])
  // “@ 谁谁答”的群里没点名、发给所有人：先由群里判断该谁答，这时谁也不直接叫；
  // 只剩一个能叫醒的成员就直接叫它，它不能跳过（和“发给”选中的一样）。
  const triaged = userWakeTargets({ mentions: [], recipients: 'all', text: '大家看看' }, members, {
    triage: true,
  })
  assert.deepEqual(triaged, { wake: [], paused: [FISH], triage: true })
  const lone = userWakeTargets(
    { mentions: [], recipients: 'all', text: '大家看看' },
    [members[0], members[2]],
    { triage: true },
  )
  assert.deepEqual(lone, {
    wake: [{ botId: KE, kind: 'recipient' }],
    paused: [FISH],
    triage: false,
  })
  // 点了名、@所有人、“发给”选了人：不经过判断。
  for (const message of [
    { mentions: [KE], recipients: 'all', text: '@小克 看下' },
    { mentions: [], recipients: 'all', text: '@所有人 收尾' },
    { mentions: [], recipients: [PRO], text: '你看看' },
  ])
    assert.equal(userWakeTargets(message, members, { triage: true }).triage, false)
  assert.deepEqual(
    userWakeTargets({ mentions: [], recipients: 'all', text: '@所有人 收尾' }, members, {
      triage: true,
    }).wake.map((item) => item.kind),
    ['everyone', 'everyone'],
  )
  const picked = userWakeTargets({ mentions: [], recipients: [KE], text: '你看看' }, members)
  assert.deepEqual(picked.wake, [{ botId: KE, kind: 'recipient' }])
  const all = userWakeTargets({ mentions: [], recipients: [], text: '@所有人 收尾' }, members)
  assert.deepEqual(
    all.wake.map((item) => item.kind),
    ['everyone', 'everyone'],
  )
  // Bot 的回复只认正文里的 @：不叫自己、不叫暂停的。
  assert.deepEqual(botWakeTargets({ authorId: KE, mentions: [KE, PRO, FISH] }, members), [PRO])
})

test('the wake budget scales with the model window and never drops below 24k characters', () => {
  assert.equal(wakeBudgetChars(undefined), 24_000)
  assert.equal(wakeBudgetChars(32_000), 24_000)
  assert.equal(wakeBudgetChars(1_000_000), 300_000)
})

test('unread skips its own messages and unfinished cards, and keeps the newest that fit', () => {
  const messages = [
    { seq: 1, id: 'a', author: { kind: 'user' }, text: 'old', at: '2026-11-09T10:00:00Z' },
    { seq: 2, id: 'b', author: { kind: 'bot', botId: KE }, text: 'mine' },
    { seq: 3, id: 'c', author: { kind: 'bot', botId: PRO }, text: 'x'.repeat(5_000) },
    { seq: 4, id: 'd', author: { kind: 'bot', botId: PRO }, text: '', turn: { state: 'running' } },
    { seq: 5, id: 'e', author: { kind: 'user' }, text: 'latest', replyTo: 'c' },
  ]
  const names = (author) => (author.kind === 'user' ? 'user' : author.botId)
  const all = selectUnread({ messages, readSeq: 0, selfId: KE, budget: 100_000, names })
  assert.deepEqual(
    all.items.map((item) => item.seq),
    [1, 3, 5],
  )
  assert.match(all.items[1].text, /full text in the room, #3/)
  assert.equal(all.items[2].replyToSeq, 3)
  const tight = selectUnread({ messages, readSeq: 0, selfId: KE, budget: 200, names })
  assert.deepEqual(
    tight.items.map((item) => item.seq),
    [5],
  )
  assert.equal(tight.omitted, 2)
})

test('the brief keeps owners that are members or the user, and parses JSON out of a reply', () => {
  const brief = normalizeBrief(
    {
      decisions: ['撤回时原子重写', ''],
      todos: [
        { text: '确认框文案', owner: KE },
        { text: '外人', owner: 'bot_strangerAA' },
        { text: '我来', owner: 'user', done: true },
      ],
    },
    [KE],
  )
  assert.deepEqual(brief.decisions, ['撤回时原子重写'])
  assert.deepEqual(
    brief.todos.map((todo) => todo.owner),
    [KE, '', 'user'],
  )
  assert.equal(
    parseBriefReply('Sure:\n{"decisions":["a"],"todos":[]}\nDone.', [KE]).decisions[0],
    'a',
  )
  assert.equal(parseBriefReply('no json here', [KE]), null)
})

test('the log survives a restart; patches fold in; a recall rewrites the file without the text', async () => {
  const dataDir = await tempDir()
  const service = new RoomService({ dataDir })
  await service.init()
  const room = await service.create({ name: '前端小组', members: [KE, PRO] })
  const first = await service.append(room.id, { author: { kind: 'user' }, text: 'secret sk-12345' })
  const card = await service.append(room.id, {
    author: { kind: 'bot', botId: KE },
    text: '',
    turn: { state: 'running', sessionId: 'seat-1' },
  })
  await service.patch(room.id, card.id, {
    text: '做完了',
    turn: { state: 'done', sessionId: 'seat-1' },
  })
  await service.setSeat(room.id, KE, { sessionId: 'seat-1', readSeq: 1 })

  const again = new RoomService({ dataDir })
  await again.init()
  const messages = await again.messages(room.id)
  assert.deepEqual(
    messages.map((message) => message.seq),
    [1, 2],
  )
  assert.equal(messages[1].text, '做完了')
  assert.equal(messages[1].turn.state, 'done')
  assert.deepEqual(again.seatOfSession('seat-1'), { roomId: room.id, botId: KE })

  await again.recall(room.id, first.id, { seenBy: [KE] })
  const file = await readFile(join(dataDir, 'rooms', room.id, 'messages.jsonl'), 'utf8')
  assert.doesNotMatch(file, /sk-12345/)
  assert.doesNotMatch(file, /"op":"patch"/)
  const recalled = (await again.messages(room.id))[0]
  assert.deepEqual(recalled.recalled.seenBy, [KE])
  assert.equal(recalled.text, undefined)
})

test('a broken room state is set aside and only that room starts empty', async () => {
  const dataDir = await tempDir()
  const service = new RoomService({ dataDir })
  await service.init()
  const broken = await service.create({ name: 'A', members: [KE] })
  const fine = await service.create({ name: 'B', members: [KE] })
  await service.setSeat(broken.id, KE, { sessionId: 'seat-a' })
  await service.setSeat(fine.id, KE, { sessionId: 'seat-b' })
  await writeFile(join(dataDir, 'rooms', broken.id, 'state.json'), '{not json')
  await writeFile(
    join(dataDir, 'rooms', broken.id, 'messages.jsonl'),
    'garbage\n{"seq":1,"id":"msg_okokok","text":"kept"}\n',
  )

  const again = new RoomService({ dataDir })
  await again.init()
  assert.equal(again.list().length, 2)
  assert.equal(again.seatOfSession('seat-a'), null)
  assert.deepEqual(again.seatOfSession('seat-b'), { roomId: fine.id, botId: KE })
  assert.equal((await again.messages(broken.id))[0].text, 'kept')
  const files = await readdir(join(dataDir, 'rooms', broken.id))
  assert.ok(files.some((name) => name.startsWith('state.json.corrupt-')))
})

test('a seat session belongs to its bot; with Labs off it is just a session', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  await runtime.rooms.setSeat(room.id, PRO, { sessionId: 'seat-x' })
  assert.equal(runtime.botForSession('seat-x')?.id, PRO)
  assert.equal(runtime.botForSession('thread-小克')?.id, KE)
  assert.deepEqual(runtime.botSessionIds(runtime.bots.get(PRO)), ['thread-小克pro', 'seat-x'])
  assert.match(runtime.roomSeatPromptFor('seat-x', runtime.bots.get(PRO)), /group room "前端小组"/)
  runtime.labsOn = false
  assert.equal(runtime.botForSession('seat-x'), null)
  assert.equal(runtime.roomSeatOf('seat-x'), null)
})

test('a message that names nobody goes to everyone; a mention wakes only that bot in its own seat', async () => {
  // 没有“只留言”：没 @ 谁、也没选“发给”时发给所有成员（旧客户端发来的空“发给”也一样），先判断该谁答；
  // 这里没有模型可用，判断不了就叫醒所有人，各自可以跳过。
  const everyone = await fakeRuntime()
  const shared = await everyone.createRoom({ name: '前端小组', members: [KE, PRO] })
  const unnamed = await everyone.postRoomMessage(shared.id, { text: '大家看看', recipients: [] })
  assert.deepEqual([unnamed.woke, unnamed.routing], [[], true])
  await settle(everyone)
  assert.deepEqual(
    everyone.runs.map((run) => everyone.roomSeatOf(run.sessionId).botId).toSorted(),
    [KE, PRO].toSorted(),
  )
  assert.match(
    everyone.runs[0].message.split(ATTACHMENT_MARKER)[1],
    /The user posted #1 without addressing anyone\. If nothing in it is for you, reply exactly \[pass\]/,
  )

  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  // 先只发给小克：小克pro 这时没被叫醒，下一次叫醒时补上这条没读过的。
  const first = await runtime.postRoomMessage(room.id, { text: '先铺垫一下背景', recipients: [KE] })
  assert.deepEqual(first.woke, [KE])
  // “发给”选的是所有人，但这句点了名：只叫点到的。
  const asked = await runtime.postRoomMessage(room.id, {
    text: '@小克pro 看看还缺什么',
    recipients: 'all',
  })
  await settle(runtime)
  assert.deepEqual(asked.woke, [PRO])
  const run = runtime.runs.find((item) => item.message.startsWith('@小克pro'))
  // 看得见的是你的原话；群上下文藏在附件上下文里，带着两条没读过的消息与叫醒来由。
  const [visible, context] = run.message.split(ATTACHMENT_MARKER)
  assert.equal(visible, '@小克pro 看看还缺什么')
  assert.match(context, /#1 \d\d:\d\d user: 先铺垫一下背景/)
  assert.match(context, /#2 .*user: @小克pro 看看还缺什么/)
  assert.match(context, /The user mentioned you in #2/)
  assert.equal(run.unattended, false)
  // 送达（这一轮真正开始）之前不算读过。
  assert.equal((await runtime.rooms.seat(room.id, PRO)).readSeq, 0)
  runtime.onSeatRunStart(run.sessionId)
  await runtime.roomSeatTurns.get(run.sessionId).ready
  assert.equal((await runtime.rooms.seat(room.id, PRO)).readSeq, 2)
})

test('a turn posts a work card, then its reply; an @ in the reply wakes the other bot', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  await runtime.postRoomMessage(room.id, { text: '@小克pro 改一下撤回' })
  await settle(runtime)
  const seat = runtime.runs[0].sessionId
  await finishTurn(runtime, seat, '改完了。@小克 确认框文案交给你')
  await settle(runtime)
  const messages = await runtime.rooms.messages(room.id)
  const card = messages.find((message) => message.turn)
  assert.equal(card.author.botId, PRO)
  assert.equal(card.turn.state, 'done')
  assert.equal(card.text, '改完了。@小克 确认框文案交给你')
  assert.deepEqual(card.mentions, [KE])
  assert.equal(card.replyTo, messages[0].id)
  // 小克被叫醒：别的 Bot 说的话按外部来源算，这一轮没人在场。
  assert.equal(runtime.runs.length, 2)
  assert.notEqual(runtime.runs[1].sessionId, seat)
  assert.equal(runtime.runs[1].unattended, true)
  assert.match(
    runtime.runs[1].message.split(ATTACHMENT_MARKER)[1],
    /小克pro mentioned you in #2 \(bot-to-bot hop 2 of 6\)/,
  )
})

test('a reply quote alone never wakes a bot, so two bots cannot ping-pong by quoting', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  await runtime.postRoomMessage(room.id, { text: '@小克pro 你好' })
  await settle(runtime)
  await finishTurn(runtime, runtime.runs[0].sessionId, '小克 说得对，我同意')
  await settle(runtime)
  assert.equal(runtime.runs.length, 1)
})

test('@everyone lets a bot pass without a message; a busy seat takes the wake mid-turn', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  await runtime.postRoomMessage(room.id, { text: '@所有人 收尾前还有顾虑吗' })
  await settle(runtime)
  assert.equal(runtime.runs.length, 2)
  const [keSeat, proSeat] = runtime.runs.map((run) => run.sessionId)
  await finishTurn(runtime, keSeat, '[pass]')
  const skipped = (await runtime.rooms.messages(room.id)).find(
    (message) => message.author?.botId === KE,
  )
  assert.equal(skipped.turn.state, 'skipped')
  assert.equal(skipped.text, '')

  // 小克pro 还在跑：新叫醒插进当前这一轮，送达后立刻算读过。
  runtime.onSeatRunStart(proSeat)
  runtime.running.add(proSeat)
  runtime.sessions.set(proSeat, { session: { isStreaming: true, messages: [] } })
  await runtime.postRoomMessage(room.id, { text: '@小克pro 顺便看下手机版' })
  await settle(runtime)
  assert.equal(runtime.queued.length, 1)
  assert.equal(runtime.queued[0].behavior, 'interrupt')
  assert.ok(runtime.queued[0].message.length <= 12_000)
  const seat = await runtime.rooms.seat(room.id, PRO)
  assert.equal(seat.readSeq, await runtime.rooms.lastSeq(room.id))
})

test('a wake that misses a run that is just ending is delivered after it ends', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [PRO] })
  await runtime.postRoomMessage(room.id, { text: '@小克pro 第一件' })
  await settle(runtime)
  const seat = runtime.runs[0].sessionId
  runtime.onSeatRunStart(seat)
  runtime.running.add(seat)
  runtime.sessions.set(seat, { session: { isStreaming: true, messages: [] } })
  runtime.failQueue = true
  await runtime.postRoomMessage(room.id, { text: '@小克pro 第二件' })
  await settle(runtime)
  assert.equal(runtime.queued.length, 0)
  const before = (await runtime.rooms.seat(room.id, PRO)).readSeq
  runtime.running.delete(seat)
  runtime.sessions.set(seat, {
    session: {
      messages: [
        { role: 'user', content: [{ type: 'text', text: 'x' }] },
        { role: 'assistant', content: [{ type: 'text', text: '第一件做完了' }] },
      ],
    },
  })
  await runtime.onSeatRunEnd(seat, {})
  await settle(runtime)
  assert.equal(runtime.runs.length, 2)
  assert.match(runtime.runs[1].message, /第二件/)
  assert.ok((await runtime.rooms.seat(room.id, PRO)).readSeq <= before + 1)
})

test('a chain stops after 12 wakes in total and says so in the room', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  const message = await runtime.rooms.append(room.id, {
    author: { kind: 'bot', botId: KE },
    text: '@小克pro 继续',
    mentions: [PRO],
  })
  runtime.roomChainWakes = new Map([['ch_fullchain1', ROOM_CHAIN_MAX_WAKES]])
  await runtime.wakeFromBot(room, runtime.bots.get(KE), PRO, message, {
    id: 'ch_fullchain1',
    hop: 2,
  })
  await settle(runtime)
  assert.equal(runtime.runs.length, 0)
  const stopped = (await runtime.rooms.messages(room.id)).at(-1)
  assert.equal(stopped.author.kind, 'system')
  assert.deepEqual(
    { type: stopped.event.type, reason: stopped.event.reason },
    { type: 'chain_stopped', reason: 'total' },
  )
  // 6 跳也挡着。
  await runtime.wakeFromBot(room, runtime.bots.get(KE), PRO, message, {
    id: 'ch_deepchain1',
    hop: 6,
  })
  assert.equal((await runtime.rooms.messages(room.id)).at(-1).event.reason, 'hops')
})

test('a recall tells the bots that read it, and the next wake says it was withdrawn', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [PRO] })
  const posted = await runtime.postRoomMessage(room.id, { text: '@小克pro 密码是 abc' })
  await settle(runtime)
  runtime.onSeatRunStart(runtime.runs[0].sessionId)
  await runtime.roomSeatTurns.get(runtime.runs[0].sessionId).ready
  await runtime.onSeatRunEnd(runtime.runs[0].sessionId, {})
  const result = await runtime.recallRoomMessage(room.id, posted.message.id)
  assert.deepEqual(result.seenBy, [PRO])
  await runtime.postRoomMessage(room.id, { text: '@小克pro 刚才那条忘掉' })
  await settle(runtime)
  const context = runtime.runs[1].message.split(ATTACHMENT_MARKER)[1]
  assert.match(context, /\[Recalled\] Messages #1 were withdrawn/)
  assert.doesNotMatch(context, /abc/)
})

test('room routes sit behind the Bots Labs switch', async () => {
  let enabled = false
  const handler = createApiHandler({
    capabilities: { features: { bots: true } },
    isBetaFeatureEnabled: () => enabled,
    listRooms: () => [{ id: 'room_abcdef', name: 'A' }],
  })
  const respond = async (method, path) => {
    const output = {
      status: 0,
      body: '',
      destroyed: false,
      writableEnded: false,
      headersSent: false,
      writeHead(status) {
        this.status = status
      },
      flushHeaders() {},
      write(chunk = '') {
        this.body += chunk
      },
      end(chunk = '') {
        this.body += chunk
        this.writableEnded = true
      },
    }
    await handler(
      { method, headers: {}, async *[Symbol.asyncIterator]() {} },
      output,
      new URL(`http://localhost${path}`),
    )
    return { status: output.status, body: JSON.parse(output.body) }
  }
  const closed = await respond('GET', '/api/rooms')
  assert.equal(closed.status, 409)
  assert.equal(closed.body.code, 'beta_feature_disabled')
  enabled = true
  assert.deepEqual(await respond('GET', '/api/rooms'), {
    status: 200,
    body: { rooms: [{ id: 'room_abcdef', name: 'A' }] },
  })
})

test('a finished card carries the approval its background job is waiting on', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [FISH] })
  await runtime.postRoomMessage(room.id, { text: '@肥鱼 跑一下冒烟' })
  await settle(runtime)
  const seat = runtime.runs[0].sessionId
  await finishTurn(runtime, seat, '派了一个子任务去跑。')
  runtime.permissions.getPending = (sessionId) =>
    sessionId === seat
      ? [{ id: 'ap_1', toolName: 'bash', args: { command: 'npm run test:ui' }, createdAt: '' }]
      : []
  const page = await runtime.listRoomMessages(room.id)
  const card = page.messages.find((message) => message.turn)
  assert.equal(card.turn.state, 'done')
  assert.equal(card.turn.pending.waiting, 'approval')
  assert.equal(card.turn.pending.approval.id, 'ap_1')
  // 轮询增量也带上它：批准出现、消失都要反映到卡上。
  const next = await runtime.listRoomMessages(room.id, { after: page.lastSeq })
  assert.ok(next.messages.some((message) => message.id === card.id))
})

test('the room page groups an everyone round, hides passes and draws bot handoffs', async () => {
  const { timelineItems, mentionSegments, parseTodoLine, activeMentionQuery } =
    await import('../../src/features/bots/room-presentation.ts')
  const user = (id, seq, text, extra = {}) => ({
    id,
    seq,
    at: '',
    author: { kind: 'user' },
    text,
    mentions: [],
    replyTo: '',
    recipients: null,
    turn: null,
    chainHop: 1,
    event: null,
    recalled: null,
    ...extra,
  })
  const card = (id, seq, botId, state, triggers, chainHop = 1) => ({
    ...user(id, seq, state === 'skipped' ? '' : 'ok'),
    author: { kind: 'bot', botId },
    chainHop,
    turn: { state, triggers, alsoAnswered: [], steps: [], sessionId: `seat-${botId}` },
  })
  const items = timelineItems([
    user('m1', 1, '@所有人 有顾虑吗'),
    card('m2', 2, KE, 'done', ['m1']),
    card('m3', 3, PRO, 'skipped', ['m1']),
    card('m4', 4, FISH, 'done', ['m2'], 2),
  ])
  assert.deepEqual(
    items.map((item) => item.kind),
    ['user', 'card', 'card'],
  )
  assert.deepEqual(items[0].round, [
    { botId: KE, state: 'replied' },
    { botId: PRO, state: 'skipped' },
  ])
  assert.deepEqual(items[2].handoff, { from: KE, hop: 2 })
  const members = [
    { id: KE, name: '小克' },
    { id: PRO, name: '小克pro' },
  ]
  assert.deepEqual(
    mentionSegments('@小克pro 和 @小克 看看 @所有人', members).map(
      (segment) => segment.memberId || (segment.everyone ? 'all' : ''),
    ),
    [PRO, '', KE, '', 'all'],
  )
  assert.deepEqual(parseTodoLine('[x] @小克 改确认框', members), {
    text: '改确认框',
    owner: KE,
    done: true,
  })
  assert.deepEqual(parseTodoLine('@me 发版', members), { text: '发版', owner: 'user', done: false })
  assert.deepEqual(activeMentionQuery('你好 @小', 5), { start: 3, query: '小' })
  assert.equal(activeMentionQuery('mail@x', 6), null)
})

test('a card that was still running when a bot was woken reaches it once it is written', () => {
  const names = (author) => (author.kind === 'user' ? 'user' : author.botId)
  const running = [
    { seq: 1, id: 'a', author: { kind: 'bot', botId: PRO }, text: '', turn: { state: 'running' } },
    { seq: 2, id: 'b', author: { kind: 'user' }, text: '@小克 看一下' },
  ]
  const first = selectUnread({ messages: running, readSeq: 0, selfId: KE, budget: 10_000, names })
  assert.deepEqual([first.items.map((item) => item.seq), first.upToSeq, first.late], [[2], 2, [1]])
  const written = [
    { ...running[0], text: '结论在这', turn: { state: 'done' } },
    running[1],
    { seq: 3, id: 'c', author: { kind: 'user' }, text: '@小克 再看' },
  ]
  const next = selectUnread({
    messages: written,
    readSeq: first.upToSeq,
    late: first.late,
    selfId: KE,
    budget: 10_000,
    names,
  })
  assert.deepEqual([next.items.map((item) => item.seq), next.late], [[1, 3], []])
})

test('a card left running by a quit is closed on the next start, and read progress never moves back', async () => {
  const dataDir = await tempDir()
  const service = new RoomService({ dataDir })
  await service.init()
  const room = await service.create({ name: '前端小组', members: [KE] })
  const card = await service.append(room.id, {
    author: { kind: 'bot', botId: KE },
    text: '',
    turn: { state: 'running', sessionId: 'seat-1' },
  })
  await service.setSeat(room.id, KE, { sessionId: 'seat-1', readSeq: 5 })
  assert.equal((await service.setSeat(room.id, KE, { readSeq: 3 })).readSeq, 5)

  const again = new RoomService({ dataDir })
  await again.init()
  const closed = await again.message(room.id, card.id)
  assert.deepEqual([closed.turn.state, closed.turn.interrupted], ['stopped', true])
  await again.append(room.id, { author: { kind: 'user' }, text: 'next' })
  const third = new RoomService({ dataDir })
  await third.init()
  assert.deepEqual(
    (await third.messages(room.id)).map((message) => [message.seq, message.turn?.state]),
    [
      [1, 'stopped'],
      [2, undefined],
    ],
  )
})

test('a wake that loses the seat to another run is not marked read and is sent again', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [PRO] })
  await runtime.postRoomMessage(room.id, { text: '@小克pro 第一件' })
  await settle(runtime)
  const seat = runtime.runs[0].sessionId
  await finishTurn(runtime, seat, '第一件做完了')
  await settle(runtime)
  const before = (await runtime.rooms.seat(room.id, PRO)).readSeq

  // 子任务做完的通知抢先开了一轮：我们的 prompt 被拒（busy）。
  runtime.promptFromChannel = async (input) => {
    runtime.runs.push(input)
    runtime.onSeatRunStart(seat, '[Pisper internal agent completion] done')
    runtime.running.add(seat)
    throw Object.assign(new Error('busy'), { code: 'busy' })
  }
  await runtime.postRoomMessage(room.id, { text: '@小克pro 第二件' })
  await settle(runtime)
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal((await runtime.rooms.seat(room.id, PRO)).readSeq, before)
  assert.equal(
    (await runtime.rooms.messages(room.id)).some(
      (message) => message.event?.type === 'wake_failed',
    ),
    false,
  )

  runtime.promptFromChannel = async (input) => {
    runtime.runs.push(input)
    return { sessionId: input.sessionId, text: '', error: '' }
  }
  runtime.running.delete(seat)
  runtime.sessions.set(seat, { session: { messages: [] } })
  await runtime.onSeatRunEnd(seat, {})
  await settle(runtime)
  assert.match(runtime.runs.at(-1).message, /第二件/)
})

test('a report after a background job stays on the chain that started the job', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  await runtime.postRoomMessage(room.id, { text: '@小克 派个活' })
  await settle(runtime)
  const seat = runtime.runs[0].sessionId
  await finishTurn(runtime, seat, '派出去了')
  const chain = (await runtime.rooms.messages(room.id)).at(-1).chain
  assert.ok(chain?.id)

  runtime.onSeatRunStart(seat, '[Pisper internal agent completion] done')
  assert.deepEqual(runtime.roomSeatTurns.get(seat).chain, chain)
  await runtime.onSeatRunEnd(seat, {})
  // 你直接在席位会话里说的话是新的开始。
  runtime.onSeatRunStart(seat, '接着做')
  assert.equal(runtime.roomSeatTurns.get(seat).chain, null)
  await runtime.onSeatRunEnd(seat, {})
})

test('stopping an old card leaves the turn that is running now alone; a seat is created once', async () => {
  const runtime = await fakeRuntime()
  const aborted = []
  runtime.abortSession = async (id) => aborted.push(id)
  const room = await runtime.createRoom({ name: '前端小组', members: [PRO] })
  const [one, two] = await Promise.all([
    runtime.ensureSeatSession(room, runtime.bots.get(PRO)),
    runtime.ensureSeatSession(room, runtime.bots.get(PRO)),
  ])
  assert.equal(one, two)
  await finishTurn(runtime, one, '第一轮')
  const old = (await runtime.rooms.messages(room.id)).at(-1)
  runtime.onSeatRunStart(one, '第二轮')
  await runtime.roomSeatTurns.get(one).ready
  assert.deepEqual(await runtime.stopRoomTurn(room.id, old.id), { stopped: false })
  assert.deepEqual(aborted, [])
  const live = (await runtime.rooms.messages(room.id)).at(-1)
  assert.deepEqual(await runtime.stopRoomTurn(room.id, live.id), { stopped: true })
  assert.deepEqual(aborted, [one])
})

test('a bot is told when its room message did not wake the other bot', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  const seat = await runtime.ensureSeatSession(room, runtime.bots.get(KE))
  const thanks = await runtime.postBotRoomMessage(
    seat,
    runtime.bots.get(KE),
    runtime.bots.get(PRO),
    '谢谢',
  )
  assert.equal(thanks.woke, false)
  const asked = await runtime.postBotRoomMessage(
    seat,
    runtime.bots.get(KE),
    runtime.bots.get(PRO),
    '帮我核对一下撤回的测试',
  )
  assert.equal(asked.woke, true)
})

test('polling brings back a card the page still shows as running, even when it is no longer the latest', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [PRO] })
  const seat = await runtime.ensureSeatSession(room, runtime.bots.get(PRO))
  await finishTurn(runtime, seat, '第一轮')
  await finishTurn(runtime, seat, '第二轮')
  const [first, second] = await runtime.rooms.messages(room.id)
  const plain = await runtime.listRoomMessages(room.id, { after: second.seq })
  assert.deepEqual(
    plain.messages.map((message) => message.id),
    [second.id],
  )
  const watched = await runtime.listRoomMessages(room.id, {
    after: second.seq,
    watch: `${first.id},not-an-id`,
  })
  assert.deepEqual(
    watched.messages.map((message) => [message.id, message.turn.state]),
    [
      [first.id, 'done'],
      [second.id, 'done'],
    ],
  )
})

test('a seat whose model did not switch tries again on the next wake instead of staying on the default model', async () => {
  const runtime = await fakeRuntime({
    bots: [bot(KE, '小克'), bot(PRO, '小克pro', { updatedAt: '2026-11-11T08:00:00.000Z' })],
  })
  const calls = []
  // 第一次套设定时模型没换成功（席位正在说话、模型暂时不可用）。
  runtime.applyBotSettings = async (_item, sessionId) => {
    calls.push(sessionId)
    return calls.length > 1
  }
  runtime.findSessionInfo = async () => ({})
  const room = await runtime.createRoom({ name: '运维群', members: [KE, PRO] })
  const seat = await runtime.ensureSeatSession(room, runtime.bots.get(PRO))
  assert.equal(calls.length, 1)
  assert.equal((await runtime.rooms.seat(room.id, PRO)).applied, '')

  await runtime.postRoomMessage(room.id, { text: '@小克pro 看一下' })
  await settle(runtime)
  assert.deepEqual(calls, [seat, seat])
  assert.equal((await runtime.rooms.seat(room.id, PRO)).applied, '2026-11-11T08:00:00.000Z')

  // 设定已记成套用过，但席位实际还在别的模型上：叫醒前对一下，不一样就重新套。
  runtime.sessionMeta = { [seat]: { model: 'openai-codex/gpt-5.5' } }
  runtime.botSessionModelRef = async () => 'anthropic-relay/claude-opus'
  await runtime.postRoomMessage(room.id, { text: '@小克pro 再看一下' })
  await settle(runtime)
  assert.equal(calls.length, 3)

  // 已经在 Bot 的模型上：不再重复套。
  runtime.sessionMeta = { [seat]: { model: 'anthropic-relay/claude-opus' } }
  await runtime.postRoomMessage(room.id, { text: '@小克pro 第三次' })
  await settle(runtime)
  assert.equal(calls.length, 3)
})

test('a seat picks up new bot settings when it is next woken; a removed member loses its seat', async () => {
  const runtime = await fakeRuntime()
  const applied = []
  const renamed = []
  runtime.applyBotSettings = async (item, sessionId) => applied.push([sessionId, item.updatedAt])
  runtime.renameSession = async (sessionId, title) => renamed.push([sessionId, title])
  // 席位会话在盘上、不在内存里。
  runtime.findSessionInfo = async () => ({})
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  const seat = await runtime.ensureSeatSession(room, runtime.bots.get(PRO))
  assert.equal(applied.length, 1)

  // 改了设定、会话不在内存里：不为它装配运行时，只把标题改掉。
  const changed = {
    ...runtime.bots.get(PRO),
    name: '小克pro',
    updatedAt: '2026-11-10T10:00:00.000Z',
  }
  runtime.bots.get = (id) => (id === PRO ? { ...changed } : null)
  await runtime.syncBotSeats(changed)
  assert.equal(applied.length, 1)
  await runtime.postRoomMessage(room.id, { text: '@小克pro 看一下' })
  await settle(runtime)
  assert.deepEqual(applied.at(-1), [seat, '2026-11-10T10:00:00.000Z'])
  assert.equal(applied.length, 2)

  await runtime.updateRoom(room.id, { name: '新名字' })
  assert.deepEqual(renamed.at(-1), [seat, '小克pro · 新名字'])

  assert.deepEqual(runtime.roomSeatOf(seat), { roomId: room.id, botId: PRO })
  await runtime.rooms.update(room.id, { members: [KE] })
  assert.equal(runtime.roomSeatOf(seat), null)
  await runtime.rooms.update(room.id, { members: [KE, PRO] })
  assert.deepEqual(runtime.roomSeatOf(seat), { roomId: room.id, botId: PRO })
})

test('attachments are checked like the session composer: count, size, kind and absolute paths', () => {
  const tooMany = Array.from({ length: 9 }, (_, index) => image(`p${index}.png`))
  assert.throws(() => normalizeRoomAttachments(tooMany), /limit/)
  const big = Buffer.alloc(10 * 1024 * 1024 + 1).toString('base64')
  assert.throws(() => normalizeRoomAttachments([image('big.png', big)]), /too large/)
  assert.throws(() => normalizeRoomAttachments([image('bad.png', 'not base64!')]), /invalid/)
  assert.throws(
    () => normalizeRoomAttachments([{ kind: 'binary', name: 'a.exe', data: PNG }]),
    /not supported/,
  )
  assert.throws(
    () => normalizeRoomAttachments([{ kind: 'path', name: 'x', path: 'relative/x.ts' }]),
    /absolute/,
  )
  assert.throws(
    () => normalizeRoomAttachments([{ kind: 'document', name: 'a.zip', data: PNG }]),
    /not supported/,
  )
  // 实际大小按内容算，不信客户端报的；没写 MIME 的图片按扩展名补上。
  const [png, note, file] = normalizeRoomAttachments([
    { ...image(), size: 1, mimeType: '' },
    { kind: 'text', name: 'notes.md', text: '要点' },
    { kind: 'path', name: 'app.ts', path: '/repo/src/app.ts' },
  ])
  assert.equal(png.size, 14)
  assert.equal(png.mimeType, 'image/png')
  assert.equal(note.size, Buffer.byteLength('要点'))
  assert.equal(file.path, '/repo/src/app.ts')
  // 只带附件、不写字也能发；什么都没有才算空。
  const only = normalizeUserRoomMessage({ attachments: [image()] }, [KE], () => true)
  assert.equal(only.text, '')
  assert.equal(only.attachments.length, 1)
  assert.throws(() => normalizeUserRoomMessage({ attachments: [] }, [KE], () => true), /required/)
})

test('attachments reach the woken seat like a normal prompt; the log keeps only references', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  const posted = await runtime.postRoomMessage(room.id, {
    text: '@小克pro 看看这张图',
    attachments: [
      image(),
      { kind: 'text', name: 'notes.md', text: 'secret notes' },
      { kind: 'path', name: 'app.ts', path: '/repo/src/app.ts' },
    ],
  })
  await settle(runtime)
  assert.deepEqual(
    posted.message.attachments.map((ref) => [ref.kind, ref.name, ref.id || ref.path]),
    [
      ['image', 'shot.png', 'asset-1'],
      ['text', 'notes.md', 'asset-2'],
      ['path', 'app.ts', '/repo/src/app.ts'],
    ],
  )
  const log = await readFile(join(runtime.dataDir, 'rooms', room.id, 'messages.jsonl'), 'utf8')
  assert.doesNotMatch(log, new RegExp(PNG))
  assert.doesNotMatch(log, /secret notes/)
  const [run] = runtime.runs
  assert.deepEqual(
    run.attachments.map((item) => [item.kind, item.name]),
    [
      ['image', 'shot.png'],
      ['text', 'notes.md'],
      ['path', 'app.ts'],
    ],
  )
  assert.equal(run.attachments[0].data, PNG)
  assert.equal(run.attachments[0].mimeType, 'image/png')
  assert.equal(run.attachments[1].text, 'secret notes')
  assert.equal(run.attachments[2].path, '/repo/src/app.ts')
  assert.equal(run.message.split(ATTACHMENT_MARKER)[0], '@小克pro 看看这张图')

  // 只发附件：看得见的那句是文件名。
  await runtime.postRoomMessage(room.id, {
    attachments: [image('two.png', 'dHdv')],
    recipients: [PRO],
  })
  await settle(runtime)
  assert.equal(runtime.runs[1].message.split(ATTACHMENT_MARKER)[0], '[attached: two.png]')
  assert.equal(runtime.runs[1].attachments[0].data, 'dHdv')
})

test('a busy seat gets the attachments with the queued wake', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [PRO] })
  const seat = await runtime.ensureSeatSession(room, runtime.bots.get(PRO))
  runtime.running.add(seat)
  runtime.sessions.set(seat, { session: { isStreaming: true, messages: [] } })
  await runtime.postRoomMessage(room.id, { text: '@小克pro 顺便看图', attachments: [image()] })
  await settle(runtime)
  assert.equal(runtime.queued.length, 1)
  assert.equal(runtime.queued[0].attachments[0].data, PNG)
})

test('other bots read only the attachment names; the content goes to the woken bot alone', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  await runtime.postRoomMessage(room.id, {
    text: '@小克pro 先看这份',
    attachments: [{ kind: 'text', name: 'plan.md', text: 'the full plan body' }],
  })
  await settle(runtime)
  await runtime.postRoomMessage(room.id, { text: '@小克 你也跟一下' })
  await settle(runtime)
  const ke = runtime.runs.find((run) => run.message.startsWith('@小克 '))
  assert.deepEqual(ke.attachments, [])
  const context = ke.message.split(ATTACHMENT_MARKER)[1]
  assert.match(context, /#1 .*user: @小克pro 先看这份 \[attached: plan\.md\]/)
  assert.doesNotMatch(context, /the full plan body/)
})

test('a recall scrubs the references and deletes files only that message used', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [PRO] })
  const kept = await runtime.postRoomMessage(room.id, {
    text: '留着',
    attachments: [image('a.png')],
  })
  const twice = await runtime.postRoomMessage(room.id, {
    text: '同一张',
    attachments: [image('b.png')],
  })
  const alone = await runtime.postRoomMessage(room.id, {
    text: '密钥截图',
    attachments: [{ kind: 'text', name: 'key.txt', text: 'sk-secret' }],
  })
  const seen = await runtime.postRoomMessage(room.id, {
    text: '给 Bot 看过',
    attachments: [{ kind: 'text', name: 'seen.txt', text: 'already delivered' }],
  })
  // Bot 收到过这一份：它的席位会话里也存了一份引用。
  const seenId = seen.message.attachments[0].id
  runtime.assets.get(seenId).references.push({ source: 'attachment', sessionId: 'seat-1' })

  await runtime.recallRoomMessage(room.id, alone.message.id)
  await runtime.recallRoomMessage(room.id, twice.message.id)
  await runtime.recallRoomMessage(room.id, seen.message.id)
  const log = await readFile(join(runtime.dataDir, 'rooms', room.id, 'messages.jsonl'), 'utf8')
  assert.doesNotMatch(log, /key\.txt|seen\.txt|b\.png/)
  assert.equal(runtime.assets.has(alone.message.attachments[0].id), false)
  // 同内容只存一份：还有没撤回的消息在用，不删。Bot 收到过的也不删。
  assert.equal(twice.message.attachments[0].id, kept.message.attachments[0].id)
  assert.equal(runtime.assets.has(kept.message.attachments[0].id), true)
  assert.equal(runtime.assets.has(seenId), true)
  const placeholder = (await runtime.rooms.messages(room.id)).find(
    (message) => message.id === alone.message.id,
  )
  assert.equal(placeholder.attachments, undefined)
})

test('the room page shows attachments through the session attachment shape', async () => {
  const { roomAttachmentViews } = await import('../../src/features/bots/room-presentation.ts')
  const { parseRoomMessage } = await import('../../src/features/bots/rooms-api.ts')
  const message = parseRoomMessage({
    seq: 1,
    id: 'msg_aaaaaaaa',
    author: { kind: 'user' },
    text: '',
    attachments: [
      { id: 'asset-1', kind: 'image', name: 'shot.png', mimeType: 'image/png', size: 14 },
      { kind: 'path', name: 'app.ts', path: '/repo/src/app.ts' },
      { kind: 'image', name: 'no id' },
      { id: 'asset-2', kind: 'binary', name: 'x' },
    ],
  })
  assert.equal(message.attachments.length, 2)
  const [shot, file] = roomAttachmentViews(message.attachments)
  assert.equal(shot.url, '/api/assets/asset-1/download?inline=1')
  assert.equal(shot.downloadUrl, '/api/assets/asset-1/download')
  assert.equal(file.id, 'path:/repo/src/app.ts')
  assert.equal(file.path, '/repo/src/app.ts')
  assert.equal(file.url, undefined)
})

test('timeline keys move message by message and act only on a focused message', async () => {
  const { timelineKeyAction } = await import('../../src/features/bots/room-presentation.ts')
  const key = (value, extra = {}) => ({
    key: value,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    ...extra,
  })
  const base = { seq: 1, at: '', text: '', attachments: [], mentions: [], replyTo: '' }
  const user = {
    kind: 'user',
    round: null,
    message: { ...base, id: 'm1', author: { kind: 'user' } },
  }
  const card = (state) => ({
    kind: 'card',
    handoff: null,
    message: {
      ...base,
      id: 'm2',
      author: { kind: 'bot', botId: KE },
      turn: { state, sessionId: 'seat-1' },
    },
  })
  const system = { kind: 'system', message: { ...base, id: 'm3', author: { kind: 'system' } } }
  assert.deepEqual(timelineKeyAction(key('ArrowDown'), user, 0, 3, true), {
    type: 'move',
    index: 1,
  })
  assert.deepEqual(timelineKeyAction(key('ArrowDown'), user, 2, 3, true), {
    type: 'move',
    index: 2,
  })
  assert.deepEqual(timelineKeyAction(key('ArrowUp'), user, 0, 3, true), { type: 'move', index: 0 })
  assert.deepEqual(timelineKeyAction(key('Home'), user, 2, 3, false), { type: 'move', index: 0 })
  assert.deepEqual(timelineKeyAction(key('End'), user, 0, 3, false), { type: 'move', index: 2 })
  assert.deepEqual(timelineKeyAction(key('r'), user, 0, 3, true), { type: 'reply' })
  assert.deepEqual(timelineKeyAction(key('Delete'), user, 0, 3, true), { type: 'recall' })
  assert.deepEqual(timelineKeyAction(key('Delete'), card('done'), 1, 3, true), {
    type: 'recall',
  })
  // 退格不撤回：选中文字后习惯性按退格的人不该被确认框打断。
  assert.equal(timelineKeyAction(key('Backspace'), user, 0, 3, true), null)
  assert.deepEqual(timelineKeyAction(key('Enter'), card('running'), 1, 3, true), {
    type: 'process',
  })
  // 还在做的卡不能回复、隐藏；系统行什么都不接；焦点在按钮上时只接移动；带修饰键的留给别的快捷键。
  assert.equal(timelineKeyAction(key('r'), card('running'), 1, 3, true), null)
  assert.equal(timelineKeyAction(key('Delete'), system, 2, 3, true), null)
  assert.equal(timelineKeyAction(key('Enter'), user, 0, 3, true), null)
  assert.equal(timelineKeyAction(key('r'), user, 0, 3, false), null)
  assert.equal(timelineKeyAction(key('Enter'), card('done'), 1, 3, false), null)
  assert.equal(timelineKeyAction(key('r', { metaKey: true }), user, 0, 3, true), null)
  assert.equal(timelineKeyAction(key('ArrowDown', { altKey: true }), user, 0, 3, true), null)
})

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

async function waitFor(check, timeoutMs = 3000) {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const value = check()
    if (value) return value
    if (Date.now() > deadline) throw new Error('timed out')
    await sleep(5)
  }
}

test('room changes reach the board stream as small hints, never message bodies', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  assert.deepEqual(runtime.roomFrames.at(-1), { roomId: room.id, scope: ['list', 'detail'] })
  runtime.roomFrames.length = 0

  // 追加：消息与详情（未读数、最后一条）。
  // 这里只看推送帧，不看叫醒（席位那边另有测试）；点了名，不经过“判断该谁答”（它的帧另有测试）。
  runtime.wakeSeat = async () => {}
  const { message } = await runtime.postRoomMessage(room.id, { text: '@小克 机密的正文' })
  assert.deepEqual(runtime.roomFrames, [{ roomId: room.id, scope: ['messages', 'detail'] }])
  // 补丁、撤回也一样；席位与简报只动详情。
  await runtime.rooms.patch(room.id, message.id, { mentions: [] })
  await runtime.recallRoomMessage(room.id, message.id)
  await runtime.rooms.setSeat(room.id, PRO, { sessionId: 'seat-x' })
  await runtime.editRoomBrief(room.id, { decisions: ['先做撤回'] })
  assert.deepEqual(
    runtime.roomFrames.slice(1).map((frame) => frame.scope),
    [['messages', 'detail'], ['messages', 'detail'], ['detail'], ['detail']],
  )
  assert.doesNotMatch(JSON.stringify(runtime.roomFrames), /机密/)

  // 席位会话的任何变化都是一次“可能变了”的探测；别的会话不算。
  runtime.roomFrames.length = 0
  runtime.roomSeatTouched('seat-x')
  runtime.roomSeatTouched('some-other-session')
  assert.deepEqual(runtime.roomFrames, [
    { roomId: room.id, scope: ['messages', 'detail', 'list'], probe: true },
  ])
  // 探测比的是群的实时签名：席位开始干活、走到下一步时签名变；没变时集线器不发帧。
  const idle = runtime.roomLiveState(room.id)
  assert.equal(idle.lastSeq, 1)
  assert.equal(runtime.roomLiveState(room.id).signature, idle.signature)
  runtime.running.add('seat-x')
  const running = runtime.roomLiveState(room.id)
  assert.notEqual(running.signature, idle.signature)
  runtime.onSeatRunStart('seat-x')
  runtime.observeSeatEvent('seat-x', 'tool_start', { name: 'bash', args: { command: 'ls' } })
  assert.notEqual(runtime.roomLiveState(room.id).signature, running.signature)
  await runtime.roomSeatTurns.get('seat-x').ready

  // 删群：列表 + deleted。
  runtime.running.clear()
  runtime.roomFrames.length = 0
  await runtime.deleteRoom(room.id)
  assert.deepEqual(runtime.roomFrames, [{ roomId: room.id, scope: ['list', 'deleted'] }])
  assert.equal(runtime.roomLiveState(room.id), null)

  // Labs 关着：不发帧。
  const other = await runtime.createRoom({ name: '后端小组', members: [KE] })
  runtime.labsOn = false
  runtime.roomFrames.length = 0
  await runtime.rooms.append(other.id, { author: { kind: 'user' }, text: 'x', mentions: [] })
  assert.deepEqual(runtime.roomFrames, [])
})

test('the board hub merges a burst into one room frame, drops probes that changed nothing and throttles each room', async () => {
  const ROOM = 'room_AAAAAAAAAA'
  let signature = 'idle'
  const hub = new BoardEventHub({
    listItems: async () => new Map(),
    roomState: (roomId) => (roomId === ROOM ? { lastSeq: 7, signature } : null),
    minIntervalMs: 80,
    coalesceMs: 5,
  })
  const frames = []
  const { ready } = hub.connect({
    send: (event, data) => {
      if (event === 'room') frames.push({ ...data, sentAt: Date.now() })
    },
    close: () => {},
  })
  await ready
  try {
    // 一阵变化（工具一秒好几步）合成一帧，范围取并集，认不出的范围丢掉。
    for (let index = 0; index < 10; index += 1) hub.roomChanged(ROOM, { scope: ['messages'] })
    hub.roomChanged(ROOM, { scope: ['detail', 'bogus'] })
    await waitFor(() => frames.length === 1)
    assert.deepEqual(frames[0].scope, ['messages', 'detail'])
    assert.equal(frames[0].roomId, ROOM)
    assert.equal(frames[0].lastSeq, 7)
    assert.equal(typeof frames[0].revision, 'number')

    // 签名没变的探测（Bot 在吐字，页面上没东西可变）：flush 掉，但不发帧。
    hub.roomChanged(ROOM, { scope: ['messages'], probe: true })
    await waitFor(() => hub.roomPending.size === 0)
    await hub.queue
    assert.equal(frames.length, 1)

    // 签名变了（走到下一步）：发，而且离上一帧至少隔一个节流窗口。
    signature = 'running · step 1'
    hub.roomChanged(ROOM, { scope: ['messages', 'detail', 'list'], probe: true })
    await waitFor(() => frames.length === 2)
    assert.ok(frames[1].sentAt - frames[0].sentAt >= 70)
    hub.roomChanged(ROOM, { scope: ['messages'] })
    await waitFor(() => frames.length === 3)
    assert.ok(frames[2].sentAt - frames[1].sentAt >= 70)

    // 不认识的群的探测不发；删群一定发，带 deleted。
    hub.roomChanged('room_BBBBBBBBBB', { scope: ['messages'], probe: true })
    hub.roomChanged('room_BBBBBBBBBB', { scope: ['list', 'deleted'] })
    await waitFor(() => frames.length === 4)
    assert.deepEqual(
      { roomId: frames[3].roomId, scope: frames[3].scope, deleted: frames[3].deleted },
      { roomId: 'room_BBBBBBBBBB', scope: ['messages', 'list', 'deleted'], deleted: true },
    )
    // 补发：带 after 重连时 room 帧也在缓冲里。
    assert.ok(hub.buffer.some((entry) => entry.event === 'room'))
  } finally {
    hub.dispose()
  }
  assert.equal(hub.roomTimer, null)
})

test('the room page and sidebar refresh on frames for them and keep a slow fallback poll', async () => {
  const { notifyRoomFeedEvent, parseRoomChange, roomFeedAlive, subscribeRoomChanges } =
    await import('../../src/features/board/room-changes.ts')
  const policy = await import('../../src/features/bots/room-live-policy.ts')
  const { applyBoardFeedEvent, emptyBoardFeed } =
    await import('../../src/features/board/board-feed.ts')
  const ROOM = 'room_AAAAAAAAAA'
  // 帧按不可信数据解析：群 id 要像样，范围只认几种。
  assert.equal(parseRoomChange({ roomId: 'nope', scope: ['messages'] }), null)
  assert.deepEqual(parseRoomChange({ roomId: ROOM, lastSeq: 3, scope: ['detail', 'x'] }), {
    roomId: ROOM,
    lastSeq: 3,
    scope: ['detail'],
    deleted: false,
  })
  const seen = []
  const unsubscribe = subscribeRoomChanges((change) => seen.push(change))
  notifyRoomFeedEvent('room', { roomId: ROOM, lastSeq: 4, scope: ['messages'] }, true, 1_000)
  notifyRoomFeedEvent('room', { roomId: ROOM, scope: ['messages'] }, false, 1_000)
  notifyRoomFeedEvent('snapshot', { revision: 1, items: [] }, true, 1_000)
  notifyRoomFeedEvent('heartbeat', { revision: 2 }, true, 1_000)
  unsubscribe()
  assert.deepEqual(seen, [{ roomId: ROOM, lastSeq: 4, scope: ['messages'], deleted: false }, '*'])
  assert.equal(roomFeedAlive(1_000 + 10_000), true)
  assert.equal(roomFeedAlive(1_000 + 60_000), false)

  // 看板数据层把 room 帧当作只推进修订号的帧，不改条目。
  const feed = { ...emptyBoardFeed('events'), ready: true, revision: 10 }
  const result = applyBoardFeedEvent(feed, 'room', { revision: 11, roomId: ROOM, scope: [] })
  assert.equal(result.feed.revision, 11)
  assert.equal(result.effects.changed, true)
  assert.deepEqual(result.feed.entries, {})

  // 推送在线：25 / 30 秒兜底；没有推送或断了：照旧 1.5 / 5 秒、3 / 10 秒。
  assert.equal(policy.roomPollDelay({ push: true, alive: true, busy: true }), 25_000)
  assert.equal(policy.roomPollDelay({ push: true, alive: false, busy: true }), 1_500)
  assert.equal(policy.roomPollDelay({ push: false, alive: true, busy: false }), 5_000)
  assert.equal(policy.roomListPollDelay({ push: true, alive: true, busy: true }), 30_000)
  assert.equal(policy.roomListPollDelay({ push: false, alive: false, busy: true }), 3_000)
  const change = (scope, roomId = ROOM) => ({ roomId, lastSeq: 1, scope, deleted: false })
  assert.equal(policy.roomChangeAffectsPage('*', ROOM), true)
  assert.equal(policy.roomChangeAffectsPage(change(['messages']), ROOM), true)
  assert.equal(policy.roomChangeAffectsPage(change(['list']), ROOM), false)
  assert.equal(policy.roomChangeAffectsPage(change(['messages'], 'room_BBBBBBBBBB'), ROOM), false)
  assert.equal(policy.roomChangeAffectsList(change(['messages', 'detail'])), false)
  assert.equal(policy.roomChangeAffectsList(change(['list'])), true)
  assert.equal(policy.roomChangeAffectsList({ ...change([]), deleted: true }), true)
})

// 主持人的模型：按顺序给出脚本里的回复（字符串）；Error 表示请求本身失败，null 表示模型报错。
// gate 给了时等它放行才回，用来确认发送不等主持人。
function scriptedModel(replies, { gate = null } = {}) {
  const prompts = []
  return {
    prompts,
    streamSimple(model, context) {
      prompts.push({ model, text: context.messages[0].content[0].text, tools: context.tools })
      const reply = replies.shift()
      if (reply instanceof Error) throw reply
      return {
        async *[Symbol.asyncIterator]() {
          if (gate) await gate
          if (typeof reply === 'string') yield { type: 'text_delta', delta: reply }
        },
        result: async () =>
          reply === null
            ? { stopReason: 'error', usage: null }
            : { stopReason: 'stop', usage: { totalTokens: 42 } },
      }
    },
  }
}

const DOG = 'bot_dogAAAAAAA'
const CAT = 'bot_catAAAAAAA'
const NAP = 'bot_napAAAAAAA'

async function hostRuntime(replies, options) {
  const runtime = await fakeRuntime({
    bots: [
      bot(KE, '小克'),
      bot(PRO, '小克pro'),
      bot(FISH, '肥鱼'),
      bot(DOG, '阿黄'),
      bot(CAT, '咪咪'),
      bot(NAP, '睡觉的', { enabled: false }),
    ],
  })
  runtime.modelRuntime = scriptedModel(replies, options)
  runtime.usage = []
  runtime.recordUsage = async (day, key, usage) => runtime.usage.push({ key, usage })
  runtime.providerPreferences = {
    resolveSubagentModel: async () => ({ model: { provider: 'luna', id: 'host-model' } }),
  }
  const room = await runtime.createRoom({
    name: '前端小组',
    members: [KE, PRO, FISH, DOG, CAT, NAP],
    answerMode: 'host',
    hostBotId: KE,
  })
  return { runtime, room }
}

async function routed(runtime, roomId) {
  await runtime.roomRouteQueues?.get(roomId)
  await settle(runtime)
  return (await runtime.rooms.messages(roomId)).filter(
    (message) => message.author.kind === 'system',
  )
}

test('who answers: rooms stored before host mode default to @ only; a host must be a member', async () => {
  // 老的群没有这两个字段：默认“@ 谁谁答”。
  const dir = await tempDir()
  await writeFile(
    join(dir, 'pisper-rooms.json'),
    JSON.stringify({
      version: 1,
      rooms: [
        { id: 'room_oldAAAAAAAA', name: '老群', members: [KE], recipients: [] },
        // 主持人已经不在群里：退回默认，这个群照常加载。
        {
          id: 'room_hostGoneAAAA',
          name: '主持人走了',
          members: [KE],
          answerMode: 'host',
          hostBotId: PRO,
        },
      ],
    }),
  )
  const rooms = new RoomService({ dataDir: dir })
  await rooms.init()
  assert.deepEqual(
    rooms.list().map((room) => [room.id, room.answerMode, room.hostBotId]),
    [
      ['room_oldAAAAAAAA', 'mention', ''],
      ['room_hostGoneAAAA', 'mention', ''],
    ],
  )
  // 明确要主持人分派却没给成员当主持人：报错；改成员时把主持人移出去：退回默认，不让这次修改失败。
  assert.throws(
    () => normalizeRoomInput({ name: 'x', members: [KE], answerMode: 'host', hostBotId: PRO }),
    (error) => error.code === 'invalid',
  )
  const current = normalizeRoomInput({
    name: 'x',
    members: [KE, PRO],
    answerMode: 'host',
    hostBotId: PRO,
  })
  assert.deepEqual([current.answerMode, current.hostBotId], ['host', PRO])
  const later = normalizeRoomInput({ members: [KE] }, current)
  assert.deepEqual([later.answerMode, later.hostBotId], ['mention', ''])
  assert.equal(normalizeRoomInput({ answerMode: 'bogus' }, current).answerMode, 'host')

  // 点了名的话不送给主持人。
  const host = { answerMode: 'host', hostBotId: KE }
  const plain = { mentions: [], recipients: [], replyToAuthor: '', text: '大家看看' }
  assert.equal(routesToHost(plain, host), true)
  assert.equal(routesToHost(plain, { answerMode: 'mention', hostBotId: KE }), false)
  assert.equal(routesToHost({ ...plain, mentions: [PRO] }, host), false)
  assert.equal(routesToHost({ ...plain, recipients: 'all' }, host), false)
  assert.equal(routesToHost({ ...plain, recipients: [PRO] }, host), false)
  assert.equal(routesToHost({ ...plain, replyToAuthor: PRO }, host), false)
  assert.equal(routesToHost({ ...plain, text: '@所有人 看看' }, host), false)

  // 分派结果：只留能叫醒的成员，写名字的也认，最多 3 个；不是 JSON、没有 bots 列表时失败。
  const eligible = [
    { id: KE, name: '小克' },
    { id: PRO, name: '小克pro' },
    { id: FISH, name: '肥鱼' },
    { id: DOG, name: '阿黄' },
  ]
  assert.deepEqual(
    parseRouteReply(
      `好的：{"bots": ["bot_nopeAAAAAA", "${PRO}", "@肥鱼", "${PRO}", "${KE}", "${DOG}"], "reason": " 一个  改代码，一个看文案 "}`,
      eligible,
    ),
    { bots: [PRO, FISH, KE], reason: '一个 改代码，一个看文案' },
  )
  assert.equal(ROOM_ROUTE_MAX_WAKES, 3)
  assert.equal(parseRouteReply('谁都行', eligible), null)
  assert.equal(parseRouteReply('{"reason": "x"}', eligible), null)
  assert.deepEqual(parseRouteReply('{"bots": []}', eligible), { bots: [], reason: '' })
})

test('the host routes an un-addressed message to the members it picks, at most three, never paused or outsiders', async () => {
  let release
  const gate = new Promise((resolve) => (release = resolve))
  const { runtime, room } = await hostRuntime(
    [
      JSON.stringify({
        bots: [PRO, 'bot_nopeAAAAAA', NAP, KE, FISH, DOG, CAT],
        reason: '小克pro 改代码，小克和肥鱼看看',
      }),
    ],
    { gate },
  )
  // 前情直接写进记录（不经分派）。
  await runtime.rooms.append(room.id, {
    author: { kind: 'user' },
    text: '先铺垫一下背景',
    mentions: [],
  })
  // 发送不等主持人：模型还没回，响应已经回来了，只说“在分派”。
  const sent = await runtime.postRoomMessage(room.id, { text: '撤回按钮好像坏了' })
  assert.deepEqual(sent.woke, [])
  assert.equal(sent.routing, true)
  assert.equal(runtime.runs.length, 0)
  release()
  const lines = await routed(runtime, room.id)
  // 只请了能叫醒的成员（不在群里的、暂停的不算），最多 3 个；主持人可以请自己。
  assert.deepEqual(
    runtime.runs.map((run) => runtime.roomSeatOf(run.sessionId).botId).sort(),
    [FISH, KE, PRO].sort(),
  )
  assert.equal(runtime.modelRuntime.prompts.length, 1)
  const prompt = runtime.modelRuntime.prompts[0]
  // 不带工具的独立请求，用主持人的模型；带着这句话、前面几条与每个成员的名字和职责。
  assert.deepEqual(prompt.tools, [])
  assert.equal(prompt.model.id, 'host-model')
  assert.match(prompt.text, /You are 小克, the host of the group room "前端小组"/)
  assert.match(prompt.text, /#1 user: 先铺垫一下背景/)
  assert.match(prompt.text, /New message #2 from the user:\n撤回按钮好像坏了/)
  assert.match(prompt.text, new RegExp(`- ${PRO}: 小克pro — 小克pro duty`))
  assert.doesNotMatch(prompt.text, /睡觉的/)
  // 群里写一行：主持人请了谁、为什么；用量记下来。
  assert.deepEqual(lines.at(-1).event, {
    botId: KE,
    seq: 2,
    type: 'routed',
    bots: [PRO, KE, FISH],
    note: '小克pro 改代码，小克和肥鱼看看',
  })
  assert.equal(runtime.usage.length, 1)
  assert.match(runtime.usage[0].key, new RegExp(`^room-route:${room.id}:`))
  // 被请来的 Bot 知道是主持人派的，看得见的是你的原话，按你说的话算可信度。
  const run = runtime.runs.find((item) => runtime.roomSeatOf(item.sessionId).botId === PRO)
  const [visible, context] = run.message.split(ATTACHMENT_MARKER)
  assert.equal(visible, '撤回按钮好像坏了')
  assert.match(context, /the room host 小克 picked you to answer it \(小克pro 改代码/)
  assert.equal(run.unattended, false)
})

test('the host is skipped for addressed messages; a bad, failed or empty answer wakes nobody and says so', async () => {
  const { runtime, room } = await hostRuntime([
    '我觉得应该让小克pro 来',
    new Error('provider down'),
    JSON.stringify({ bots: [], reason: '只是留个言' }),
    null,
  ])
  // 点了名：照常叫醒，不问主持人。
  const asked = await runtime.postRoomMessage(room.id, { text: '@肥鱼 看看' })
  assert.deepEqual(asked.woke, [FISH])
  assert.equal(asked.routing, undefined)
  await settle(runtime)
  assert.equal(runtime.modelRuntime.prompts.length, 0)
  runtime.runs.length = 0

  for (const text of ['一', '二', '三', '四'])
    await runtime.postRoomMessage(room.id, { text: `没点名的话${text}` })
  const lines = await routed(runtime, room.id)
  assert.equal(runtime.runs.length, 0)
  assert.deepEqual(
    lines.map((line) => [line.event.type, line.event.reason || '', line.event.note || '']),
    [
      ['route_failed', 'invalid', ''],
      ['route_failed', 'failed', ''],
      ['routed', '', '只是留个言'],
      // 请求出错或超时（没有回复）是“没问成”，不是“回复不对”。
      ['route_failed', 'failed', ''],
    ],
  )
  assert.deepEqual(lines[2].event.bots, [])

  // 主持人暂停了：不问模型，写一行。
  runtime.bots.get = ((get) => (id) => (id === KE ? { ...get(id), enabled: false } : get(id)))(
    runtime.bots.get,
  )
  const before = runtime.modelRuntime.prompts.length
  const last = await runtime.postRoomMessage(room.id, { text: '还有人吗' })
  const after = await routed(runtime, room.id)
  assert.equal(runtime.modelRuntime.prompts.length, before)
  assert.deepEqual(after.at(-1).event, {
    botId: KE,
    seq: last.message.seq,
    type: 'route_failed',
    reason: 'host_paused',
  })
})

test('the room page reads who answers safely from any Runtime and shows the host decision', async () => {
  const { parseRoom, parseRoomMessage } = await import('../../src/features/bots/rooms-api.ts')
  // 旧 Runtime 没有这两个字段：按“@ 谁谁答”。
  const old = parseRoom({ id: 'room_oldAAAAAAAA', name: '老群', members: [KE] })
  assert.deepEqual([old.answerMode, old.hostBotId], ['mention', ''])
  const hosted = parseRoom({
    id: 'room_newAAAAAAAA',
    name: '新群',
    answerMode: 'host',
    hostBotId: KE,
  })
  assert.deepEqual([hosted.answerMode, hosted.hostBotId], ['host', KE])
  assert.equal(parseRoom({ id: 'room_x', name: 'x', answerMode: 'boss' }).answerMode, 'mention')
  const line = parseRoomMessage({
    seq: 3,
    id: 'msg_AAAAAAAA',
    author: { kind: 'system' },
    event: { type: 'routed', botId: KE, bots: [PRO, 7, FISH], note: '一个改，一个看' },
  })
  assert.deepEqual([line.event.bots, line.event.note], [[PRO, FISH], '一个改，一个看'])
})

test('a recall says how many uploads a bot already took; a file being sent is not deleted under it', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [PRO] })
  const seen = await runtime.postRoomMessage(room.id, {
    text: '给 Bot 看过',
    attachments: [
      { kind: 'text', name: 'seen.txt', text: 'already delivered' },
      { kind: 'text', name: 'alone.txt', text: 'nobody read this' },
    ],
  })
  const [seenRef, aloneRef] = seen.message.attachments
  runtime.assets.get(seenRef.id).references.push({ source: 'attachment', sessionId: 'seat-1' })
  const result = await runtime.recallRoomMessage(room.id, seen.message.id)
  assert.equal(result.attachmentsKept, 1)
  assert.equal(runtime.assets.has(aloneRef.id), false)
  assert.equal(runtime.assets.has(seenRef.id), true)

  // 正在发的那条（归档了、还没写进记录）用着同一个文件：同时进行的撤回不能把它删掉。
  const first = await runtime.postRoomMessage(room.id, {
    text: '第一条',
    attachments: [{ kind: 'text', name: 'shared.txt', text: 'same bytes' }],
  })
  const sharedId = first.message.attachments[0].id
  runtime.holdRoomAsset(sharedId, 1)
  assert.equal((await runtime.recallRoomMessage(room.id, first.message.id)).attachmentsKept, 1)
  assert.equal(runtime.assets.has(sharedId), true)
  runtime.holdRoomAsset(sharedId, -1)
  assert.equal(await runtime.releaseRoomAttachments([{ id: sharedId }]), 1)
  // 发完之后不再占着。
  assert.equal(runtime.roomAssetHolds.size, 0)
})

test('one message waking several bots reads its attachment once; a lone room goes with its bot and its uploads', async () => {
  const runtime = await fakeRuntime()
  let reads = 0
  const read = runtime.getAssetDownload
  runtime.getAssetDownload = async (id) => {
    reads += 1
    return read(id)
  }
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO, FISH] })
  await runtime.postRoomMessage(room.id, { text: '@所有人 看图', attachments: [image()] })
  await settle(runtime)
  assert.equal(runtime.runs.length, 3)
  assert.equal(reads, 1)
  assert.ok(runtime.runs.every((run) => run.attachments[0].data === PNG))
  clearTimeout(runtime.roomAttachmentReadsTimer)

  const solo = await runtime.createRoom({ name: '只有它', members: [FISH] })
  const posted = await runtime.postRoomMessage(solo.id, {
    text: '留个文件',
    attachments: [{ kind: 'text', name: 'solo.txt', text: 'only in the solo room' }],
  })
  const soloAsset = posted.message.attachments[0].id
  await runtime.removeBotFromRooms(FISH)
  assert.equal(runtime.rooms.get(solo.id), null)
  assert.equal(runtime.assets.has(soloAsset), false)
})

test('a wake that cannot start says which kind of thing failed, never the error text', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [PRO] })
  runtime.promptFromChannel = async () => {
    throw pisperError('invalid', 'attachment', '/home/someone/secret/broken.pdf')
  }
  await runtime.postRoomMessage(room.id, { text: '@小克pro 看这个' })
  await settle(runtime)
  await new Promise((resolve) => setImmediate(resolve))
  const line = (await runtime.rooms.messages(room.id)).at(-1)
  assert.deepEqual(line.event, { type: 'wake_failed', botId: PRO, reason: 'attachment' })
})

test('the host sees the file names of an attachment-only message', async () => {
  const { runtime, room } = await hostRuntime([JSON.stringify({ bots: [], reason: '' })])
  await runtime.postRoomMessage(room.id, {
    text: '',
    attachments: [{ kind: 'text', name: '报表.csv', text: 'a,b\n1,2' }],
  })
  await routed(runtime, room.id)
  assert.match(JSON.stringify(runtime.modelRuntime.prompts.at(-1)), /\[attached: 报表\.csv\]/)
})

// “@ 谁谁答”的群：没点名的话由群里先判断该谁答。fast 为假时子代理没配“快速”档。
async function triageRuntime(
  replies,
  { gate = null, fast = true, members = [KE, PRO, FISH, NAP] } = {},
) {
  const runtime = await fakeRuntime({
    bots: [
      bot(KE, '小克'),
      bot(PRO, '小克pro'),
      bot(FISH, '肥鱼'),
      bot(NAP, '睡觉的', { enabled: false }),
    ],
  })
  runtime.modelRuntime = scriptedModel(replies, { gate })
  runtime.usage = []
  runtime.recordUsage = async (day, key, usage) => runtime.usage.push({ key, usage })
  runtime.providerPreferences = {
    resolveSubagentModel: async ({ tier }) => ({
      model:
        tier === 'fast'
          ? fast
            ? { provider: 'luna', id: 'fast-model' }
            : null
          : { provider: 'luna', id: 'bot-model' },
    }),
  }
  const room = await runtime.createRoom({ name: '前端小组', members })
  return { runtime, room }
}

const botsOf = (runtime) => runtime.runs.map((run) => runtime.roomSeatOf(run.sessionId).botId)

test('an un-addressed message is triaged: one small request picks who answers and only they wake', async () => {
  let release
  const gate = new Promise((resolve) => (release = resolve))
  const { runtime, room } = await triageRuntime(
    [JSON.stringify({ bots: [PRO, NAP, 'bot_nopeAAAAAA'], reason: '撤回按钮是它改的' })],
    { gate },
  )
  await runtime.rooms.append(room.id, {
    author: { kind: 'user' },
    text: '先铺垫一下背景',
    mentions: [],
  })
  // 发送不等判断：响应先回来，只说“在分派”，谁也没叫。
  const sent = await runtime.postRoomMessage(room.id, { text: '撤回按钮好像坏了' })
  assert.deepEqual([sent.woke, sent.routing], [[], true])
  assert.equal(runtime.runs.length, 0)
  release()
  const lines = await routed(runtime, room.id)
  // 只叫醒选中的、能叫醒的（暂停的、不在群里的不算）。
  assert.deepEqual(botsOf(runtime), [PRO])
  // 一次不带工具的请求，用子代理“快速”档；没有主持人的口吻，倾向只请一个。
  assert.equal(runtime.modelRuntime.prompts.length, 1)
  const prompt = runtime.modelRuntime.prompts[0]
  assert.deepEqual(prompt.tools, [])
  assert.equal(prompt.model.id, 'fast-model')
  assert.match(prompt.text, /^You route messages in the group room "前端小组"/)
  assert.doesNotMatch(prompt.text, /You are |\(you\)|睡觉的/)
  assert.match(prompt.text, /usually one\. A short follow-up/)
  assert.match(prompt.text, /#1 user: 先铺垫一下背景/)
  // 群里写一行群里请了谁、为什么（没有主持人）；用量记在群下。
  assert.deepEqual(lines.find((line) => line.event.type === 'triaged').event, {
    type: 'triaged',
    seq: 2,
    bots: [PRO],
    note: '撤回按钮是它改的',
  })
  assert.equal(runtime.usage.length, 1)
  assert.match(runtime.usage[0].key, new RegExp(`^room-triage:${room.id}:`))
  // 被选中的知道为什么是它，看得见的是你的原话；它不能跳过，写 [pass] 照原样贴出来。
  const [visible, context] = runtime.runs[0].message.split(ATTACHMENT_MARKER)
  assert.equal(visible, '撤回按钮好像坏了')
  assert.match(
    context,
    /the room picked you to answer it \(撤回按钮是它改的\)\. Answer in the room/,
  )
  assert.doesNotMatch(context, /reply exactly \[pass\]/)
  assert.equal(runtime.runs[0].unattended, false)
  await finishTurn(runtime, runtime.runs[0].sessionId, '[pass]')
  const card = (await runtime.rooms.messages(room.id)).find((message) => message.turn)
  assert.deepEqual([card.turn.state, card.text], ['done', '[pass]'])
  // 席位提示：只有叫醒说明里允许时才能跳过。
  assert.match(
    runtime.roomSeatPromptFor(runtime.runs[0].sessionId, runtime.bots.get(PRO)),
    /allows \[pass\] and nothing is for you, reply exactly \[pass\] and nothing else\. Never reply \[pass\] otherwise\./,
  )
})

test('a triage that fails or answers badly wakes everyone, who may pass; a message nobody needs to answer leaves a line', async () => {
  for (const reply of [new Error('provider down'), null, '我觉得小克pro 来']) {
    const { runtime, room } = await triageRuntime([reply])
    await runtime.postRoomMessage(room.id, { text: '大家看看' })
    const lines = await routed(runtime, room.id)
    assert.deepEqual(botsOf(runtime).toSorted(), [KE, PRO, FISH].toSorted())
    // 判断不了不写分派行（不然每次故障都刷一行），叫醒说明照实说“没点名”，允许跳过。
    assert.equal(lines.filter((line) => line.event.type === 'triaged').length, 0)
    assert.match(
      runtime.runs[0].message.split(ATTACHMENT_MARKER)[1],
      /The user posted #1 without addressing anyone\. If nothing in it is for you, reply exactly \[pass\]\./,
    )
    await finishTurn(runtime, runtime.runs[0].sessionId, '[pass]')
    const card = (await runtime.rooms.messages(room.id)).find((message) => message.turn)
    assert.equal(card.turn.state, 'skipped')
  }
  const { runtime, room } = await triageRuntime([JSON.stringify({ bots: [], reason: '只是道谢' })])
  await runtime.postRoomMessage(room.id, { text: '谢谢大家' })
  const lines = await routed(runtime, room.id)
  assert.equal(runtime.runs.length, 0)
  assert.deepEqual(lines.at(-1).event, { type: 'triaged', seq: 1, bots: [], note: '只是道谢' })
})

test('without a fast tier the triage uses the brief keeper model; a lone member and @everyone skip it', async () => {
  const { runtime, room } = await triageRuntime([JSON.stringify({ bots: [KE], reason: '' })], {
    fast: false,
  })
  await runtime.postRoomMessage(room.id, { text: '看看' })
  await routed(runtime, room.id)
  assert.equal(runtime.modelRuntime.prompts[0].model.id, 'bot-model')
  assert.deepEqual(botsOf(runtime), [KE])

  // @所有人：照旧叫醒每个成员、可以跳过，不问模型。
  const everyone = await triageRuntime([])
  await everyone.runtime.postRoomMessage(everyone.room.id, { text: '@所有人 收尾前还有顾虑吗' })
  await settle(everyone.runtime)
  assert.equal(everyone.runtime.modelRuntime.prompts.length, 0)
  assert.equal(everyone.runtime.runs.length, 3)

  // 只剩一个能叫醒的成员：直接叫它，不问模型；暂停的照常写一行。
  const lone = await triageRuntime([], { members: [FISH, NAP] })
  const sent = await lone.runtime.postRoomMessage(lone.room.id, { text: '看看' })
  assert.deepEqual([sent.woke, sent.routing], [[FISH], undefined])
  await settle(lone.runtime)
  assert.equal(lone.runtime.modelRuntime.prompts.length, 0)
  assert.match(
    lone.runtime.runs[0].message.split(ATTACHMENT_MARKER)[1],
    /The user sent #1 to you\./,
  )
  const notes = (await lone.runtime.rooms.messages(lone.room.id)).filter(
    (message) => message.author.kind === 'system',
  )
  assert.deepEqual(
    notes.map((message) => message.event.type),
    ['paused'],
  )

  // 主持人分派的群里明确选了“所有人”：叫醒每个成员，不经过判断。
  const { runtime: hosted, room: hostRoom } = await hostRuntime([])
  const all = await hosted.postRoomMessage(hostRoom.id, { text: '大家看看', recipients: 'all' })
  assert.deepEqual([all.woke.length, all.routing], [5, undefined])
})

test('a triage keeps order across messages and wakes nobody when the message is gone before it answers', async () => {
  const { runtime, room } = await triageRuntime([
    JSON.stringify({ bots: [KE], reason: '' }),
    JSON.stringify({ bots: [KE], reason: '' }),
  ])
  const first = await runtime.postRoomMessage(room.id, { text: '第一句' })
  const second = await runtime.postRoomMessage(room.id, { text: '第二句' })
  const lines = await routed(runtime, room.id)
  assert.deepEqual(
    lines.filter((line) => line.event.type === 'triaged').map((line) => line.event.seq),
    [first.message.seq, second.message.seq],
  )
  assert.match(runtime.modelRuntime.prompts[1].text, /#1 user: 第一句/)

  for (const undo of ['recall', 'host']) {
    let release
    const gate = new Promise((resolve) => (release = resolve))
    const { runtime, room } = await triageRuntime([new Error('down')], { gate })
    const sent = await runtime.postRoomMessage(room.id, { text: '看看' })
    if (undo === 'recall') await runtime.recallRoomMessage(room.id, sent.message.id)
    else await runtime.updateRoom(room.id, { answerMode: 'host', hostBotId: KE })
    release()
    const lines = await routed(runtime, room.id)
    // 失败时的“叫醒所有人”也不做。
    assert.equal(runtime.runs.length, 0)
    assert.equal(lines.filter((line) => line.event.type === 'triaged').length, 0)
  }
})

test('while the room works out who answers, the room detail says so and a recall stops the request', async () => {
  let release
  const gate = new Promise((resolve) => (release = resolve))
  const { runtime, room } = await triageRuntime([JSON.stringify({ bots: [PRO], reason: '' })], {
    gate,
  })
  runtime.roomFrames.length = 0
  const sent = await runtime.postRoomMessage(room.id, { text: '撤回按钮好像坏了' })
  // 发出去到有人接话之间：群详情里写着这句还在判断，推一帧让页面重读详情。
  assert.deepEqual((await runtime.getRoom(room.id)).routing, [
    { messageId: sent.message.id, seq: sent.message.seq, kind: 'triage' },
  ])
  assert.ok(runtime.roomFrames.some((frame) => frame.scope.join() === 'detail'))
  release()
  await routed(runtime, room.id)
  assert.deepEqual((await runtime.getRoom(room.id)).routing, [])
  assert.deepEqual(botsOf(runtime), [PRO])

  // 主持人分派的群：同样有这一行（kind 是 host）；判断中撤回就停下那次请求，不写“分派失败”。
  let open
  const held = new Promise((resolve) => (open = resolve))
  const hosted = await hostRuntime([JSON.stringify({ bots: [PRO], reason: '' })], { gate: held })
  let signal = null
  const stream = hosted.runtime.modelRuntime.streamSimple
  hosted.runtime.modelRuntime.streamSimple = (model, context, options) => {
    signal = options.signal
    return stream(model, context, options)
  }
  const asked = await hosted.runtime.postRoomMessage(hosted.room.id, { text: '谁来看看' })
  assert.equal((await hosted.runtime.getRoom(hosted.room.id)).routing[0].kind, 'host')
  await hosted.runtime.recallRoomMessage(hosted.room.id, asked.message.id)
  assert.deepEqual((await hosted.runtime.getRoom(hosted.room.id)).routing, [])
  open()
  const lines = await routed(hosted.runtime, hosted.room.id)
  assert.equal(signal?.aborted, true)
  assert.equal(hosted.runtime.runs.length, 0)
  assert.deepEqual(lines, [])
})

test('recalling the message a bot is answering stops that turn, unless it is also answering a message still there', async () => {
  const runtime = await fakeRuntime()
  const aborted = []
  runtime.abortSession = async (id) => aborted.push(id)
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  const first = await runtime.postRoomMessage(room.id, { text: '@小克pro 看看撤回' })
  await settle(runtime)
  const seat = runtime.runs[0].sessionId
  runtime.onSeatRunStart(seat)
  await runtime.roomSeatTurns.get(seat).ready
  // 这一轮还接着答了一句没撤回的：撤回第一句不停。
  const other = await runtime.rooms.append(room.id, {
    author: { kind: 'user' },
    text: '再看看这个',
    mentions: [PRO],
  })
  runtime.roomSeatTurns.get(seat).alsoAnswered = [other.id]
  assert.deepEqual((await runtime.recallRoomMessage(room.id, first.message.id)).stopped, [])
  assert.deepEqual(aborted, [])
  // 它答的话都撤回了：停下这一轮，告诉页面停了谁。
  assert.deepEqual((await runtime.recallRoomMessage(room.id, other.id)).stopped, [PRO])
  assert.deepEqual(aborted, [seat])

  // 送进去之后、开跑之前撤回：这一轮一开就停。
  const late = await fakeRuntime()
  const stops = []
  late.abortSession = async (id) => stops.push(id)
  const lateRoom = await late.createRoom({ name: '前端小组', members: [KE, PRO] })
  const asked = await late.postRoomMessage(lateRoom.id, { text: '@小克 看看' })
  await settle(late)
  await late.recallRoomMessage(lateRoom.id, asked.message.id)
  const lateSeat = late.runs[0].sessionId
  late.onSeatRunStart(lateSeat)
  await late.roomSeatTurns.get(lateSeat).ready
  assert.deepEqual(stops, [lateSeat])
})

test('a wake still queued when its message is recalled is dropped instead of starting a turn', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  const seat = await runtime.ensureSeatSession(room, runtime.bots.get(KE))
  runtime.sessions.set(seat, { session: { messages: [] } })
  // 席位正在开始别的一轮：叫醒先排着。
  runtime.startingRuns.set(seat, {})
  const asked = await runtime.postRoomMessage(room.id, { text: '@小克 看看' })
  await settle(runtime)
  assert.equal(runtime.runs.length, 0)
  await runtime.recallRoomMessage(room.id, asked.message.id)
  runtime.startingRuns.delete(seat)
  await runtime.flushSeatWake(room.id, KE)
  assert.equal(runtime.runs.length, 0)
  assert.equal(runtime.roomWakes.has(`${room.id}:${KE}`), false)
})

test('with nobody connected the board hub keeps no room frames and no timer', async () => {
  const hub = new BoardEventHub({
    listItems: async () => new Map(),
    roomState: () => ({ lastSeq: 1, signature: 'x' }),
    minIntervalMs: 20,
    coalesceMs: 5,
  })
  try {
    hub.roomChanged('room_AAAAAAAAAA', { scope: ['messages'] })
    assert.equal(hub.roomPending.size, 0)
    assert.equal(hub.roomTimer, null)
    assert.equal(
      hub.buffer.some((entry) => entry.event === 'room'),
      false,
    )
    // 连着的页面走了：待发的不留，定时器停掉。
    const connection = hub.connect({ send: () => {}, close: () => {} })
    await connection.ready
    hub.roomChanged('room_AAAAAAAAAA', { scope: ['messages'] })
    hub.roomChanged('room_AAAAAAAAAA', { scope: ['detail'] })
    assert.equal(hub.roomPending.size, 1)
    connection.detach()
    assert.equal(hub.roomPending.size, 0)
    assert.equal(hub.roomTimer, null)
  } finally {
    hub.dispose()
  }
})

test('a frame that only says messages changed does not refetch the room detail', async () => {
  const policy = await import('../../src/features/bots/room-live-policy.ts')
  const change = (scope) => ({ roomId: 'room_AAAAAAAAAA', lastSeq: 3, scope, deleted: false })
  assert.equal(policy.roomChangeNeedsDetail(change(['messages'])), false)
  assert.equal(policy.roomChangeNeedsDetail(change(['messages', 'detail'])), true)
  assert.equal(policy.roomChangeNeedsDetail(change([])), true)
  assert.equal(policy.roomChangeNeedsDetail('*'), true)
  assert.ok(policy.ROOM_FRAME_COALESCE_MS >= 300 && policy.ROOM_FRAME_COALESCE_MS <= 500)
})

test('messages that name nobody go to everyone in new rooms, or to the host in host rooms', () => {
  const plain = normalizeRoomInput({ name: '群', members: [KE, PRO] })
  assert.equal(plain.recipients, 'all')
  const hosted = normalizeRoomInput({
    name: '群',
    members: [KE, PRO],
    answerMode: 'host',
    hostBotId: KE,
  })
  assert.deepEqual(hosted.recipients, [])
  // 记着的选择不被无关的修改冲掉。
  const picked = normalizeRoomInput({ recipients: [PRO] }, plain)
  assert.deepEqual(normalizeRoomInput({ goal: '收尾' }, picked).recipients, [PRO])
  // 没有“只留言”：“@ 谁谁答”的群里空的“发给”就是所有人，选的人被移出群也一样。
  assert.equal(normalizeRoomInput({ recipients: [] }, plain).recipients, 'all')
  assert.equal(normalizeRoomInput({ members: [KE] }, picked).recipients, 'all')
  assert.equal(roomRecipients([], 'mention'), 'all')
  assert.deepEqual(roomRecipients([], 'host'), [])
  // 切换谁来回答时按新模式的默认来，除非这次一起给了“发给”。
  assert.deepEqual(normalizeRoomInput({ answerMode: 'host', hostBotId: PRO }, plain).recipients, [])
  assert.equal(normalizeRoomInput({ answerMode: 'mention' }, hosted).recipients, 'all')
  assert.deepEqual(
    normalizeRoomInput({ answerMode: 'mention', recipients: [PRO] }, hosted).recipients,
    [PRO],
  )
  // 老的群没有 answerMode、“发给”为空（以前的“只留言”）：加载时按所有人算。
  const old = { name: '老群', members: [KE], recipients: [] }
  assert.equal(normalizeRoomInput({}, old).recipients, 'all')
})

test('the composer counts the same wakes as the Runtime: naming someone skips Send to', async () => {
  const { composerWakeTargets } = await import('../../src/features/bots/room-presentation.ts')
  const members = [
    { id: KE, name: '小克' },
    { id: PRO, name: '小克pro' },
  ]
  const all = [KE, PRO]
  assert.deepEqual(composerWakeTargets({ text: '大家看看', members, chosen: all }), {
    addressed: false,
    mentionsAll: false,
    targets: all,
  })
  assert.deepEqual(composerWakeTargets({ text: '@小克pro 看下', members, chosen: all }).targets, [
    PRO,
  ])
  assert.deepEqual(
    composerWakeTargets({ text: '接着说', members, chosen: all, repliedBot: KE }).targets,
    [KE],
  )
  assert.deepEqual(composerWakeTargets({ text: '@所有人 收尾', members, chosen: [] }).targets, all)
  assert.deepEqual(composerWakeTargets({ text: '先记一下', members, chosen: [] }).targets, [])
})

test('working bots sit above the composer, finished turns become rows, long chats ask for a brief', async () => {
  const { roomStatusEntries, mergesWithPrevious, briefPromptStats, ROOM_BRIEF_PROMPT_CHARS } =
    await import('../../src/features/bots/room-presentation.ts')
  const { ROOM_WAKE_MIN_CHARS } = await import('../services/rooms/room-model.mjs')
  // 提示条的门槛就是一次叫醒最少能带的字数：两边改一边会让提示条提前或太晚出现。
  assert.equal(ROOM_BRIEF_PROMPT_CHARS, ROOM_WAKE_MIN_CHARS)
  const approval = { id: 'ap', toolName: 'bash', argsPreview: 'npm test', severity: 'standard' }
  const bot = (id, seq, botId, turn, extra = {}) => ({
    id,
    seq,
    at: `2026-11-10T10:0${seq}:00Z`,
    author: { kind: 'bot', botId },
    text: 'ok',
    recalled: null,
    replyTo: '',
    turn: { state: 'done', waiting: null, approval: null, pending: null, ...turn },
    ...extra,
  })
  const entries = roomStatusEntries([
    bot('a', 1, KE, { state: 'running' }),
    bot('b', 2, PRO, { state: 'waiting', waiting: 'approval', approval }),
    bot('c', 3, FISH, { pending: { waiting: 'approval', approval } }),
    bot('d', 4, KE, {}),
    bot('e', 5, PRO, { state: 'skipped' }),
    bot('f', 6, FISH, { state: 'running' }, { recalled: { seenBy: [] } }),
  ])
  assert.deepEqual(
    entries.map((entry) => [entry.message.id, entry.waiting, entry.pending]),
    [
      ['a', null, false],
      ['b', 'approval', false],
      ['c', 'approval', true],
    ],
  )
  assert.equal(entries[1].approval.id, 'ap')

  // “这一轮”只挂在真叫醒了所有人的那句下面：“发给”记着所有人、但这句 @ 了某个 Bot，就不是一轮。
  const { timelineItems } = await import('../../src/features/bots/room-presentation.ts')
  const said = (id, seq, text, mentions) => ({
    id,
    seq,
    at: '',
    author: { kind: 'user' },
    text,
    mentions,
    replyTo: '',
    recipients: 'all',
    turn: null,
    recalled: null,
  })
  const rounds = timelineItems([
    said('n1', 1, '@小克 看下', [KE]),
    bot('n2', 2, KE, { triggers: ['n1'], alsoAnswered: [] }),
    said('n3', 3, '大家看看', []),
    bot('n4', 4, KE, { triggers: ['n3'], alsoAnswered: [] }),
  ])
  assert.equal(rounds[0].round, null)
  assert.deepEqual(rounds[2].round, [{ botId: KE, state: 'replied' }])

  // 同一个人两分钟内接着说的合并头部；隔久了、换了人、回复了某条都不合并。
  const user = (id, at, extra = {}) => ({
    id,
    at,
    author: { kind: 'user' },
    replyTo: '',
    recalled: null,
    ...extra,
  })
  const first = user('u1', '2026-11-10T10:00:00Z')
  assert.equal(mergesWithPrevious(first, user('u2', '2026-11-10T10:01:30Z')), true)
  assert.equal(mergesWithPrevious(first, user('u3', '2026-11-10T10:03:00Z')), false)
  assert.equal(
    mergesWithPrevious(first, user('u4', '2026-11-10T10:00:30Z', { replyTo: 'x' })),
    false,
  )
  assert.equal(
    mergesWithPrevious(first, {
      ...user('u5', '2026-11-10T10:00:30Z'),
      author: { kind: 'bot', botId: KE },
    }),
    false,
  )

  // 没人维护简报、简报之后没整理的超过一次叫醒能带的量时才问；有维护的 Bot 时 Runtime 自己会让它整理。
  const long = Array.from({ length: 30 }, (_, index) => ({
    seq: index + 1,
    text: 'x'.repeat(1_000),
    recalled: null,
  }))
  assert.deepEqual(
    briefPromptStats({ messages: long, brief: null, lastSeq: 30, maintained: false }),
    { count: 30, chars: 30_000 },
  )
  assert.equal(
    briefPromptStats({ messages: long, brief: null, lastSeq: 30, maintained: true }),
    null,
  )
  assert.equal(
    briefPromptStats({ messages: long, brief: { upToSeq: 10 }, lastSeq: 30, maintained: false }),
    null,
  )
  assert.equal(
    briefPromptStats({ messages: long, brief: null, lastSeq: 30, maintained: false, skipUpTo: 20 }),
    null,
  )
})

test('the room page keeps the first look: list rows, status rows, a scrolling Send to row, no cards', async () => {
  const root = join(import.meta.dirname, '../..')
  const read = (file) => readFile(join(root, 'src/features/bots', file), 'utf8')
  const [timeline, composer, page, status] = await Promise.all([
    read('RoomTimeline.tsx'),
    read('RoomComposer.tsx'),
    read('RoomPage.tsx'),
    read('RoomStatusRows.tsx'),
  ])
  // 时间线：一个 feed，每条一个 article，roving tabindex 与“第几条 / 共几条”；不再有右侧气泡和工作卡。
  assert.match(timeline, /role="feed"/)
  assert.match(timeline, /aria-posinset=\{index \+ 1\}/)
  assert.match(timeline, /tabIndex=\{message\.id === activeId \? 0 : -1\}/)
  assert.match(timeline, /timelineKeyAction\(/)
  assert.doesNotMatch(timeline, /user-bubble|RoomWorkCard|items-end/)
  await assert.rejects(read('RoomWorkCard.tsx'), { code: 'ENOENT' })
  await assert.rejects(read('RoomRail.tsx'), { code: 'ENOENT' })
  // 在做的 Bot 是输入框上方的一行，读屏会念出变化；批准就在这一行上。
  assert.match(status, /aria-live="polite"/)
  assert.match(status, /onApprove\(entry\.message, approval\.id, true\)/)
  // “发给”：每个成员一个按下式芯片，多了横着滑；没有原生 select、没有下拉多选。
  assert.match(composer, /aria-pressed=\{pressed\}/)
  assert.match(composer, /overflow-x-auto/)
  assert.doesNotMatch(composer, /DropdownMenuCheckboxItem|<select/)
  // 没点名、发给所有人（不是主持人分派的群）：提示由群里判断谁答，而不是“会叫醒 N 个”。
  assert.match(composer, /const triaging = !idleLabel && !addressed && everyone && wakes >= 2/)
  assert.match(composer, /\{triaging \? \([\s\S]{0,400}room\.composer\.triageHint/)
  // Windows 自绘标题栏：面板没开时页头右边给窗口按钮让出 144px，开着时面板内容从按钮下面开始。
  assert.match(page, /\[\[data-custom-titlebar\]_&\]:pr-\[144px\]/)
  assert.match(page, /\[\[data-custom-titlebar\]_&\]:pt-\[52px\]/)
})
