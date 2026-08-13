import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { ChannelService } from '../services/channels/channel-service.mjs'
import { FeishuOnboardingService } from '../services/channels/feishu-onboarding.mjs'
import { WeixinGateway } from '../services/channels/weixin-gateway.mjs'
import { WeixinProtocol, WEIXIN_API_BASE } from '../services/channels/weixin-protocol.mjs'

class FakeGateway {
  constructor({ onMessage, onSyncBuf }) {
    this.onMessage = onMessage
    this.onSyncBuf = onSyncBuf
    this.sent = []
    this.assets = []
    this.connectCount = 0
    this.status = { state: 'idle', lastError: '', bot: null }
  }

  getStatus() {
    return this.status
  }

  async connect(config) {
    this.config = config
    this.connectCount += 1
    this.status = {
      state: 'connected',
      lastError: '',
      connectedAt: new Date().toISOString(),
      bot: { name: 'Pisper Agent', openId: 'bot' },
    }
    return this.status
  }

  async disconnect() {
    this.status = { state: 'idle', lastError: '', bot: null }
  }

  async send(message, input) {
    this.sent.push({ peerId: message.peerId, input, replyTo: message.messageId })
  }

  async sendToPeer(peerId, input, scope) {
    this.sent.push({ peerId, input, scope })
  }

  async sendAsset(peerId, asset, scope) {
    this.assets.push({ peerId, asset, scope })
  }

  async downloadResources(resources = []) {
    return resources.filter((resource) => resource.buffer)
  }
}

async function fixture({ agent = {}, stored } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-channels-'))
  const path = join(directory, 'channels.json')
  if (stored) await writeFile(path, JSON.stringify(stored))
  const gateways = {}
  const gatewayFactory = (platform) => (options) => {
    const gateway = new FakeGateway(options)
    gateways[platform] = gateway
    return gateway
  }
  let sessionCounter = 0
  const service = new ChannelService({
    path,
    cwd: directory,
    agent: {
      prompt:
        agent.prompt ||
        (async ({ model }) => ({
          sessionId: `session-${++sessionCounter}`,
          text: 'Agent 回复',
          cwd: directory,
          model: model ? `${model.provider}/${model.model}` : 'openai/gpt-5',
          assets: [],
        })),
      abort: agent.abort || (async () => true),
      validateDirectory: agent.validateDirectory || (async (value) => value || directory),
    },
    gatewayFactories: { feishu: gatewayFactory('feishu'), weixin: gatewayFactory('weixin') },
    onboardingFactories: {
      feishu: () => ({
        start: async () => ({}),
        get: () => null,
        cancel: () => false,
        dispose: () => {},
      }),
      weixin: () => ({
        start: async () => ({}),
        get: () => null,
        cancel: () => false,
        verify: () => null,
        dispose: () => {},
      }),
    },
  })
  await service.init()
  return { directory, path, service, gateways }
}

test('legacy one-way webhook configuration is discarded during migration', async (t) => {
  const { directory, path, service } = await fixture({
    stored: { version: 1, channels: [{ type: 'feishu', webhookUrl: 'https://secret' }] },
  })
  t.after(() => rm(directory, { recursive: true, force: true }))
  const state = service.getState()
  assert.equal(state.connections.feishu, null)
  assert.equal(state.connections.weixin, null)
  assert.doesNotMatch(await readFile(path, 'utf8'), /webhookUrl|secret/)
})

test('version 2 Feishu state migrates to platform-scoped version 3 state', async (t) => {
  const stored = {
    version: 2,
    connection: {
      appId: 'cli_old',
      appSecret: 'secret',
      ownerOpenId: 'owner',
      enabled: false,
      defaultCwd: 'E:\\work',
    },
    scopes: { chat1: { sessionId: 'session-old', title: '旧会话', cwd: 'E:\\work' } },
  }
  const { directory, path, service } = await fixture({ stored })
  t.after(() => rm(directory, { recursive: true, force: true }))
  const state = service.getState()
  assert.equal(state.connections.feishu.accountId, 'cli_old')
  assert.equal(state.connections.weixin, null)
  assert.equal(state.scopes[0].key, 'feishu:chat1')
  assert.equal(JSON.parse(await readFile(path, 'utf8')).version, 3)
})

