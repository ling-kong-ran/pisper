// 决策服务路由：/api/decisions/*
// 职责仅限请求解析、字段白名单校验、调用 DecisionService 与响应序列化。

function requireDecisions(services) {
  if (!services.decisions) {
    throw Object.assign(new Error('Decision service is unavailable.'), { statusCode: 503 })
  }
  return services.decisions
}

// 决策路由在响应体里附带稳定机器可读错误码（{ error, code }），
// 属于新增接口的初始契约；旧接口的错误形状保持不变。
function withErrorCode(handler) {
  return async (context) => {
    try {
      await handler(context)
    } catch (error) {
      if (context.res.destroyed) return
      if (error && typeof error.code === 'string') {
        const status = Number.isInteger(error.statusCode) ? error.statusCode : 400
        context.json(status, { error: context.publicError(error), code: error.code })
        return
      }
      throw error
    }
  }
}

// 测试与显式决策属于当前 HTTP 请求；客户端断开后不得留下后台推理和重试。
function withDecisionRequest(handler) {
  return withErrorCode(async (context) => {
    const { req, res } = context
    const controller = new AbortController()
    const abort = () => controller.abort()
    const close = () => {
      if (!res.writableEnded) abort()
    }
    req.once?.('aborted', abort)
    res.once?.('close', close)
    if (req.aborted || res.destroyed) abort()
    try {
      if (controller.signal.aborted) return
      await handler({ ...context, signal: controller.signal })
    } finally {
      req.removeListener?.('aborted', abort)
      res.removeListener?.('close', close)
    }
  })
}

function fields(value, allowed) {
  if (
    !value ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    Object.keys(value).some((key) => !allowed.includes(key))
  ) {
    throw new Error('Invalid decisions request.')
  }
  return value
}

export const decisionRoutes = [
  {
    method: 'GET',
    path: '/api/decisions/status',
    handler: withErrorCode(function handler({ services, json }) {
      const decisions = requireDecisions(services)
      json(200, { config: decisions.publicConfig() })
    }),
  },
  {
    method: 'PUT',
    path: '/api/decisions/config',
    handler: withErrorCode(async function handler({ services, body, json }) {
      const decisions = requireDecisions(services)
      const patch = fields(await body(), ['remote', 'delegate'])
      if (patch.remote !== undefined) {
        fields(patch.remote, ['provider', 'baseUrl', 'modelId', 'apiKey'])
      }
      if (patch.delegate !== undefined) {
        fields(patch.delegate, ['enabled', 'allowThreshold', 'verifyActions'])
      }
      json(200, { config: await decisions.updateConfig(patch) })
    }),
  },
  {
    method: 'POST',
    path: '/api/decisions/test',
    handler: withDecisionRequest(async function handler({ services, json, signal }) {
      const decisions = requireDecisions(services)
      const result = await decisions.testConnection({ signal })
      if (!signal.aborted) json(200, result)
    }),
  },
  {
    method: 'POST',
    path: '/api/decisions/decide',
    handler: withDecisionRequest(async function handler({ services, body, json, signal }) {
      const decisions = requireDecisions(services)
      const input = fields(await body(), ['state', 'questions'])
      if (signal.aborted) return
      const result = await decisions.decide(input, { signal })
      if (!signal.aborted) json(200, result)
    }),
  },
]
