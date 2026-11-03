// Bot 路由（B1）：Bot 注册表的增删改查。只做门控、调用服务与序列化；校验与唯一性在 BotService。
// 两道门：Runtime 能力 bots（经 CAPABILITY_ROUTES）与实验功能开关 bots（用户在设置 › 实验功能里打开）。
// 错误统一返回 { error, code }，code 供客户端区分名字重复、群聊缺白名单等情况。

const BOTS_FEATURE_ID = 'bots'
const TASK_STATUSES = new Set([
  'queued',
  'preparing',
  'running',
  'review',
  'done',
  'failed',
  'interrupted',
  'discarded',
])

function record(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value : {}
}

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
function botRoute(handler) {
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
  {
    method: 'POST',
    path: '/api/bots/:botId/tasks',
    handler: botRoute(async ({ runtime, params, body, json }) => {
      json(201, { task: await runtime.dispatchBotTask(params.botId, record(await body())) })
    }),
  },
  {
    method: 'POST',
    path: '/api/bots/:botId/messages',
    handler: botRoute(async ({ runtime, params, body, json }) => {
      json(201, { task: await runtime.sendBotMessage(params.botId, record(await body())) })
    }),
  },
  {
    method: 'GET',
    path: '/api/bot-tasks',
    handler: botRoute(({ runtime, url, json }) => {
      const botId = String(url.searchParams.get('botId') || '').slice(0, 200)
      const status = String(url.searchParams.get('status') || '')
      if (status && !TASK_STATUSES.has(status)) {
        json(400, { error: '未知的任务状态。', code: 'bot_task_status_invalid' })
        return
      }
      const limit = Number(url.searchParams.get('limit') || 100)
      json(200, {
        tasks: runtime.listBotTasks({
          botId,
          status,
          limit: Number.isInteger(limit) && limit > 0 ? Math.min(limit, 500) : 100,
        }),
      })
    }),
  },
  {
    method: 'GET',
    path: '/api/bot-tasks/:taskId',
    handler: botRoute(({ runtime, params, json }) => {
      const task = runtime.getBotTask(params.taskId)
      if (!task) json(404, { error: '任务不存在。', code: 'bot_task_not_found' })
      else json(200, { task })
    }),
  },
  {
    method: 'GET',
    path: '/api/bot-tasks/:taskId/diff',
    handler: botRoute(async ({ runtime, params, json }) => {
      json(200, await runtime.getBotTaskDiff(params.taskId))
    }),
  },
  ...['accept', 'discard', 'resume', 'stop'].map((action) => ({
    method: 'POST',
    path: `/api/bot-tasks/:taskId/${action}`,
    handler: botRoute(async ({ runtime, params, body, json }) => {
      const input = action === 'accept' ? record(await body()) : {}
      const task =
        action === 'accept'
          ? await runtime.acceptBotTask(params.taskId, input)
          : action === 'discard'
            ? await runtime.discardBotTask(params.taskId)
            : action === 'resume'
              ? await runtime.resumeBotTask(params.taskId)
              : await runtime.stopBotTask(params.taskId)
      json(200, { task })
    }),
  })),
  {
    method: 'DELETE',
    path: '/api/bots/:botId',
    handler: botRoute(async ({ runtime, params, json }) => {
      const deleted = await runtime.deleteBot(params.botId)
      if (!deleted) json(404, { error: 'Bot 不存在。', code: 'bot_not_found' })
      else json(200, { deleted: true })
    }),
  },
]
