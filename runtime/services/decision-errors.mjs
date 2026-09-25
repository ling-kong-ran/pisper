// 决策边界使用稳定错误码；不转发上游错误正文或底层异常中的凭据。
export class DecisionError extends Error {
  /**
   * @param {string} code 稳定机器可读错误码
   * @param {string} message
   * @param {{ statusCode?: number, retryable?: boolean }} [options]
   */
  constructor(code, message, { statusCode = 400, retryable = false } = {}) {
    super(message)
    this.name = 'DecisionError'
    this.code = code
    this.statusCode = statusCode
    this.retryable = retryable
  }
}

/** @type {Record<string, string>} */
const FAILURE_MESSAGES = {
  unsupported_provider: '不支持所选决策服务商。',
  unsupported_capability: '所选决策模型不支持此判断能力。',
  config_missing: '尚未配置决策服务 密钥或地址。',
  auth: '决策服务 认证失败，请检查 API 密钥与账户权限。',
  rate_limited: '决策服务 请求过于频繁，请稍后重试。',
  overloaded: '决策服务 暂时过载，请稍后重试。',
  state_too_large: '输入超出所选决策模型的上下文限制。',
  invalid: '决策服务 拒绝了请求，请检查问题定义。',
  network: '无法连接 决策服务。',
  timeout: '决策服务 请求超时。',
  aborted: '请求已取消。',
  bad_response: '决策服务 返回了无法解析的响应。',
}

/**
 * @param {string} code
 * @param {{ statusCode?: number, retryable?: boolean }} [extra]
 */
export function remoteFailure(code, extra = {}) {
  return new DecisionError(code, FAILURE_MESSAGES[code] ?? FAILURE_MESSAGES.network, extra)
}
