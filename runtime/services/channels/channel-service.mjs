// 渠道服务：统一管理飞书/微信渠道的接入、消息收发、模板通知与作用域控制。
// 渠道收到的消息通过 agent.prompt 转成会话运行，并支持附件（图片/文件）。
import { extname } from 'node:path'
import { readJson, writeJsonAtomic } from '../../storage/json-file.mjs'
import { FeishuGateway } from './feishu-gateway.mjs'
import { FeishuOnboardingService } from './feishu-onboarding.mjs'
import {
  defaultTemplates,
  normalizeTemplates,
  NOTIFICATION_EVENTS,
  renderNotificationTemplate,
  sampleNotificationData,
  templateCatalog,
} from './notification-templates.mjs'
import { WeixinGateway } from './weixin-gateway.mjs'
import { WeixinOnboardingService } from './weixin-onboarding.mjs'

const PLATFORMS = new Set(['feishu', 'weixin'])
const TEMPLATE_PLATFORMS = new Set(['feishu', 'weixin', 'browser'])
const TEXT_EXTENSIONS = new Set([
  '.txt',
  '.md',
  '.json',
  '.js',
  '.jsx',
  '.ts',
  '.tsx',
  '.css',
  '.html',
  '.xml',
  '.yaml',
  '.yml',
  '.csv',
  '.log',
  '.py',
  '.java',
  '.go',
  '.rs',
  '.sh',
  '.ps1',
  '.toml',
  '.sql',
])
const DOCUMENT_EXTENSIONS = new Set([
  '.pdf',
  '.docx',
  '.pptx',
  '.xlsx',
  '.odt',
  '.odp',
  '.ods',
  '.rtf',
  '.epub',
])
const IMAGE_EXTENSIONS = new Set(['.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp'])

function defaultState() {
  return {
    version: 3,
    connections: { feishu: null, weixin: null },
    scopes: {},
    templates: defaultTemplates(),
  }
}

function scopeKey(platform, peerId) {
  return `${platform}:${peerId}`
}

function maskedId(value) {
  const id = String(value || '')
  return id.length > 10 ? `${id.slice(0, 7)}••••${id.slice(-4)}` : id
}

function attachmentFromResource(resource) {
  const extension = extname(resource.name || '').toLowerCase()
  if (resource.type === 'image' || IMAGE_EXTENSIONS.has(extension)) {
    const mimeType =
      resource.mimeType ||
      (extension === '.jpg' || extension === '.jpeg'
        ? 'image/jpeg'
        : extension === '.webp'
          ? 'image/webp'
          : extension === '.gif'
            ? 'image/gif'
            : 'image/png')
    return {
      kind: 'image',
      name: resource.name,
      mimeType,
      size: resource.buffer.length,
      data: resource.buffer.toString('base64'),
    }
  }
  if (TEXT_EXTENSIONS.has(extension))
    return {
      kind: 'text',
      name: resource.name,
      size: resource.buffer.length,
      text: resource.buffer.toString('utf8'),
    }
  if (DOCUMENT_EXTENSIONS.has(extension))
    return {
      kind: 'document',
      name: resource.name,
      size: resource.buffer.length,
      extension,
      data: resource.buffer.toString('base64'),
    }
  return null
}

function publicScope(key, scope) {
  return {
    key,
    platform: scope.platform,
    peerId: scope.peerId,
    chatType: scope.chatType || 'p2p',
    sessionId: scope.sessionId || '',
    title: scope.title || scope.peerId,
    cwd: scope.cwd || '',
    model: scope.model || '',
    lastMessage: scope.lastMessage || '',
    updatedAt: scope.updatedAt || null,
  }
}

