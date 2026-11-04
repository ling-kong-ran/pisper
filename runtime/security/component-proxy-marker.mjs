// 组件网络代理的来源标记：代理发往私网源（含本机回环）的每个请求都带这个头，
// Pisper 自己的所有监听（应用 / 远程 / MCP）在鉴权之前见到它一律 403。
//
// 为什么需要：私网源允许 localhost，而应用监听把回环请求当成本机可信请求（开发与 npm 模式没有令牌）。
// 没有这个标记，组件声明 http://127.0.0.1:<应用端口> 就能绕过鉴权调用 Pisper 自己的 API。
// 组件改不掉它：代理从空白开始构造请求头，组件只能给白名单里的几个头。
export const COMPONENT_PROXY_HEADER = 'x-pisper-component-proxy'

/** @param {{ headers?: Record<string, unknown> }} req */
export function isComponentProxyRequest(req) {
  return req?.headers?.[COMPONENT_PROXY_HEADER] !== undefined
}
