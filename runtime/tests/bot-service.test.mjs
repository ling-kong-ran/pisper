import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { createApiHandler } from '../http/api-handler.mjs'
import { resolveRuntimeCapabilities } from '../runtime-capabilities.mjs'
import {
  BOT_AVATAR_CHARACTERS,
  BOT_AVATAR_COLORS,
  MAX_BOTS,
  normalizeBotInput,
} from '../services/bots/bot-model.mjs'
import { BotService } from '../services/bots/bot-service.mjs'

const base = { name: '审查员', instructions: '审查 PR', workspace: { cwd: '/repo' } }

// 校验错误的 “码/名词|detail”：码只有通用的几个，具体是哪条规则看 detail。
function codeOf(fn) {
  try {
    fn()
  } catch (error) {
    return `${error.code}/${error.params?.subject}|${error.params?.detail || ''}`
  }
  return ''
}

const FULL_ACCESS_REFUSED =
  /^invalid\/bot\|bots triggered from groups or by everyone cannot use full access$/

test('bot fields are validated and given safe defaults', () => {
  const bot = normalizeBotInput(base)
  // 没给头像时按名字稳定地挑角色与颜色；旧数据（一个字 + 颜色序号）映射到颜色名。
  assert.ok(BOT_AVATAR_CHARACTERS.includes(bot.avatar.character))
  assert.ok(BOT_AVATAR_COLORS.includes(bot.avatar.color))
  assert.deepEqual(normalizeBotInput(base).avatar, bot.avatar)
  assert.equal(
    normalizeBotInput({ ...base, avatar: { glyph: '审', color: 3 } }).avatar.color,
    'orange',
  )
  assert.deepEqual(
    normalizeBotInput({ ...base, avatar: { character: 'owl', color: 'teal' } }).avatar,
    {
      character: 'owl',
      color: 'teal',
    },
  )
  assert.deepEqual(bot.model, { tier: 'standard' })
  // Bot 是替用户干活的：新建默认危险操作才问；已经存下的权限沿用，不悄悄放宽。
  assert.equal(bot.executionMode, 'workspace-write')
  assert.equal(
    normalizeBotInput({ instructions: '改一下' }, { ...bot, executionMode: 'approval-required' })
      .executionMode,
    'approval-required',
  )
  // Bot 就是一个会话加上设定：工作区只有电脑与目录，没有分支与交付方式。
  assert.deepEqual(bot.workspace, { environmentId: 'local', cwd: bot.workspace.cwd })
  assert.equal('delivery' in bot, false)
  assert.equal(bot.runMode, 'plan')
  assert.equal(bot.tools, null)
  assert.deepEqual(bot.channels, [])

  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, name: '  ' })),
    'invalid/bot|name is required',
  )
  assert.match(
    codeOf(() => normalizeBotInput({ ...base, name: '代码 审查' })),
    /^invalid\/bot\|name: letters, digits/,
  )
  assert.match(
    codeOf(() => normalizeBotInput({ ...base, name: '/审查' })),
    /^invalid\/bot\|name: letters/,
  )
  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, name: 'x'.repeat(25) })),
    'invalid/bot|name: max 24 characters',
  )
  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, workspace: {} })),
    'invalid/folder|choose a working folder',
  )
  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, model: { ref: 'no-slash' } })),
    'invalid/model|use provider/model',
  )
})

test('commands are validated: unique, not just digits, with content; extra text only for tasks', () => {
  const command = (input) => ({ name: '查部署', kind: 'task', text: '查看部署状态', ...input })
  const commands = (list) => normalizeBotInput({ ...base, commands: list }).commands
  assert.deepEqual(normalizeBotInput(base).commands, [])
  assert.deepEqual(commands([command({ audience: 'everyone', allowExtra: true })]), [
    { name: '查部署', kind: 'task', text: '查看部署状态', audience: 'everyone', allowExtra: true },
  ])
  // 默认只给管理员；固定回复不带补充。
  assert.deepEqual(commands([command({ kind: 'reply', allowExtra: true })]), [
    { name: '查部署', kind: 'reply', text: '查看部署状态', audience: 'admins', allowExtra: false },
  ])
  for (const list of [
    [command({}), command({ name: '查部署 ' })],
    [command({ name: '12' })],
    [command({ text: '  ' })],
    [command({ name: '' })],
    Array.from({ length: 21 }, (_, index) => command({ name: `指令${index}` })),
    'x',
  ])
    assert.match(
      codeOf(() => normalizeBotInput({ ...base, commands: list })),
      /^invalid\/bot\|.*command/,
    )
  // 打开“所有人”的私聊规则同样不能配完全访问。
  const open = {
    platform: 'telegram',
    chatType: 'p2p',
    peerIds: null,
    senderIds: [],
    everyone: true,
  }
  assert.match(
    codeOf(() => normalizeBotInput({ ...base, channels: [open], executionMode: 'full-access' })),
    FULL_ACCESS_REFUSED,
  )
})

