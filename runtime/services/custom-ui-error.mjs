// 原生组件接口的结构化错误：路由包装器把它写成 { code, error: code, ...details }，
// 前端按 custom-ui:errors.<code> 本地化；其他异常仍走 api-handler 的通用 400 / 5xx。
export class CustomUiError extends Error {
  /**
   * @param {string} code
   * @param {number} [statusCode]
   * @param {Record<string, unknown>} [details]
   */
  constructor(code, statusCode = 400, details = {}) {
    super(code)
    this.name = 'CustomUiError'
    this.code = code
    this.statusCode = statusCode
    this.details = details
  }
}