test('Feishu and WeChat credentials stay private while public identifiers are masked', async (t) => {
  const { directory, path, service, gateways } = await fixture()
  t.after(() => rm(directory, { recursive: true, force: true }))
  await service.completeOnboarding('feishu', {
    appId: 'cli_1234567890',
    appSecret: 'private-feishu-secret',
    ownerOpenId: 'owner',
    domain: 'feishu',
  })
  await service.completeOnboarding('weixin', {
    accountId: 'weixin_bot_1234567890',
    token: 'private-weixin-token',
    ownerUserId: 'wx-owner',
    baseUrl: WEIXIN_API_BASE,
    cdnBaseUrl: 'https://cdn.example',
  })
  const state = service.getState()
  assert.equal(state.connections.feishu.status, 'connected')
  assert.equal(state.connections.weixin.status, 'connected')
  assert.doesNotMatch(JSON.stringify(state), /private-feishu-secret|private-weixin-token/)
  assert.match(state.connections.feishu.accountId, /••••/)
  assert.match(state.connections.weixin.accountId, /••••/)
  assert.equal(gateways.feishu.config.appSecret, 'private-feishu-secret')
  assert.equal(gateways.weixin.config.token, 'private-weixin-token')
  const privateState = await readFile(path, 'utf8')
  assert.match(privateState, /private-feishu-secret/)
  assert.match(privateState, /private-weixin-token/)
})

test('channel reply model is passed to the Agent without reconnecting the transport', async (t) => {
  let promptInput
  const { directory, service, gateways } = await fixture({
    agent: {
      prompt: async (input) => {
        promptInput = input
        return {
          sessionId: 'pi-session',
          text: '**完成**',
          cwd: directory,
          model: 'openai/gpt-5.1',
          assets: [],
        }
      },
    },
  })
  t.after(() => rm(directory, { recursive: true, force: true }))
  await service.completeOnboarding('feishu', {
    appId: 'cli_app',
    appSecret: 'secret',
    ownerOpenId: 'owner',
  })
  await service.update('feishu', { replyModel: { provider: 'openai', model: 'gpt-5.1' } })
  assert.equal(gateways.feishu.connectCount, 1)
  await service.handleMessage('feishu', {
    messageId: 'm1',
    peerId: 'chat1',
    chatType: 'p2p',
    senderId: 'owner',
    senderName: '用户',
    content: '检查项目',
    resources: [],
  })
  assert.equal(promptInput.message, '检查项目')
  assert.deepEqual(promptInput.model, { provider: 'openai', model: 'gpt-5.1' })
  assert.equal(service.getState().scopes[0].sessionId, 'pi-session')
  assert.deepEqual(gateways.feishu.sent.at(-1).input, { markdown: '**完成**' })
})

test('personal WeChat peers map to independent Pisper sessions', async (t) => {
  let count = 0
  const { directory, service } = await fixture({
    agent: {
      prompt: async (input) => ({
        sessionId: input.sessionId || `wx-session-${++count}`,
        text: '收到',
        cwd: directory,
        model: 'google/gemini-2.5-pro',
        assets: [],
      }),
    },
  })
  t.after(() => rm(directory, { recursive: true, force: true }))
  await service.completeOnboarding('weixin', {
    accountId: 'wx-bot',
    token: 'token',
    ownerUserId: 'owner',
    baseUrl: WEIXIN_API_BASE,
  })
  await service.update('weixin', {
    accessMode: 'all',
    replyModel: { provider: 'google', model: 'gemini-2.5-pro' },
  })
  await service.handleMessage('weixin', {
    messageId: 'w1',
    peerId: 'user-1',
    senderId: 'user-1',
    chatType: 'p2p',
    content: '第一个会话',
    resources: [],
    contextToken: 'ctx-1',
  })
  await service.handleMessage('weixin', {
    messageId: 'w2',
    peerId: 'user-2',
    senderId: 'user-2',
    chatType: 'p2p',
    content: '第二个会话',
    resources: [],
    contextToken: 'ctx-2',
  })
  const scopes = service.getState().scopes
  assert.equal(scopes.length, 2)
  assert.notEqual(
    scopes.find((scope) => scope.peerId === 'user-1').sessionId,
    scopes.find((scope) => scope.peerId === 'user-2').sessionId,
  )
})

