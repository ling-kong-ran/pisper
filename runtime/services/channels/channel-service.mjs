// 渠道服务：统一管理飞书、微信、QQ 与 Telegram 渠道的接入、消息收发、模板通知与作用域控制。
// 渠道收到的消息通过 agent.prompt 转成会话运行，并支持附件（图片/文件）。
import { extname } from 'node:path'
import { botChatAction } from '../bots/bot-notifications.mjs'
import {
  botAccess,
  botCommandPrompt,
  botCommandsFor,
  botMenuText,
  botsForChat,
  matchBotCommand,
  parseBotMention,
  refusalText,
  routeChannelMessage,
} from '../bots/bot-routing.mjs'
import QRCode from 'qrcode'
import { readJson, writeJsonAtomic } from '../../storage/json-file.mjs'
import { FeishuGateway } from './feishu-gateway.mjs'
import { FeishuOnboardingService } from './feishu-onboarding.mjs'
import { QQGateway } from './qq-gateway.mjs'
import { QQOnboardingService } from './qq-onboarding.mjs'
import { TelegramGateway } from './telegram-gateway.mjs'
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
import { normalizeExecutionMode } from '../../security/execution-mode.mjs'

const PLATFORMS = new Set(['feishu', 'weixin', 'qq', 'telegram'])
const MANUAL_PLATFORMS = new Set(['telegram'])
const TEMPLATE_PLATFORMS = new Set(['feishu', 'weixin', 'qq', 'telegram', 'browser'])
const CHANNEL_RUN_MODES = new Set(['plan', 'goal', 'team'])
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
    version: 5,
    connections: { feishu: null, weixin: null, qq: null, telegram: null },
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

// 渠道创建者在这个平台上的身份：飞书是 open_id，其余平台是用户 id。
function ownerIdOf(platform, connection) {
  if (!connection) return ''
  if (platform === 'feishu') return connection.ownerOpenId || ''
  return PLATFORMS.has(platform) ? connection.ownerUserId || '' : ''
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
    executionMode: normalizeExecutionMode(scope.executionMode),
    runMode: CHANNEL_RUN_MODES.has(scope.runMode) ? scope.runMode : 'plan',
    lastMessage: scope.lastMessage || '',
    updatedAt: scope.updatedAt || null,
  }
}

function normalizeState(stored) {
  if (stored?.version === 5 || stored?.version === 4 || stored?.version === 3)
    return {
      version: 5,
      connections: {
        feishu: stored.connections?.feishu || null,
        weixin: stored.connections?.weixin || null,
        // 旧版本可能保存了个人账号凭据；升级时直接丢弃，避免继续连接或保留敏感数据。
        qq: stored.connections?.qq?.mode === 'personal' ? null : stored.connections?.qq || null,
        telegram:
          stored.connections?.telegram?.mode === 'personal'
            ? null
            : stored.connections?.telegram || null,
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
      version: 5,
      connections: { feishu: stored.connection || null, weixin: null, qq: null, telegram: null },
      scopes,
      templates: defaultTemplates(),
    }
  }
  return defaultState()
}

// ask_user 在渠道里的等待通知：问题与选项编号列出，提示去 Pisper 应用里回答。
export function channelQuestionText(data) {
  const questions = Array.isArray(data?.questions) ? data.questions : []
  const lines = ['代理在等你回答：']
  questions.forEach((question, index) => {
    const prefix = questions.length > 1 ? `${index + 1}. ` : ''
    lines.push(
      `${prefix}${String(question?.question || '')}${question?.multiSelect ? '（可多选）' : ''}`,
    )
    const options = Array.isArray(question?.options) ? question.options : []
    options.forEach((option, optionIndex) => {
      lines.push(`   ${String.fromCharCode(65 + optionIndex)}. ${String(option?.label || '')}`)
    })
  })
  lines.push('请在 Pisper 应用里点选回答。')
  return lines.join('\n')
}

// /名字 状态 的回复：它此刻在不在说话、有没有在等人。
function botStatusText(bot, activity) {
  if (!bot.enabled) return `${bot.name} 已暂停。`
  if (activity?.status !== 'running') return `${bot.name} 现在空闲。`
  if (activity.waiting === 'approval') return `${bot.name} 在忙，正等人批准一个操作。`
  if (activity.waiting === 'question') return `${bot.name} 在忙，有问题等人回答。`
  return `${bot.name} 正在忙。`
}