function normalizeState(stored) {
  if (stored?.version === 3)
    return {
      version: 3,
      connections: {
        feishu: stored.connections?.feishu || null,
        weixin: stored.connections?.weixin || null,
      },
      scopes: stored.scopes && typeof stored.scopes === 'object' ? stored.scopes : {},
      templates: normalizeTemplates(stored.templates),
    }
  if (stored?.version === 2) {
    const scopes = Object.fromEntries(
      Object.entries(stored.scopes || {}).map(([peerId, scope]) => [
        scopeKey('feishu', peerId),
        { ...scope, platform: 'feishu', peerId },
      ]),
    )
    return {
      version: 3,
      connections: { feishu: stored.connection || null, weixin: null },
      scopes,
      templates: defaultTemplates(),
    }
  }
  return defaultState()
}

export class ChannelService {
  constructor({ path, cwd, agent, gatewayFactories = {}, onboardingFactories = {} }) {
    this.path = path
    this.cwd = cwd
    this.agent = agent
    this.state = defaultState()
    this.writeQueue = Promise.resolve()
    this.chatQueues = new Map()
    this.gateways = {
      feishu: gatewayFactories.feishu
        ? gatewayFactories.feishu({ onMessage: (message) => this.enqueue('feishu', message) })
        : new FeishuGateway({ onMessage: (message) => this.enqueue('feishu', message) }),
      weixin: gatewayFactories.weixin
        ? gatewayFactories.weixin({
            onMessage: (message) => this.enqueue('weixin', message),
            onSyncBuf: (value) => this.updateWeixinSync(value),
          })
        : new WeixinGateway({
            onMessage: (message) => this.enqueue('weixin', message),
            onSyncBuf: (value) => this.updateWeixinSync(value),
          }),
    }
    this.onboardings = {
      feishu: onboardingFactories.feishu
        ? onboardingFactories.feishu({
            onCompleted: (credentials) => this.completeOnboarding('feishu', credentials),
          })
        : new FeishuOnboardingService({
            onCompleted: (credentials) => this.completeOnboarding('feishu', credentials),
          }),
      weixin: onboardingFactories.weixin
        ? onboardingFactories.weixin({
            onCompleted: (credentials) => this.completeOnboarding('weixin', credentials),
          })
        : new WeixinOnboardingService({
            onCompleted: (credentials) => this.completeOnboarding('weixin', credentials),
          }),
    }
  }

  async init() {
    const stored = await readJson(this.path, defaultState())
    this.state = normalizeState(stored)
    const hasLegacyTemplateTargets = Object.values(stored?.templates || {}).some((template) =>
      Object.values(template?.channels || {}).some((variant) =>
        Object.hasOwn(variant || {}, 'targets'),
      ),
    )
    if (stored?.version !== 3 || hasLegacyTemplateTargets) await this.save()
    for (const platform of PLATFORMS) {
      const connection = this.state.connections[platform]
      if (connection && connection.enabled !== false) void this.connect(platform).catch(() => {})
    }
  }

  save() {
    const snapshot = JSON.parse(JSON.stringify(this.state))
    this.writeQueue = this.writeQueue
      .catch(() => {})
      .then(() => writeJsonAtomic(this.path, snapshot))
    return this.writeQueue
  }

  publicConnection(platform) {
    const connection = this.state.connections[platform]
    if (!connection) return null
    const live = this.gateways[platform].getStatus()
    const ownerId = platform === 'feishu' ? connection.ownerOpenId : connection.ownerUserId
    return {
      id: platform,
      type: platform,
      name:
        connection.name ||
        (platform === 'feishu' ? live.bot?.name || 'Pisper Agent' : '微信机器人'),
      enabled: connection.enabled !== false,
      accountId: maskedId(platform === 'feishu' ? connection.appId : connection.accountId),
      accessMode: connection.accessMode || 'owner',
      defaultCwd: connection.defaultCwd || this.cwd,
      replyModel: connection.replyModel || null,
      ownerConfigured: Boolean(ownerId),
      bot: live.bot || null,
      status: live.state,
      lastError: live.lastError || '',
      connectedAt: live.connectedAt || null,
      lastEventAt: live.lastEventAt || null,
    }
  }