test('owner-only access rejects other users and /stop aborts the bound session', async (t) => {
  let aborted = ''
  const { directory, service, gateways } = await fixture({
    agent: {
      abort: async (id) => {
        aborted = id
        return true
      },
    },
  })
  t.after(() => rm(directory, { recursive: true, force: true }))
  await service.completeOnboarding('feishu', {
    appId: 'cli_app',
    appSecret: 'secret',
    ownerOpenId: 'owner',
  })
  await service.handleMessage('feishu', {
    messageId: 'm1',
    peerId: 'chat1',
    chatType: 'p2p',
    senderId: 'owner',
    content: '开始',
    resources: [],
  })
  await service.handleMessage('feishu', {
    messageId: 'm2',
    peerId: 'chat2',
    chatType: 'p2p',
    senderId: 'other',
    content: '继续',
    resources: [],
  })
  assert.match(gateways.feishu.sent.at(-1).input.text, /扫码创建者/)
  await service.handleMessage('feishu', {
    messageId: 'm3',
    peerId: 'chat1',
    chatType: 'p2p',
    senderId: 'owner',
    content: '/stop',
    resources: [],
  })
  assert.equal(aborted, 'session-1')
})

test('notification templates use platform-specific content and the latest channel conversation', async (t) => {
  const { directory, service, gateways } = await fixture()
  t.after(() => rm(directory, { recursive: true, force: true }))
  await service.completeOnboarding('feishu', {
    appId: 'cli_app',
    appSecret: 'secret',
    ownerOpenId: 'owner',
  })
  await service.completeOnboarding('weixin', {
    accountId: 'wx-bot',
    token: 'token',
    ownerUserId: 'wx-owner',
    baseUrl: WEIXIN_API_BASE,
  })
  await service.handleMessage('feishu', {
    messageId: 'm1',
    peerId: 'chat1',
    chatType: 'p2p',
    senderId: 'owner',
    senderName: '飞书用户',
    content: 'hello',
    resources: [],
  })
  await service.handleMessage('feishu', {
    messageId: 'm2',
    peerId: 'chat-latest',
    chatType: 'p2p',
    senderId: 'owner',
    senderName: '最近会话',
    content: 'latest',
    resources: [],
  })
  await service.handleMessage('weixin', {
    messageId: 'w1',
    peerId: 'wx-owner',
    chatType: 'p2p',
    senderId: 'wx-owner',
    senderName: '微信用户',
    content: 'hello',
    resources: [],
    contextToken: 'ctx',
  })
  await service.updateTemplate('schedule.completed', 'feishu', { content: '飞书：{{task.name}}' })
  await service.updateTemplate('schedule.completed', 'weixin', {
    content: '微信：{{task.summary}}',
  })
  await service.notify('schedule.completed', { task: { name: '日报', summary: '已完成' } })
  assert.deepEqual(gateways.feishu.sent.at(-1).input, { markdown: '飞书：日报' })
  assert.deepEqual(gateways.weixin.sent.at(-1).input, { text: '微信：已完成' })
  assert.equal(gateways.feishu.sent.at(-1).peerId, 'chat-latest')
  assert.equal(gateways.weixin.sent.at(-1).peerId, 'wx-owner')
  const weixinSendCount = gateways.weixin.sent.length
  await service.notify(
    'schedule.completed',
    { task: { name: '仅飞书', summary: '完成' } },
    { platforms: ['feishu'] },
  )
  assert.equal(gateways.weixin.sent.length, weixinSendCount)
  assert.equal(gateways.feishu.sent.at(-1).peerId, 'chat-latest')
  await service.notify(
    'schedule.completed',
    { task: { name: '忽略模板', summary: '忽略模板' } },
    { platforms: ['feishu', 'weixin'], content: '节点自定义正文' },
  )
  assert.deepEqual(gateways.feishu.sent.at(-1).input, { markdown: '节点自定义正文' })
  assert.deepEqual(gateways.weixin.sent.at(-1).input, { text: '节点自定义正文' })
  const tested = await service.testNotification('schedule.completed', 'weixin')
  assert.equal(tested.sent, 1)
  assert.match(tested.preview, /发现 2 个待处理问题/)
})