test('group chats without a sender list mean only the owner, and cannot grant full access', () => {
  // 名单为空不再拒绝：路由按“只有渠道创建者”处理（bot-routing.test）。
  const group = { platform: 'feishu', chatType: 'group', peerIds: ['oc_1'], senderIds: [] }
  assert.deepEqual(normalizeBotInput({ ...base, channels: [group] }).channels, [
    { ...group, everyone: false },
  ])
  assert.match(
    codeOf(() => normalizeBotInput({ ...base, channels: [group], executionMode: 'full-access' })),
    FULL_ACCESS_REFUSED,
  )
  const allowed = { ...group, senderIds: ['ou_a'] }
  assert.match(
    codeOf(() => normalizeBotInput({ ...base, channels: [allowed], executionMode: 'full-access' })),
    FULL_ACCESS_REFUSED,
  )
  // 私聊可以选完全访问。
  const p2p = { platform: 'telegram', chatType: 'p2p', peerIds: null, senderIds: [] }
  assert.equal(
    normalizeBotInput({ ...base, channels: [p2p], executionMode: 'full-access' }).executionMode,
    'full-access',
  )
  // 默认聊天必须落在渠道范围内。
  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, channels: [allowed], defaultIn: ['feishu:oc_2'] })),
    "invalid/channel|default chat must be one of the bot's chats",
  )
  assert.deepEqual(
    normalizeBotInput({ ...base, channels: [allowed], defaultIn: ['feishu:oc_1'] }).defaultIn,
    ['feishu:oc_1'],
  )
})

async function serviceFixture(t, { busy = new Set() } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-bots-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'pisper-bots.json')
  const resolved = []
  const make = () =>
    new BotService({
      path,
      validateDirectory: async (input) => {
        resolved.push(input)
        if (input === '/missing') throw new Error('ENOENT')
        return `/abs${input}`
      },
      isBusy: (id) => busy.has(id),
    })
  const service = make()
  await service.init()
  return { service, path, resolved, make }
}

test('bots persist with resolved directories, unique names and unique default chats', async (t) => {
  const { service, path, resolved, make } = await serviceFixture(t)
  const channel = { platform: 'feishu', chatType: 'group', peerIds: ['oc_1'], senderIds: ['ou_a'] }
  const reviewer = await service.create({
    ...base,
    channels: [channel],
    defaultIn: ['feishu:oc_1'],
  })
  assert.match(reviewer.id, /^bot_/)
  assert.equal(reviewer.workspace.cwd, '/abs/repo')

  await assert.rejects(service.create({ ...base, name: '审查员' }), {
    code: 'exists',
    params: { subject: 'bot', detail: '审查员' },
  })
  // 全角与大小写变体视为同名。
  await service.create({ ...base, name: 'Reviewer' })
  await assert.rejects(service.create({ ...base, name: 'ＲＥＶＩＥＷＥＲ' }), {
    code: 'exists',
    params: { subject: 'bot', detail: 'REVIEWER' },
  })
  await assert.rejects(
    service.create({ ...base, name: '日报', channels: [channel], defaultIn: ['feishu:oc_1'] }),
    { code: 'exists', params: { subject: 'bot', detail: 'default bot of this chat is 审查员' } },
  )
  await assert.rejects(service.create({ ...base, name: '巡检', workspace: { cwd: '/missing' } }), {
    code: 'not_found',
    statusCode: 400,
    params: { subject: 'folder', detail: 'missing or not accessible' },
  })
  await assert.rejects(
    service.create({ ...base, name: '巡检', workspace: { cwd: '/repo', environmentId: 'env_x' } }),
    { code: 'not_found', params: { subject: 'machine' } },
  )

  // 只改名字不重新解析目录；改目录才解析。
  const before = resolved.length
  const renamed = await service.update(reviewer.id, { name: '审查员2' })
  assert.equal(resolved.length, before)
  assert.equal(renamed.createdAt, reviewer.createdAt)
  await service.update(reviewer.id, { workspace: { cwd: '/other' } })
  assert.equal(service.get(reviewer.id).workspace.cwd, '/abs/other')
  assert.equal(await service.update('bot_missing', { name: 'x' }), null)

  await service.writeQueue
  if (process.platform !== 'win32') assert.equal((await stat(path)).mode & 0o777, 0o600)
  const reloaded = make()
  await reloaded.init()
  assert.deepEqual(
    reloaded.list().map((bot) => bot.name),
    ['审查员2', 'Reviewer'],
  )
})