  getState() {
    return {
      providers: [
        {
          type: 'feishu',
          name: '飞书应用机器人',
          description: '官方扫码创建，WebSocket 长连接，支持私聊与群聊 @',
        },
        {
          type: 'weixin',
          name: '微信',
          description: '腾讯 iLink Bot 扫码登录，支持个人微信私聊与媒体消息',
        },
      ],
      connections: {
        feishu: this.publicConnection('feishu'),
        weixin: this.publicConnection('weixin'),
      },
      scopes: Object.entries(this.state.scopes)
        .map(([key, scope]) => publicScope(key, scope))
        .sort((a, b) => new Date(b.updatedAt || 0) - new Date(a.updatedAt || 0)),
      templates: templateCatalog(this.state.templates),
    }
  }

  startOnboarding(platform) {
    if (!PLATFORMS.has(platform)) throw new Error('不支持这个渠道。')
    const options =
      platform === 'weixin'
        ? { localTokens: [this.state.connections.weixin?.token].filter(Boolean) }
        : undefined
    return this.onboardings[platform].start(options)
  }

  getOnboarding(platform, id) {
    return this.onboardings[platform]?.get(id) || null
  }

  cancelOnboarding(platform, id) {
    return this.onboardings[platform]?.cancel(id) || false
  }

  verifyOnboarding(platform, id, code) {
    if (platform !== 'weixin') throw new Error('该渠道不需要配对码。')
    return this.onboardings.weixin.verify(id, code)
  }

  async completeOnboarding(platform, credentials) {
    const current = this.state.connections[platform]
    const common = {
      name: platform === 'feishu' ? 'Pisper Agent' : '微信机器人',
      accessMode: 'owner',
      defaultCwd: current?.defaultCwd || this.cwd,
      replyModel: current?.replyModel || null,
      enabled: true,
      createdAt: current?.createdAt || new Date().toISOString(),
      updatedAt: new Date().toISOString(),
    }
    this.state.connections[platform] =
      platform === 'feishu'
        ? {
            ...common,
            appId: credentials.appId,
            appSecret: credentials.appSecret,
            ownerOpenId: credentials.ownerOpenId || '',
            domain: credentials.domain || 'feishu',
          }
        : {
            ...common,
            accountId: credentials.accountId,
            token: credentials.token,
            ownerUserId: credentials.ownerUserId || '',
            baseUrl: credentials.baseUrl,
            cdnBaseUrl: credentials.cdnBaseUrl,
            syncBuf: '',
          }
    for (const key of Object.keys(this.state.scopes))
      if (this.state.scopes[key].platform === platform) delete this.state.scopes[key]
    await this.save()
    const status = await this.connect(platform)
    if (platform === 'feishu' && status.bot?.name) {
      this.state.connections.feishu.name = status.bot.name
      await this.save()
    }
    return this.getState()
  }

  async connect(platform) {
    const connection = this.state.connections[platform]
    if (!connection) throw new Error(`请先扫码连接${platform === 'feishu' ? '飞书' : '微信'}。`)
    if (connection.enabled === false) return this.gateways[platform].getStatus()
    return this.gateways[platform].connect(connection)
  }

  async update(platform, input) {
    const connection = this.state.connections[platform]
    if (!connection) throw new Error('渠道尚未连接。')
    const enabledChanged = Object.hasOwn(input || {}, 'enabled')
    if (Object.hasOwn(input || {}, 'enabled')) connection.enabled = Boolean(input.enabled)
    if (Object.hasOwn(input || {}, 'accessMode'))
      connection.accessMode = input.accessMode === 'all' ? 'all' : 'owner'
    if (Object.hasOwn(input || {}, 'defaultCwd'))
      connection.defaultCwd = await this.agent.validateDirectory(input.defaultCwd)
    if (Object.hasOwn(input || {}, 'replyModel')) {
      const provider = String(input.replyModel?.provider || '')
      const model = String(input.replyModel?.model || '')
      connection.replyModel = provider && model ? { provider, model } : null
    }
    connection.updatedAt = new Date().toISOString()
    await this.save()
    if (enabledChanged) {
      if (connection.enabled) await this.connect(platform)
      else await this.gateways[platform].disconnect()
    }
    return this.getState()
  }