// 指令清单发出后，回序号的有效时间。
const BOT_MENU_TTL_MS = 5 * 60_000
// 非管理员交给 Bot 做的指令：每人每 10 分钟最多 6 次，免得群里有人刷指令一直打断 Bot、耗光用量。
const PUBLIC_TASK_WINDOW_MS = 10 * 60_000
const PUBLIC_TASK_LIMIT = 6

export class ChannelService {
  constructor({ path, cwd, agent, bots = null, gatewayFactories = {}, onboardingFactories = {} }) {
    this.path = path
    this.cwd = cwd
    this.agent = agent
    // Bots（B3）：{ enabled(), list(), activity(botId), message(botId, input), act(botId, action) }；
    // 开关关闭时不参与分派。
    this.bots = bots
    // 叫过 Bot 的聊天的回复上下文（微信要 contextToken 才能主动发消息）；只在内存里，B5 回推用。
    this.botPeers = new Map()
    // 发过指令清单的人：几分钟内直接回序号就能选，不必再点名。键是“平台:聊天:人”，只在内存里。
    this.botMenus = new Map()
    // 非管理员最近交给 Bot 的指令时间，按“平台:人”记，只在内存里。
    this.publicTaskTimes = new Map()
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
      qq: gatewayFactories.qq
        ? gatewayFactories.qq({ onMessage: (message) => this.enqueue('qq', message) })
        : new QQGateway({ onMessage: (message) => this.enqueue('qq', message) }),
      telegram: gatewayFactories.telegram
        ? gatewayFactories.telegram({ onMessage: (message) => this.enqueue('telegram', message) })
        : new TelegramGateway({
            onMessage: (message) => this.enqueue('telegram', message),
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
      qq: onboardingFactories.qq
        ? onboardingFactories.qq({
            onCompleted: (credentials) => this.completeOnboarding('qq', credentials),
          })
        : new QQOnboardingService({
            onCompleted: (credentials) => this.completeOnboarding('qq', credentials),
          }),
      telegram: onboardingFactories.telegram || this.manualOnboarding('telegram'),
    }
  }

  manualOnboarding(platform) {
    return {
      start: async () => {
        const setupUrl = platform === 'qq' ? 'https://q.qq.com/qqbot/' : 'https://t.me/BotFather'
        return {
          mode: 'manual',
          platform,
          fields: platform === 'qq' ? ['appId', 'appSecret', 'token'] : ['token'],
          required: platform === 'qq' ? ['appId', 'appSecretOrToken'] : ['token'],
          setupUrl,
          qrDataUrl: await QRCode.toDataURL(setupUrl, {
            width: 180,
            margin: 2,
            errorCorrectionLevel: 'M',
          }),
        }
      },
      get: () => null,
      cancel: () => false,
      dispose: () => {},
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
    if (stored?.version !== 5 || hasLegacyTemplateTargets) await this.save()
    for (const platform of PLATFORMS) {
      const connection = this.state.connections[platform]
      if (connection && connection.enabled !== false) void this.connect(platform).catch(() => {})
    }
  }

  save() {
    this.writeQueue = this.writeQueue
      .catch(() => {})
      // 在写入队列执行时取快照，避免排队中的旧快照撤销刚确认的模板偏好。
      .then(() =>
        writeJsonAtomic(this.path, JSON.parse(JSON.stringify(this.state)), { mode: 0o600 }),
      )
    return this.writeQueue
  }

  publicConnection(platform) {
    const connection = this.state.connections[platform]
    if (!connection) return null
    const live = this.gateways[platform].getStatus()
    const ownerId = ownerIdOf(platform, connection)
    return {
      id: platform,
      type: platform,
      name:
        connection.name ||
        (platform === 'feishu'
          ? live.bot?.name || 'Pisper Agent'
          : platform === 'weixin'
            ? '微信机器人'
            : platform === 'qq'
              ? live.bot?.username || live.bot?.nickname || 'QQ 官方机器人'
              : live.bot?.username || live.bot?.name || 'Telegram Bot'),
      enabled: connection.enabled !== false,
      accountId: maskedId(
        platform === 'feishu'
          ? connection.appId
          : platform === 'qq'
            ? connection.appId
            : platform === 'telegram'
              ? live.bot?.id || connection.accountId
              : connection.accountId,
      ),
      accessMode: connection.accessMode || 'owner',
      defaultCwd: connection.defaultCwd || this.cwd,
      replyModel: connection.replyModel || null,
      executionMode: normalizeExecutionMode(connection.executionMode),
      runMode: CHANNEL_RUN_MODES.has(connection.runMode) ? connection.runMode : 'plan',
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
        {
          type: 'qq',
          name: 'QQ 官方机器人',
          description: '腾讯 QQ Bot Connector 扫码创建，使用官方 OpenAPI 与 WebSocket 接入',
          accessMode: 'qr',
        },
        {
          type: 'telegram',
          name: 'Telegram Bot',
          description: '使用 Telegram Bot API 长轮询接入',
          accessMode: 'manual',
        },
      ],
      connections: {
        feishu: this.publicConnection('feishu'),
        weixin: this.publicConnection('weixin'),
        qq: this.publicConnection('qq'),
        telegram: this.publicConnection('telegram'),
      },
      scopes: Object.entries(this.state.scopes)
        .map(([key, scope]) => publicScope(key, scope))
        .sort((a, b) => new Date(b.updatedAt || 0) - new Date(a.updatedAt || 0)),
      templates: templateCatalog(this.state.templates),
    }
  }

  startOnboarding(platform, input = undefined) {
    if (!PLATFORMS.has(platform)) throw new Error('不支持这个渠道。')
    if (input?.mode === 'personal') throw new Error('不支持个人账号接入，请使用官方机器人。')
    if (MANUAL_PLATFORMS.has(platform) || platform === 'qq') {
      if (input && typeof input === 'object' && Object.keys(input).length)
        return this.completeOnboarding(platform, input)
      return this.onboardings[platform].start()
    }
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

  async completeOnboarding(platform, credentials = {}) {
    if (!PLATFORMS.has(platform)) throw new Error('不支持这个渠道。')
    const current = this.state.connections[platform]
    const common = {
      name:
        platform === 'feishu'
          ? 'Pisper Agent'
          : platform === 'weixin'
            ? '微信机器人'
            : platform === 'qq'
              ? 'QQ 官方机器人'
              : 'Telegram Bot',
      accessMode: platform === 'telegram' ? 'all' : 'owner',
      defaultCwd: current?.defaultCwd || this.cwd,
      replyModel: current?.replyModel || null,
      executionMode: normalizeExecutionMode(current?.executionMode),
      runMode: CHANNEL_RUN_MODES.has(current?.runMode) ? current.runMode : 'plan',
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
        : platform === 'weixin'
          ? {
              ...common,
              accountId: credentials.accountId,
              token: credentials.token,
              ownerUserId: credentials.ownerUserId || '',
              baseUrl: credentials.baseUrl,
              cdnBaseUrl: credentials.cdnBaseUrl,
              syncBuf: '',
            }
          : platform === 'qq'
            ? {
                ...common,
                appId: String(credentials.appId || credentials.app_id || '').trim(),
                appSecret: String(
                  credentials.appSecret || credentials.appSecretOrToken || '',
                ).trim(),
                token: String(credentials.token || credentials.accessToken || '').trim(),
                ownerUserId: String(credentials.ownerUserId || '').trim(),
                baseUrl: credentials.baseUrl,
              }
            : {
                ...common,
                accountId: String(credentials.accountId || '').trim(),
                token: String(credentials.token || credentials.botToken || '').trim(),
                ownerUserId: String(credentials.ownerUserId || '').trim(),
                baseUrl: credentials.baseUrl,
                fileBaseUrl: credentials.fileBaseUrl,
              }
    for (const key of Object.keys(this.state.scopes))
      if (this.state.scopes[key].platform === platform) delete this.state.scopes[key]
    await this.save()
    const status = await this.connect(platform)
    if (status.bot?.name || status.bot?.username || status.bot?.nickname) {
      this.state.connections[platform].name =
        status.bot.name || status.bot.username || status.bot.nickname
      await this.save()
    }
    return this.getState()
  }

  async connect(platform) {
    const connection = this.state.connections[platform]
    if (!connection)
      throw new Error(
        `请先${MANUAL_PLATFORMS.has(platform) ? '填写凭据连接' : '扫码连接'}${
          platform === 'feishu'
            ? '飞书'
            : platform === 'weixin'
              ? '微信'
              : platform === 'qq'
                ? 'QQ'
                : 'Telegram'
        }。`,
      )
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
    if (Object.hasOwn(input || {}, 'executionMode'))
      connection.executionMode = normalizeExecutionMode(input.executionMode)
    if (Object.hasOwn(input || {}, 'runMode')) {
      const runMode = String(input.runMode || '')
      if (!CHANNEL_RUN_MODES.has(runMode)) throw new Error('运行模式无效。')
      connection.runMode = runMode
    }
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
    // 审批回复必须绕过当前聊天的运行队列，否则原任务会阻塞 /approve 自身。
    if (/^\/(?:approve|deny|yes|no)(?:\s|$)/i.test(String(message.content || '').trim())) {
      void this.handleMessage(platform, message).catch(() => {})
      return
    }
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
    const ownerId = ownerIdOf(platform, connection)
    if (connection.accessMode !== 'all' && (!ownerId || message.senderId !== ownerId)) {
      // 渠道本身只给创建者用；别人只能经 Bot 的聊天规则进来（规则里写了他、或打开了“所有人”）。
      if (
        this.bots?.enabled() &&
        (await this.handleBotMessage(platform, message, ownerId, { guest: true }))
      )
        return
      await this.gateways[platform].send(message, {
        text: ownerId
          ? '当前机器人仅允许扫码创建者使用。'
          : '未获取到创建者身份，请重新扫码或调整访问范围。',
      })
      return
    }
    const key = scopeKey(platform, message.peerId)
    const scope = this.state.scopes[key] || {}
    const command = message.content.trim()
    const normalizedCommand = command.toLowerCase()
    if (normalizedCommand === '/new' || normalizedCommand === '/reset') {
      await this.resetScope(key)
      await this.gateways[platform].send(message, { text: '已开始新的 Pisper 会话。' })
      return
    }
    if (normalizedCommand === '/status') {
      await this.gateways[platform].send(message, {
        text: scope.sessionId
          ? `会话：${scope.sessionId}\n模型：${scope.model || '默认'}\n工作目录：${scope.cwd || connection.defaultCwd}\n审批模式：${scope.executionMode || connection.executionMode || 'approval-required'}\n执行模式：${scope.runMode || connection.runMode || 'plan'}`
          : `当前聊天还没有绑定 Pisper 会话。\n审批模式：${scope.executionMode || connection.executionMode || 'approval-required'}\n执行模式：${scope.runMode || connection.runMode || 'plan'}`,
      })
      return
    }
    if (normalizedCommand === '/mode') {
      await this.gateways[platform].send(message, {
        text: `当前审批模式：${scope.executionMode || connection.executionMode || 'approval-required'}\n可选：approval-required、workspace-write、full-access\n用法：/mode <模式>`,
      })
      return
    }
    if (normalizedCommand.startsWith('/mode ')) {
      const modeParts = command.split(/\s+/)
      const mode = normalizeExecutionMode(modeParts[1], '')
      if (
        modeParts.length !== 2 ||
        !mode ||
        !['approval-required', 'workspace-write', 'full-access'].includes(mode)
      ) {
        await this.gateways[platform].send(message, {
          text: '用法：/mode approval-required|workspace-write|full-access',
        })
        return
      }
      if (scope.sessionId && this.agent.setExecutionMode)
        await this.agent.setExecutionMode(scope.sessionId, mode)
      this.state.scopes[key] = { ...scope, platform, peerId: message.peerId, executionMode: mode }
      await this.save()
      await this.gateways[platform].send(message, { text: `审批模式已切换为 ${mode}。` })
      return
    }
    if (normalizedCommand === '/run') {
      await this.gateways[platform].send(message, {
        text: `当前执行模式：${scope.runMode || connection.runMode || 'plan'}\n可选：plan、goal、team\n用法：/run <模式>`,
      })
      return
    }
    if (normalizedCommand.startsWith('/run ')) {
      const runParts = command.split(/\s+/)
      const runMode = runParts[1]?.toLowerCase() || ''
      if (runParts.length !== 2 || !CHANNEL_RUN_MODES.has(runMode)) {
        await this.gateways[platform].send(message, { text: '用法：/run plan|goal|team' })
        return
      }
      if (scope.sessionId && this.agent.setRunMode) {
        try {
          await this.agent.setRunMode(scope.sessionId, runMode)
        } catch (error) {
          await this.gateways[platform].send(message, {
            text: error instanceof Error ? error.message : String(error),
          })
          return
        }
      }
      this.state.scopes[key] = { ...scope, platform, peerId: message.peerId, runMode }
      await this.save()
      await this.gateways[platform].send(message, { text: `执行模式已切换为 ${runMode}。` })
      return
    }
    if (normalizedCommand === '/dir') {
      await this.gateways[platform].send(message, {
        text: `当前工作目录：${scope.cwd || connection.defaultCwd}\n用法：/dir <path>`,
      })
      return
    }
    if (normalizedCommand.startsWith('/dir ')) {
      const requested = command.slice(command.search(/\s/) + 1).trim()
      if (!requested) {
        await this.gateways[platform].send(message, { text: '用法：/dir <path>' })
        return
      }
      const directory = await this.agent.validateDirectory(requested)
      if (scope.sessionId && this.agent.setCwd) {
        try {
          await this.agent.setCwd(scope.sessionId, directory)
        } catch (error) {
          await this.gateways[platform].send(message, {
            text: error instanceof Error ? error.message : String(error),
          })
          return
        }
      }
      this.state.scopes[key] = { ...scope, platform, peerId: message.peerId, cwd: directory }
      await this.save()
      await this.gateways[platform].send(message, { text: `工作目录已切换为 ${directory}。` })
      return
    }
    if (/^\/(?:approve|deny|yes|no)(?:\s|$)/i.test(command)) {
      const approved = /^(?:\/approve|\/yes)(?:\s|$)/i.test(command)
      const approvalId = command.split(/\s+/)[1] || scope.pendingApprovalId || ''
      const resolved =
        approvalId && this.agent.resolveApproval && scope.sessionId
          ? await this.agent.resolveApproval(scope.sessionId, approvalId, approved)
          : { found: false }
      await this.gateways[platform].send(message, {
        text: resolved?.found
          ? approved
            ? '已批准，继续执行。'
            : '已拒绝本次操作。'
          : '没有找到待处理的审批请求。',
      })
      return
    }
    if (normalizedCommand === '/stop') {
      const stopped = scope.sessionId ? await this.agent.abort(scope.sessionId) : false
      await this.gateways[platform].send(message, {
        text: stopped ? '已停止当前任务。' : '当前没有运行中的任务。',
      })
      return
    }
    if (this.bots?.enabled() && (await this.handleBotMessage(platform, message, ownerId))) return
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
        title: `${platform === 'feishu' ? '飞书' : platform === 'weixin' ? '微信' : platform === 'qq' ? 'QQ' : 'Telegram'} · ${message.senderName || (message.chatType === 'p2p' ? '私聊' : '群聊')}`,
        model: connection.replyModel,
        executionMode: scope.executionMode || connection.executionMode,
        goalMode: (scope.runMode || connection.runMode || 'plan') === 'goal',
        teamMode: (scope.runMode || connection.runMode || 'plan') === 'team',
        onEvent: (event, data) => {
          if (event === 'permission_request') {
            // 首条消息触发的审批到达时作用域尚未写入 sessionId，必须从这里补齐，
            // 否则运行期间收到的 /approve 找不到会话而无法放行。
            this.state.scopes[key] = {
              ...this.state.scopes[key],
              platform,
              peerId: message.peerId,
              sessionId: this.state.scopes[key]?.sessionId || data.sessionId || '',
              pendingApprovalId: data.id,
            }
            void this.save()
            void this.gateways[platform]
              .send(message, {
                text: `需要审批：${data.toolName || '工具'}\n${data.reason || ''}\n请回复 /approve 批准，或 /deny 拒绝。`,
              })
              .catch(() => {})
          } else if (event === 'question_request') {
            // 渠道只带出问题与选项文字，回答在应用里完成（规格 8.4；渠道回复映射是后续工作）。
            void this.gateways[platform]
              .send(message, { text: channelQuestionText(data) })
              .catch(() => {})
          } else if (event === 'permission_resolved' && this.state.scopes[key]) {
            delete this.state.scopes[key].pendingApprovalId
            void this.save()
          }
        },
      })
      this.state.scopes[key] = {
        ...scope,
        platform,
        peerId: message.peerId,
        sessionId: result.sessionId,
        chatType: message.chatType,
        title:
          message.senderName ||
          `${platform === 'feishu' ? '飞书' : platform === 'weixin' ? '微信' : platform === 'qq' ? 'QQ' : 'Telegram'}${message.chatType === 'p2p' ? '私聊' : '群聊'}`,
        cwd: result.cwd || connection.defaultCwd || this.cwd,
        model:
          result.model ||
          (connection.replyModel
            ? `${connection.replyModel.provider}/${connection.replyModel.model}`
            : ''),
        contextToken: message.contextToken || scope.contextToken || '',
        // 谁在这个聊天里说话：bot_notify 只发到创建者本人的私聊，靠它分辨。
        senderId: message.senderId || scope.senderId || '',
        executionMode: scope.executionMode || connection.executionMode || 'approval-required',
        runMode: scope.runMode || connection.runMode || 'plan',
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

  // 点名了某个 Bot 的消息交给它；返回 false 表示与 Bot 无关，照旧直接对话。
  // 渠道消息交给 Bot：管理员什么都能让它做，其他人（规则打开了“所有人”）只能用公开的指令。
  // guest：渠道本身不对这个人开放，只有确实交给了某个 Bot 才算处理；没处理时由调用方照旧拒绝。
  async handleBotMessage(platform, message, ownerId, { guest = false } = {}) {
    const gateway = this.gateways[platform]
    const bots = this.bots.list()
    const command = message.content.trim().toLowerCase()
    const context = { ...message, platform, content: message.content }
    if (command === '/whoami') {
      await gateway.send(message, {
        text: `渠道：${platform}\n这个聊天：${message.peerId}\n你的 ID：${message.senderId}\n在 Bot 的“可以叫它的聊天”里填这两个 ID。`,
      })
      return true
    }
    if (command === '/bots') {
      const available = botsForChat(context, bots, { ownerId })
      if (guest && !available.length) return false
      await gateway.send(message, {
        text: available.length
          ? `这里可以叫的 Bot：\n${available.map((bot) => `· ${bot.name}`).join('\n')}\n用法：@ 机器人后写“名字 要做的事”（也可以用 /名字）；只写名字会列出能用的指令。`
          : '这个聊天里还没有可以叫的 Bot。在 Pisper 里给 Bot 加上这个聊天（/whoami 查看 ID）。',
      })
      return true
    }
    // 刚收到指令清单的人直接回序号：不必再点名。只认序号，免得把闲聊当成指令。
    const menuKey = `${platform}:${message.peerId}:${message.senderId}`
    const menu = this.botMenus.get(menuKey)
    if (menu && menu.expiresAt > Date.now()) {
      const parsed = parseBotMention(message.content, bots)
      if (!parsed.bot && !parsed.command && /^\d{1,2}(?:$|[\s,，:：、.．])/.test(parsed.text)) {
        const bot = bots.find((item) => item.id === menu.botId)
        const access = bot ? botAccess(bot, context, { ownerId }) : { reason: 'not_in_chat' }
        if (bot && access.role) {
          this.botMenus.delete(menuKey)
          return this.runBotRequest(platform, message, ownerId, bot, access.role, parsed.text)
        }
      }
    } else if (menu) this.botMenus.delete(menuKey)
    const routed = routeChannelMessage(context, bots, { ownerId })
    if (routed.kind === 'none') return false
    if (routed.kind === 'refused') {
      if (guest) return false
      await gateway.send(message, { text: refusalText(routed.bot, routed.reason) })
      return true
    }
    return this.runBotRequest(platform, message, ownerId, routed.bot, routed.role, routed.text)
  }

  // 点名之后的一句话：指令（序号或名字）、批准与状态（只给管理员），或交给 Bot 的话（只给管理员）。
  async runBotRequest(platform, message, ownerId, bot, role, text) {
    const gateway = this.gateways[platform]
    const admin = role === 'admin'
    this.botPeers.set(scopeKey(platform, message.peerId), {
      platform,
      peerId: message.peerId,
      chatType: message.chatType,
      contextToken: message.contextToken || '',
      senderId: message.senderId || '',
      updatedAt: new Date().toISOString(),
    })
    const showMenu = async (prefix = '') => {
      const commands = botCommandsFor(bot, role)
      if (commands.length) {
        this.#rememberMenu(`${platform}:${message.peerId}:${message.senderId}`, bot.id)
      }
      await gateway.send(message, {
        text: `${prefix}${botMenuText(bot, role, { group: message.chatType !== 'p2p' })}`,
      })
      return true
    }
    if (!text) return showMenu()
    const action = botChatAction(text)
    if (action || /^(?:状态|status)$/i.test(text)) {
      if (!admin) {
        await gateway.send(message, { text: refusalText(bot, 'admin_only') })
        return true
      }
      if (action) {
        const reply = await this.bots
          .act(bot.id, action)
          .catch((error) => (error instanceof Error ? error.message : String(error)))
        await gateway.send(message, { text: reply })
        return true
      }
      const activity = this.bots.activity(bot.id)
      await gateway.send(message, { text: botStatusText(bot, activity) })
      return true
    }
    const matched = matchBotCommand(text, botCommandsFor(bot, role))
    if (matched && (!matched.extra || matched.command.allowExtra)) {
      if (matched.command.kind === 'reply') {
        await gateway.send(message, { text: matched.command.text })
        return true
      }
      if (!admin && !this.#allowPublicTask(`${platform}:${message.senderId}`)) {
        await gateway.send(message, { text: `用得太频繁了，过几分钟再叫 ${bot.name}。` })
        return true
      }
      return this.sendToBot(
        platform,
        message,
        ownerId,
        bot,
        botCommandPrompt(matched.command, matched.extra, {
          admin,
          senderName: String(message.senderName || ''),
        }),
        matched.command.name,
      )
    }
    // 管理员没对上指令（或给不带补充的指令加了话）：整句交给 Bot；其他人只能用指令，列给他看。
    if (admin) return this.sendToBot(platform, message, ownerId, bot, text, '')
    return showMenu(
      matched ? `「${matched.command.name}」后面不能带补充。\n` : '这里只能用下面的指令。\n',
    )
  }

  #allowPublicTask(key) {
    const now = Date.now()
    const recent = (this.publicTaskTimes.get(key) || []).filter(
      (time) => now - time < PUBLIC_TASK_WINDOW_MS,
    )
    if (recent.length >= PUBLIC_TASK_LIMIT) {
      this.publicTaskTimes.set(key, recent)
      return false
    }
    recent.push(now)
    this.publicTaskTimes.set(key, recent)
    if (this.publicTaskTimes.size > 1000)
      for (const [entry, times] of this.publicTaskTimes)
        if (!times.some((time) => now - time < PUBLIC_TASK_WINDOW_MS))
          this.publicTaskTimes.delete(entry)
    return true
  }

  #rememberMenu(key, botId) {
    const now = Date.now()
    for (const [entry, value] of this.botMenus)
      if (value.expiresAt <= now) this.botMenus.delete(entry)
    // 极端情况下（很多人同时点名）只留最近的一批，不让内存无限长。
    if (this.botMenus.size >= 1000) this.botMenus.delete(this.botMenus.keys().next().value)
    this.botMenus.set(key, { botId, expiresAt: now + BOT_MENU_TTL_MS })
  }

  async sendToBot(platform, message, ownerId, bot, text, commandName) {
    const gateway = this.gateways[platform]
    try {
      // 话直接进 Bot 的长期会话；这一轮说完，回复由 Runtime 发回这个聊天（B5）。
      const sent = await this.bots.message(bot.id, {
        text,
        source: {
          kind: 'channel',
          platform,
          peerId: String(message.peerId),
          chatType: message.chatType === 'p2p' ? 'p2p' : 'group',
          senderId: String(message.senderId || ''),
          senderName: String(message.senderName || '').slice(0, 40),
          messageId: String(message.messageId || ''),
          // 渠道绑定的用户本人：Bot 把他的话当用户的话记，其他人的话只算外部说法。
          fromOwner: Boolean(ownerId) && String(message.senderId || '') === String(ownerId),
          ...(commandName ? { command: commandName } : {}),
        },
      })
      const attachmentNote = message.resources?.length ? '（附件暂不转给 Bot，只收到了文字。）' : ''
      if (sent.status === 'busy')
        await gateway.send(message, {
          text: `${bot.name} 正在忙别的事，等它说完再叫它，或者在 Pisper 里接着说。`,
        })
      else if (attachmentNote) await gateway.send(message, { text: attachmentNote })
    } catch (error) {
      await gateway.send(message, {
        text: `${bot.name} 没接下这句话：${error instanceof Error ? error.message : String(error)}`,
      })
    }
    return true
  }

  // 主动发到某个聊天（B5 回推）：用叫 Bot 时记下的上下文，没有时退回已保存的会话作用域。
  async sendToChat(platform, peerId, text) {
    const connection = this.state.connections[platform]
    if (!connection?.enabled || !this.gateways[platform]) return false
    const key = scopeKey(platform, peerId)
    const scope = {
      platform,
      peerId,
      ...(this.state.scopes[key] || {}),
      ...(this.botPeers.get(key) || {}),
    }
    await this.gateways[platform].sendToPeer(
      peerId,
      platform === 'feishu' ? { markdown: text } : { text },
      scope,
    )
    return true
  }

  // 创建者本人和机器人的私聊（最近说过话的那个）。只认私聊、只认他本人：旧记录没写发送者时，
  // 渠道只给创建者用才算他的。他从没在私聊里说过话的平台找不到（平台不允许机器人先开口）。
  ownerChat(platform) {
    const connection = this.state.connections[platform]
    const ownerId = ownerIdOf(platform, connection)
    if (!connection?.enabled || !this.gateways[platform] || !ownerId) return null
    const mine = (chat) =>
      chat.platform === platform &&
      (chat.chatType || 'p2p') === 'p2p' &&
      (chat.senderId ? chat.senderId === ownerId : connection.accessMode !== 'all')
    return [...Object.values(this.state.scopes), ...this.botPeers.values()]
      .filter(mine)
      .reduce(
        (latest, chat) =>
          !latest ||
          new Date(chat.updatedAt || 0).getTime() >= new Date(latest.updatedAt || 0).getTime()
            ? chat
            : latest,
        null,
      )
  }

  // Bot 的 bot_notify：发到创建者本人的私聊。只发已连接的平台，逐个报告结果，一个失败不影响别的。
  async notifyOwner(text, { platforms } = {}) {
    const results = []
    for (const platform of PLATFORMS) {
      if (platforms && !platforms.includes(platform)) continue
      if (!this.state.connections[platform]?.enabled || !this.gateways[platform]) continue
      const chat = this.ownerChat(platform)
      if (!chat) {
        results.push({ platform, status: 'no_chat' })
        continue
      }
      try {
        await this.gateways[platform].sendToPeer(
          chat.peerId,
          platform === 'feishu' ? { markdown: text } : { text },
          { ...(this.state.scopes[scopeKey(platform, chat.peerId)] || {}), ...chat },
        )
        results.push({ platform, status: 'sent' })
      } catch (error) {
        results.push({
          platform,
          status: 'failed',
          error: error instanceof Error ? error.message : String(error),
        })
      }
    }
    return results
  }

  async updateTemplate(event, platform, input) {
    if (!NOTIFICATION_EVENTS[event] || !TEMPLATE_PLATFORMS.has(platform))
      throw new Error('通知模板类型不存在。')
    const patch = {}
    if (Object.hasOwn(input || {}, 'enabled')) patch.enabled = Boolean(input.enabled)
    if (Object.hasOwn(input || {}, 'channelEnabled')) {
      if (typeof input.channelEnabled !== 'boolean') {
        throw Object.assign(new Error('通知渠道开关必须是布尔值。'), {
          code: 'INVALID_NOTIFICATION_CHANNEL_ENABLED',
        })
      }
      patch.channelEnabled = input.channelEnabled
    }
    if (Object.hasOwn(input || {}, 'content')) {
      const content = String(input.content || '').trim()
      if (!content) throw new Error('通知模板不能为空。')
      patch.content = content.slice(0, 12_000)
    }
    // 模板修改与现有持久化队列串行；只有磁盘确认后才发布到投递使用的状态。
    this.writeQueue = this.writeQueue
      .catch(() => {})
      .then(async () => {
        const template = this.state.templates[event]
        const variant = { ...template.channels[platform] }
        if (Object.hasOwn(patch, 'channelEnabled')) variant.enabled = patch.channelEnabled
        if (Object.hasOwn(patch, 'content')) variant.content = patch.content
        const next = { ...template, channels: { ...template.channels, [platform]: variant } }
        if (Object.hasOwn(patch, 'enabled')) next.enabled = patch.enabled
        const templates = { ...this.state.templates, [event]: next }
        await writeJsonAtomic(this.path, { ...this.state, templates }, { mode: 0o600 })
        this.state.templates = templates
      })
    await this.writeQueue
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
      if (!selected.has(platform) || template.channels?.[platform]?.enabled === false) continue
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
