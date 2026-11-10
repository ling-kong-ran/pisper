// Bot 群聊：一份共用的聊天记录，Bot 不叫不说话；叫醒一次就是一轮，在自己的席位会话里读没读过的那段、
// 干活、把结果贴回群里。这里覆盖存储（撤回擦掉原文、坏文件只影响一个群）、叫醒规则、增量上下文、
// Bot 之间的链与上限，以及运行时的接线（用真实的 RoomService 和最小的假运行时）。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, writeFile, readdir } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { createApiHandler } from '../http/api-handler.mjs'
import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { roomMethods } from '../runtime/room-methods.mjs'
import { BotLinkService } from '../services/bots/bot-links.mjs'
import { normalizeBotInput } from '../services/bots/bot-model.mjs'
import {
  botWakeTargets,
  selectUnread,
  userWakeTargets,
  wakeBudgetChars,
} from '../services/rooms/room-context.mjs'
import {
  ROOM_CHAIN_MAX_WAKES,
  isPassReply,
  mentionsEveryone,
  normalizeBrief,
  normalizeUserRoomMessage,
  parseBriefReply,
  parseMentions,
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
  const rooms = new RoomService({ dataDir })
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
  })
  return runtime
}

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
  assert.deepEqual(nobody, { wake: [], paused: [] })
  const some = userWakeTargets(
    { mentions: [PRO], recipients: [KE], replyToAuthor: FISH, text: 'x' },
    members,
  )
  assert.deepEqual(some.wake, [
    { botId: PRO, kind: 'mention' },
    { botId: KE, kind: 'recipient' },
  ])
  assert.deepEqual(some.paused, [FISH])
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

test('nobody wakes without an @; a mention wakes only that bot in its own seat', async () => {
  const runtime = await fakeRuntime()
  const room = await runtime.createRoom({ name: '前端小组', members: [KE, PRO] })
  const quiet = await runtime.postRoomMessage(room.id, { text: '先铺垫一下背景' })
  assert.deepEqual(quiet.woke, [])
  const asked = await runtime.postRoomMessage(room.id, { text: '@小克pro 看看还缺什么' })
  await settle(runtime)
  assert.deepEqual(asked.woke, [PRO])
  assert.equal(runtime.runs.length, 1)
  const run = runtime.runs[0]
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