  async remove(platform) {
    await this.gateways[platform].disconnect()
    this.state.connections[platform] = null
    for (const key of Object.keys(this.state.scopes))
      if (this.state.scopes[key].platform === platform) delete this.state.scopes[key]
    await this.save()
    return true
  }

  async resetScope(key) {
    if (!this.state.scopes[key]) return false
    delete this.state.scopes[key]
    await this.save()
    return true
  }

  latestScope(platform) {
    return Object.values(this.state.scopes).reduce((latest, scope) => {
      if (scope.platform !== platform) return latest
      if (!latest) return scope
      return new Date(scope.updatedAt || 0).getTime() >= new Date(latest.updatedAt || 0).getTime()
        ? scope
        : latest
    }, null)
  }

  updateWeixinSync(value) {
    if (!this.state.connections.weixin || this.state.connections.weixin.syncBuf === value) return
    this.state.connections.weixin.syncBuf = value
    void this.save()
  }

  enqueue(platform, message) {
    const key = scopeKey(platform, message.peerId)
    const previous = this.chatQueues.get(key) || Promise.resolve()
    const next = previous.catch(() => {}).then(() => this.handleMessage(platform, message))
    this.chatQueues.set(key, next)
    next
      .finally(() => {
        if (this.chatQueues.get(key) === next) this.chatQueues.delete(key)
      })
      .catch(() => {})
  }

  async handleMessage(platform, message) {
    const connection = this.state.connections[platform]
    if (!connection?.enabled) return
    const ownerId = platform === 'feishu' ? connection.ownerOpenId : connection.ownerUserId
    if (connection.accessMode !== 'all' && (!ownerId || message.senderId !== ownerId)) {
      await this.gateways[platform].send(message, {
        text: ownerId
          ? '当前机器人仅允许扫码创建者使用。'
          : '未获取到创建者身份，请重新扫码或调整访问范围。',
      })
      return
    }
    const key = scopeKey(platform, message.peerId)
    const scope = this.state.scopes[key] || {}
    const command = message.content.trim().toLowerCase()
    if (command === '/new' || command === '/reset') {
      await this.resetScope(key)
      await this.gateways[platform].send(message, { text: '已开始新的 Pisper 会话。' })
      return
    }
    if (command === '/status') {
      await this.gateways[platform].send(message, {
        text: scope.sessionId
          ? `会话：${scope.sessionId}\n模型：${scope.model || '默认'}\n工作目录：${scope.cwd || connection.defaultCwd}`
          : '当前聊天还没有绑定 Pisper 会话。',
      })
      return
    }
    if (command === '/stop') {
      const stopped = scope.sessionId ? await this.agent.abort(scope.sessionId) : false
      await this.gateways[platform].send(message, {
        text: stopped ? '已停止当前任务。' : '当前没有运行中的任务。',
      })
      return
    }
    try {
      const resources = await this.gateways[platform].downloadResources(message.resources)
      const attachments = resources.map(attachmentFromResource).filter(Boolean)
      const prompt = message.content.trim() || (attachments.length ? '请分析这些附件。' : '')
      if (!prompt) return
      const result = await this.agent.prompt({
        sessionId: scope.sessionId || '',
        message: prompt,
        attachments,
        cwd: scope.cwd || connection.defaultCwd || this.cwd,
        title: `${platform === 'feishu' ? '飞书' : '微信'} · ${message.senderName || (message.chatType === 'p2p' ? '私聊' : '群聊')}`,
        model: connection.replyModel,
      })
      this.state.scopes[key] = {
        ...scope,
        platform,
        peerId: message.peerId,
        sessionId: result.sessionId,
        chatType: message.chatType,
        title:
          message.senderName ||
          `${platform === 'feishu' ? '飞书' : '微信'}${message.chatType === 'p2p' ? '私聊' : '群聊'}`,
        cwd: result.cwd || connection.defaultCwd || this.cwd,
        model:
          result.model ||
          (connection.replyModel
            ? `${connection.replyModel.provider}/${connection.replyModel.model}`
            : ''),
        contextToken: message.contextToken || scope.contextToken || '',
        lastMessage: prompt.slice(0, 120),
        updatedAt: new Date().toISOString(),
      }
      await this.save()
      await this.gateways[platform].send(message, { markdown: result.text || '任务已完成。' })
      for (const asset of result.assets || [])
        await this.gateways[platform].sendAsset(message.peerId, asset, this.state.scopes[key])
    } catch (error) {
      await this.gateways[platform]
        .send(message, {
          text: `执行失败：${error instanceof Error ? error.message : String(error)}`,
        })
        .catch(() => {})
    }
  }

