// Bot 路由（B1）：Bot 注册表的增删改查，以及打开 Bot 的长期会话。只做门控、调用服务与序列化；
// 校验与唯一性在 BotService。Bot 就是一个会话加上设定：在应用里和它说话走普通会话的接口。
// 两道门：Runtime 能力 bots（经 CAPABILITY_ROUTES）与实验功能开关 bots（用户在设置 › 实验功能里打开）。
// 错误统一返回 { error, code }，code 供客户端区分名字重复、群聊缺白名单等情况。

const BOTS_FEATURE_ID = 'bots'

function respondError(json, error) {
  const status = Number(error?.statusCode)
  const code =
    typeof error?.code === 'string' && /^[a-z_]{1,64}$/.test(error.code) ? error.code : ''
  if (!code) throw error
  json(Number.isInteger(status) && status >= 400 && status < 500 ? status : 400, {
    error: error.message,
    code,
  })
}

// 包一层：实验开关没开时一律 409，其余异常按 code 映射；没有 code 的交给通用错误处理。
export function botRoute(handler) {
  return async (context) => {
    if (!context.runtime.isBetaFeatureEnabled(BOTS_FEATURE_ID)) {
      context.json(409, {
        error: 'Bots 是实验功能，请先在设置 › 实验功能里打开。',
        code: 'beta_feature_disabled',
      })
      return
    }
    try {
      await handler(context)
    } catch (error) {
      respondError(context.json, error)
    }
  }
}

export const botRoutes = [
  {
    method: 'GET',
    path: '/api/bots',
    handler: botRoute(({ runtime, json }) => {
      json(200, { bots: runtime.listBots() })
    }),
  },
  {
    method: 'POST',
    path: '/api/bots',
    handler: botRoute(async ({ runtime, body, json }) => {
      json(201, { bot: await runtime.createBot(await body()) })
    }),
  },
  {
    method: 'GET',
    path: '/api/bots/:botId',
    handler: botRoute(({ runtime, params, json }) => {
      const bot = runtime.getBot(params.botId)
      if (!bot) json(404, { error: 'Bot 不存在。', code: 'bot_not_found' })
      else json(200, { bot })
    }),
  },
  {
    method: 'PATCH',
    path: '/api/bots/:botId',
    handler: botRoute(async ({ runtime, params, body, json }) => {
      const bot = await runtime.updateBot(params.botId, await body())
      if (!bot) json(404, { error: 'Bot 不存在。', code: 'bot_not_found' })
      else json(200, { bot })
    }),
  },
  // 打开 Bot：返回它的长期会话 id（还没有或会话文件已不在时新建），界面随后打开这个会话。
  {
    method: 'POST',
    path: '/api/bots/:botId/session',
    handler: botRoute(async ({ runtime, params, json }) => {
      json(200, { sessionId: await runtime.ensureBotSession(params.botId) })
    }),
  },
  {
    method: 'DELETE',
    path: '/api/bots/:botId',
    handler: botRoute(async ({ runtime, params, json }) => {
      const deleted = await runtime.deleteBot(params.botId)
      if (!deleted) json(404, { error: 'Bot 不存在。', code: 'bot_not_found' })
      else json(200, { deleted: true })
    }),
  },
  // Bot 自己的记忆：设置页里逐条看、加、改、删（对话里的记住 / 忘掉走决定，不经这里）。
  {
    method: 'GET',
    path: '/api/bots/:botId/memory',
    handler: botRoute(async ({ runtime, params, json }) => {
      json(200, { entries: await runtime.listBotMemory(params.botId) })
    }),
  },
  {
    method: 'POST',
    path: '/api/bots/:botId/memory',
    handler: botRoute(async ({ runtime, params, body, json }) => {
      const input = await body()
      json(201, { entry: await runtime.addBotMemory(params.botId, input?.text) })
    }),
  },
  {
    method: 'PATCH',
    path: '/api/bots/:botId/memory/:entryId',
    handler: botRoute(async ({ runtime, params, body, json }) => {
      const input = await body()
      json(200, { entry: await runtime.editBotMemory(params.botId, params.entryId, input?.text) })
    }),
  },
  {
    method: 'DELETE',
    path: '/api/bots/:botId/memory/:entryId',
    handler: botRoute(async ({ runtime, params, json }) => {
      const forgotten = await runtime.forgetBotMemory(params.botId, params.entryId)
      if (!forgotten) json(404, { error: '这条记忆不存在。', code: 'bot_memory_not_found' })
      else json(200, { deleted: true })
    }),
  },
]