test('WeChat QR and authenticated requests use Tencent iLink endpoints and headers', async () => {
  const requests = []
  const protocol = new WeixinProtocol({
    fetchImpl: async (url, options) => {
      requests.push({ url: String(url), options })
      const body = String(url).includes('get_bot_qrcode')
        ? { qrcode: 'qr-id', qrcode_img_content: 'https://weixin.example/qr' }
        : String(url).includes('get_qrcode_status')
          ? { status: 'wait' }
          : { ret: 0 }
      return new Response(JSON.stringify(body), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    },
  })
  await protocol.startQr({ localTokens: ['old-token'] })
  await protocol.pollQr({ qrcode: 'qr-id' })
  await protocol.notifyStart({ baseUrl: WEIXIN_API_BASE, token: 'bot-token' })
  assert.equal(requests[0].url, `${WEIXIN_API_BASE}/ilink/bot/get_bot_qrcode?bot_type=3`)
  assert.equal(requests[0].options.method, 'POST')
  assert.equal(requests[0].options.headers.AuthorizationType, 'ilink_bot_token')
  assert.ok(requests[0].options.headers['X-WECHAT-UIN'])
  assert.equal(requests[0].options.headers.Authorization, undefined)
  assert.match(requests[0].options.body, /old-token/)
  assert.match(requests[1].url, /get_qrcode_status\?qrcode=qr-id$/)
  assert.equal(requests[1].options.method, 'GET')
  assert.equal(requests[1].options.headers['iLink-App-Id'], 'bot')
  assert.equal(requests[1].options.headers.Authorization, undefined)
  assert.equal(requests[2].options.headers.Authorization, 'Bearer bot-token')
})

test('WeChat gateway disconnect interrupts reconnect backoff immediately', async () => {
  let updateCalled
  const updateStarted = new Promise((resolve) => {
    updateCalled = resolve
  })
  const protocol = {
    notifyStart: async () => ({ ret: 0 }),
    notifyStop: async () => ({ ret: 0 }),
    getUpdates: async () => {
      updateCalled()
      throw new Error('temporary network error')
    },
  }
  const gateway = new WeixinGateway({ protocol, onMessage: () => {} })
  await gateway.connect({ token: 'token', syncBuf: '' })
  await updateStarted
  const startedAt = Date.now()
  await gateway.disconnect()
  assert.ok(Date.now() - startedAt < 500)
  assert.equal(gateway.getStatus().state, 'idle')
})

test('WeChat proactive sends use the latest persisted context token', async () => {
  const sent = []
  const gateway = new WeixinGateway({
    protocol: {
      sendText: async (_connection, input) => {
        sent.push(input)
        return { messageId: 'sent' }
      },
    },
    onMessage: () => {},
  })
  gateway.connection = { token: 'token' }
  await gateway.send({ peerId: 'wx-owner', contextToken: 'current-context' }, { text: 'reply' })
  await gateway.sendToPeer(
    'wx-owner',
    { text: 'scheduled notification' },
    { contextToken: 'stale-context' },
  )
  assert.equal(sent[0].contextToken, 'current-context')
  assert.equal(sent[1].contextToken, 'stale-context')
})

test('WeChat expired contexts return an actionable notification error', async () => {
  const protocol = new WeixinProtocol({
    fetchImpl: async () =>
      new Response(JSON.stringify({ ret: -2, errmsg: 'prepare failed' }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
  })
  await assert.rejects(
    protocol.sendText(
      { token: 'token' },
      { to: 'wx-owner', text: 'scheduled notification', contextToken: 'expired' },
    ),
    /请先在微信中向机器人发送一条消息/,
  )
})

test('channel notification results preserve platform delivery failures', async (t) => {
  const { directory, service, gateways } = await fixture()
  t.after(() => rm(directory, { recursive: true, force: true }))
  await service.completeOnboarding('weixin', {
    accountId: 'wx-bot',
    token: 'token',
    ownerUserId: 'wx-owner',
    baseUrl: WEIXIN_API_BASE,
  })
  await service.handleMessage('weixin', {
    messageId: 'w1',
    peerId: 'wx-owner',
    chatType: 'p2p',
    senderId: 'wx-owner',
    content: 'hello',
    resources: [],
    contextToken: 'ctx',
  })
  gateways.weixin.sendToPeer = async () => {
    throw new Error('prepare failed')
  }
  const results = await service.notify(
    'schedule.completed',
    { task: { name: 'daily', summary: 'done' } },
    { platforms: ['weixin'] },
  )
  assert.deepEqual(results, [{ platform: 'weixin', status: 'rejected', error: 'prepare failed' }])
})

test('official registerApp flow requests only bidirectional bot capabilities', async () => {
  let options
  const service = new FeishuOnboardingService({
    registerAppImpl: async (input) => {
      options = input
      input.onQRCodeReady({ url: 'https://accounts.feishu.cn/device', expireIn: 60 })
      return {
        client_id: 'cli_app',
        client_secret: 'secret',
        user_info: { open_id: 'owner', tenant_brand: 'feishu' },
      }
    },
    renderQr: async () => 'data:image/png;base64,qr',
    onCompleted: async () => {},
  })
  const job = await service.start()
  assert.equal(job.qrDataUrl, 'data:image/png;base64,qr')
  await service.jobs.get(job.id).promise
  assert.equal(options.createOnly, true)
  assert.deepEqual(options.addons.events.items.tenant, ['im.message.receive_v1'])
  assert.ok(options.addons.scopes.tenant.includes('im:message:send_as_bot'))
})
