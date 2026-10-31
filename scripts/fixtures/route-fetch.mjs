// 冒烟共用：Playwright route.fetch 遇到连接级错误时重取（规格 A9 遗留：“route.fetch 连接错误重试做成共用助手”）。
//
// Playwright 的 route.fetch 走它自己的长连接池，池里的空闲连接没有超时（Node 只用服务器的 Keep-Alive 提示去缩短
// 已有的超时，Playwright 的连接池没设超时，提示不起作用）；运行时服务器空闲 6 秒（5 秒加 1 秒缓冲）就关掉长连接。
// 两边同在一个进程里，负载下事件循环变慢时，route.fetch 可能恰好把请求写进服务器同一轮要关的旧连接，服务器先关
// 后读，得到 read ECONNRESET（调用日志只有“→ GET”、没有响应）。
// 只对幂等的 GET / HEAD 重取：连接级错误时坏连接已被销毁，换新连接再取，最多三次；其余方法照常只取一次。
// 每次重取都交给 onRetry（调用方记进报告），不静默吞掉。

const CONNECTION_ERROR = /ECONNRESET|socket hang up|EPIPE/

/**
 * @param {import('playwright-core').Route} route
 * @param {{ onRetry?: (detail: string) => void }} [options]
 */
export async function fetchIdempotent(route, { onRetry } = {}) {
  const method = route.request().method()
  const retryable = method === 'GET' || method === 'HEAD'
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await route.fetch()
    } catch (error) {
      const message = String(error?.message || error)
      if (!retryable || attempt >= 3 || !CONNECTION_ERROR.test(message)) throw error
      onRetry?.(`${new URL(route.request().url()).pathname}: ${message.split('\n')[0]}`)
    }
  }
}
