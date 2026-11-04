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

function codeOf(fn) {
  try {
    fn()
  } catch (error) {
    return error.code
  }
  return ''
}

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
  assert.equal(bot.executionMode, 'approval-required')
  // Bot 就是一个会话加上设定：工作区只有电脑与目录，没有分支与交付方式。
  assert.deepEqual(bot.workspace, { environmentId: 'local', cwd: bot.workspace.cwd })
  assert.equal('delivery' in bot, false)
  assert.equal(bot.runMode, 'plan')
  assert.equal(bot.tools, null)
  assert.deepEqual(bot.channels, [])

  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, name: '  ' })),
    'bot_name_required',
  )
  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, name: '代码 审查' })),
    'bot_name_invalid',
  )
  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, name: '/审查' })),
    'bot_name_invalid',
  )
  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, name: 'x'.repeat(25) })),
    'bot_name_too_long',
  )
  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, workspace: {} })),
    'bot_cwd_required',
  )
  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, model: { ref: 'no-slash' } })),
    'bot_model_invalid',
  )
})

test('group chats need a sender allow-list and cannot grant full access', () => {
  const group = { platform: 'feishu', chatType: 'group', peerIds: ['oc_1'], senderIds: [] }
  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, channels: [group] })),
    'bot_group_needs_senders',
  )
  const allowed = { ...group, senderIds: ['ou_a'] }
  assert.equal(
    codeOf(() => normalizeBotInput({ ...base, channels: [allowed], executionMode: 'full-access' })),
    'bot_group_full_access',
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
    'bot_default_chat_invalid',
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

  await assert.rejects(service.create({ ...base, name: '审查员' }), { code: 'bot_name_taken' })
  // 全角与大小写变体视为同名。
  await service.create({ ...base, name: 'Reviewer' })
  await assert.rejects(service.create({ ...base, name: 'ＲＥＶＩＥＷＥＲ' }), {
    code: 'bot_name_taken',
  })
  await assert.rejects(
    service.create({ ...base, name: '日报', channels: [channel], defaultIn: ['feishu:oc_1'] }),
    { code: 'bot_default_chat_taken' },
  )
  await assert.rejects(service.create({ ...base, name: '巡检', workspace: { cwd: '/missing' } }), {
    code: 'directory_not_allowed',
  })
  await assert.rejects(
    service.create({ ...base, name: '巡检', workspace: { cwd: '/repo', environmentId: 'env_x' } }),
    { code: 'environment_not_found' },
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
  await assert.rejects(service.remove(bot.id), { code: 'invalid_state', statusCode: 409 })
  busy.delete(bot.id)
  assert.equal(await service.remove(bot.id), true)
  assert.equal(await service.remove(bot.id), false)
  for (let index = 0; index < MAX_BOTS; index += 1)
    await service.create({ ...base, name: `bot${index}` })
  await assert.rejects(service.create({ ...base, name: 'one-more' }), { code: 'bot_limit' })
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
  assert.deepEqual([taken.status, taken.body.code], [409, 'bot_name_taken'])
  const bad = await call(handler, 'POST', '/api/bots', { ...base, name: 'a b' })
  assert.deepEqual([bad.status, bad.body.code], [400, 'bot_name_invalid'])
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