test('broken or duplicate entries on disk are skipped, the rest still load', async (t) => {
  const { path, make } = await serviceFixture(t)
  await writeFile(
    path,
    JSON.stringify({
      version: 1,
      bots: [
        { id: 'bot_a', ...base, workspace: { cwd: '/abs/repo' } },
        { id: 'bot_b', ...base, workspace: { cwd: '/abs/repo' } },
        { id: 'bot_c', name: '坏 名字', workspace: { cwd: '/abs/repo' } },
        { id: 'not-a-bot-id', ...base, name: '日报', workspace: { cwd: '/abs/repo' } },
        { id: 'bot_d', ...base, name: '日报', workspace: { cwd: '/abs/repo' } },
      ],
    }),
  )
  const service = make()
  await service.init()
  assert.deepEqual(
    service.list().map((bot) => bot.id),
    ['bot_a', 'bot_d'],
  )
})

test('a busy bot cannot be deleted and the bot limit is enforced', async (t) => {
  const busy = new Set()
  const { service } = await serviceFixture(t, { busy })
  const bot = await service.create(base)
  busy.add(bot.id)
  await assert.rejects(service.remove(bot.id), {
    code: 'busy',
    statusCode: 409,
    params: { subject: 'bot', detail: 'stop or finish its jobs and approvals first' },
  })
  busy.delete(bot.id)
  assert.equal(await service.remove(bot.id), true)
  assert.equal(await service.remove(bot.id), false)
  for (let index = 0; index < MAX_BOTS; index += 1)
    await service.create({ ...base, name: `bot${index}` })
  await assert.rejects(service.create({ ...base, name: 'one-more' }), {
    code: 'limit',
    statusCode: 409,
  })
})

function request(method, body) {
  return {
    method,
    async *[Symbol.asyncIterator]() {
      if (body !== undefined) yield Buffer.from(JSON.stringify(body))
    },
  }
}

function response() {
  return {
    status: 0,
    body: '',
    destroyed: false,
    writableEnded: false,
    headersSent: false,
    writeHead(status) {
      this.status = status
    },
    flushHeaders() {},
    write(body = '') {
      this.body += body
    },
    end(body = '') {
      this.body += body
      this.writableEnded = true
    },
  }
}

async function call(handler, method, path, body) {
  const output = response()
  await handler(request(method, body), output, new URL(`http://localhost${path}`))
  return { status: output.status, body: output.body ? JSON.parse(output.body) : null }
}

test('the bots API is gated by the capability and the Labs switch and returns error codes', async (t) => {
  const { service } = await serviceFixture(t)
  let enabled = false
  const runtime = {
    capabilities: { features: { bots: true } },
    isBetaFeatureEnabled: (id) => id === 'bots' && enabled,
    listBots: () => service.list(),
    getBot: (id) => service.get(id),
    createBot: (input) => service.create(input),
    updateBot: (id, input) => service.update(id, input),
    deleteBot: (id) => service.remove(id),
  }
  const handler = createApiHandler(runtime)
  const off = await call(handler, 'GET', '/api/bots')
  assert.equal(off.status, 409)
  assert.equal(off.body.code, 'beta_feature_disabled')

  enabled = true
  const created = await call(handler, 'POST', '/api/bots', base)
  assert.equal(created.status, 201)
  const id = created.body.bot.id
  assert.equal((await call(handler, 'GET', '/api/bots')).body.bots.length, 1)
  assert.equal((await call(handler, 'GET', `/api/bots/${id}`)).body.bot.name, '审查员')
  const taken = await call(handler, 'POST', '/api/bots', base)
  assert.deepEqual(
    [taken.status, taken.body.code, taken.body.params],
    [409, 'exists', { subject: 'bot', detail: '审查员' }],
  )
  const bad = await call(handler, 'POST', '/api/bots', { ...base, name: 'a b' })
  assert.deepEqual([bad.status, bad.body.code, bad.body.params.subject], [400, 'invalid', 'bot'])
  const patched = await call(handler, 'PATCH', `/api/bots/${id}`, { enabled: false })
  assert.equal(patched.body.bot.enabled, false)
  assert.equal((await call(handler, 'GET', '/api/bots/bot_none')).body.code, 'bot_not_found')
  assert.equal((await call(handler, 'DELETE', `/api/bots/${id}`)).body.deleted, true)

  const gated = createApiHandler({ capabilities: { features: { bots: false } } })
  const unsupported = await call(gated, 'GET', '/api/bots')
  assert.deepEqual([unsupported.status, unsupported.body.capability], [409, 'bots'])
})

test('bots are a desktop capability', async () => {
  const modules = { childProcess: true, workerThreads: true, sqlite: true, wasm: true }
  for (const [profile, expected] of [
    ['desktop', true],
    ['mobile-embedded', false],
    ['mobile-store', false],
  ]) {
    const capabilities = await resolveRuntimeCapabilities({
      environment: { PISPER_RUNTIME_PROFILE: profile },
      moduleSupport: modules,
    })
    assert.equal(capabilities.features.bots, expected, profile)
  }
})

test('the bots store file is plain JSON', async (t) => {
  const { service, path } = await serviceFixture(t)
  await service.create(base)
  await service.writeQueue
  const stored = JSON.parse(await readFile(path, 'utf8'))
  assert.equal(stored.version, 1)
  assert.equal(stored.bots[0].name, '审查员')
})
