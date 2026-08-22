// 真机验证用的 OpenAI 兼容 mock：/v1/chat/completions 返回 SSE 增量，/v1/models 返回模型列表。
// 仅监听 127.0.0.1，配合 adb reverse 供设备上的本机 Runtime 访问。
import http from 'node:http'

const server = http.createServer((request, response) => {
  const url = new URL(request.url, 'http://127.0.0.1')
  if (request.method === 'GET' && url.pathname === '/v1/models') {
    response.writeHead(200, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ data: [{ id: 'mock-model-1' }, { id: 'mock-model-2' }] }))
    return
  }
  if (request.method === 'POST' && url.pathname === '/v1/chat/completions') {
    let body = ''
    request.on('data', (chunk) => (body += chunk))
    request.on('end', () => {
      let prompt = ''
      try {
        const parsed = JSON.parse(body)
        prompt = parsed.messages?.map((message) => message.content).join(' / ') ?? ''
        console.log('mock 收到请求 model=%s messages=%d 最后消息=%s', parsed.model, parsed.messages?.length ?? 0, prompt.slice(-40))
      } catch {
        console.log('mock 收到无法解析的请求体')
      }
      response.writeHead(200, { 'content-type': 'text/event-stream' })
      const frames = ['你好，', '这是手机本机', ' Runtime 的回复。']
      let index = 0
      const timer = setInterval(() => {
        if (index < frames.length) {
          const delta = { choices: [{ delta: { content: frames[index] } }] }
          response.write(`data: ${JSON.stringify(delta)}\n\n`)
          index += 1
        } else {
          response.write('data: [DONE]\n\n')
          clearInterval(timer)
          response.end()
        }
      }, 120)
    })
    return
  }
  response.writeHead(404)
  response.end()
})

server.listen(8788, '127.0.0.1', () => console.log('mock provider: http://127.0.0.1:8788/v1'))