  async updateTemplate(event, platform, input) {
    if (!NOTIFICATION_EVENTS[event] || !TEMPLATE_PLATFORMS.has(platform))
      throw new Error('通知模板类型不存在。')
    const template = this.state.templates[event]
    if (Object.hasOwn(input || {}, 'enabled')) template.enabled = Boolean(input.enabled)
    if (Object.hasOwn(input || {}, 'content')) {
      const content = String(input.content || '').trim()
      if (!content) throw new Error('通知模板不能为空。')
      template.channels[platform].content = content.slice(0, 12_000)
    }
    await this.save()
    return this.getState()
  }

  renderNotification(event, platform, data) {
    const definition = NOTIFICATION_EVENTS[event]
    const variant = this.state.templates[event]?.channels?.[platform]
    if (!definition || !variant) throw new Error('通知模板不存在。')
    return { title: definition.name, content: renderNotificationTemplate(variant.content, data) }
  }

  async notify(event, data, { platforms, content } = {}) {
    const template = this.state.templates[event]
    if (!template?.enabled) return []
    const selected = platforms ? new Set(platforms) : PLATFORMS
    const contentOverride = typeof content === 'string' ? content.slice(0, 12_000) : null
    const deliveries = []
    for (const platform of PLATFORMS) {
      if (!selected.has(platform)) continue
      const connection = this.state.connections[platform]
      if (!connection?.enabled) continue
      const scope = this.latestScope(platform)
      if (!scope) continue
      const renderedContent =
        contentOverride ?? this.renderNotification(event, platform, data).content
      deliveries.push({
        platform,
        promise: this.gateways[platform].sendToPeer(
          scope.peerId,
          platform === 'feishu' ? { markdown: renderedContent } : { text: renderedContent },
          scope,
        ),
      })
    }
    const settled = await Promise.allSettled(deliveries.map((delivery) => delivery.promise))
    return settled.map((result, index) =>
      result.status === 'fulfilled'
        ? { platform: deliveries[index].platform, status: 'fulfilled' }
        : {
            platform: deliveries[index].platform,
            status: 'rejected',
            error: result.reason instanceof Error ? result.reason.message : String(result.reason),
          },
    )
  }

  async testNotification(event, platform) {
    const variant = this.state.templates[event]?.channels?.[platform]
    if (!variant) throw new Error('通知模板不存在。')
    const scope = this.latestScope(platform)
    if (!scope) throw new Error('该渠道还没有可接收通知的历史会话。')
    const content = this.renderNotification(event, platform, sampleNotificationData()).content
    await this.gateways[platform].sendToPeer(
      scope.peerId,
      platform === 'feishu' ? { markdown: content } : { text: content },
      scope,
    )
    return { sent: 1, preview: content }
  }

  async dispose() {
    for (const onboarding of Object.values(this.onboardings)) onboarding.dispose()
    await Promise.all(Object.values(this.gateways).map((gateway) => gateway.disconnect()))
  }
}
