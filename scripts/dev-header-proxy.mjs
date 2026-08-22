// 真机验证用的请求头注入代理：设备 WebView → adb reverse → 本脚本 → 目标 runtime，
// 给每个请求补 X-Pisper-Client: mobile-app，模拟移动端壳代理的形态。
// 仅用于本地开发验证，不参与构建与发布。
// 用法：node scripts/dev-header-proxy.mjs <listenPort> <targetPort>
import http from 'node:http'

const [listenPort, targetPort] = process.argv.slice(2).map(Number)
if (!listenPort || !targetPort) {
  console.error('用法: node dev-header-proxy.mjs <listenPort> <targetPort>')
  process.exit(1)
}

const server = http.createServer((request, response) => {
  const headers = {
    ...request.headers,
    'x-pisper-client': 'mobile-app',
    host: `127.0.0.1:${targetPort}`,
  }
  const upstream = http.request(
    { host: '127.0.0.1', port: targetPort, path: request.url, method: request.method, headers },
    (upstreamResponse) => {
      response.writeHead(upstreamResponse.statusCode ?? 502, upstreamResponse.headers)
      upstreamResponse.pipe(response)
    },
  )
  upstream.on('error', (error) => {
    response.writeHead(502, { 'content-type': 'text/plain' })
    response.end(String(error))
  })
  request.pipe(upstream)
})

server.listen(listenPort, '127.0.0.1', () => {
  console.log(
    `header proxy: http://127.0.0.1:${listenPort} → 127.0.0.1:${targetPort} (+X-Pisper-Client)`,
  )
})
