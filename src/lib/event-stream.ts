// SSE 读取器：与 HTTP JSON 客户端分开成独立模块，让只需要读事件流的领域 API（例如看板事件流）
// 不必依赖 @/lib/api 的其余导出；@/lib/api 继续原样再导出 consumeEventStream，现有调用方不变。
import { readHttpError } from '@/lib/http-response'

// 消费 SSE 事件流：逐字节读取响应体，按行解析 event:/data:/id: 字段，
// 每遇到空行（记录分隔）或 EOF 时回调 onEvent(event, data, meta)。
// meta.id 是服务端为 run 内每帧编号的游标，供断流后重挂续传。
// 回调返回 false 可提前停止读取；解析容错：即使中间的 WebView/代理
// 丢失了空行分隔符，也能在下一个 event: 出现时冲刷上一笔待发记录。
export async function consumeEventStream<T = Record<string, unknown>>(
  response: Response,
  onEvent: (event: string, data: T, meta: { id: string | null }) => boolean | void,
): Promise<void> {
  if (!response.ok) throw await readHttpError(response)
  if (!response.body) throw new Error('响应不包含可读取的数据流')
  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  let event = 'message'
  let id: string | null = null
  let dataLines: string[] = []
  let stopped = false

  const dispatch = () => {
    if (!dataLines.length) {
      event = 'message'
      return true
    }
    const data = JSON.parse(dataLines.join('\n')) as T
    const keepReading = onEvent(event || 'message', data, { id }) !== false
    event = 'message'
    id = null
    dataLines = []
    return keepReading
  }
  const processLine = (line: string) => {
    if (!line) return dispatch()
    if (line.startsWith(':')) return true
    const separator = line.indexOf(':')
    const field = separator < 0 ? line : line.slice(0, separator)
    let value = separator < 0 ? '' : line.slice(separator + 1)
    if (value.startsWith(' ')) value = value.slice(1)
    if (field === 'event') {
      // Each Pisper record starts with `event:`. Flush a pending record here as a
      // recovery path when a WebView or intermediary drops the blank SSE separator.
      if (dataLines.length && !dispatch()) return false
      event = value
    } else if (field === 'data') dataLines.push(value)
    else if (field === 'id') id = value || null
    return true
  }
  const drain = (final = false) => {
    let offset = 0
    for (let index = 0; index < buffer.length; index += 1) {
      const code = buffer.charCodeAt(index)
      if (code !== 10 && code !== 13) continue
      if (code === 13 && index + 1 >= buffer.length && !final) break
      const line = buffer.slice(offset, index)
      if (code === 13 && buffer.charCodeAt(index + 1) === 10) index += 1
      offset = index + 1
      if (!processLine(line)) {
        buffer = buffer.slice(offset)
        return false
      }
    }
    buffer = buffer.slice(offset)
    if (final && buffer) {
      const line = buffer
      buffer = ''
      if (!processLine(line)) return false
    }
    return final ? dispatch() : true
  }

  try {
    while (!stopped) {
      const { value, done } = await reader.read()
      buffer += decoder.decode(value || new Uint8Array(), { stream: !done })
      if (!drain(done)) {
        stopped = true
        break
      }
      if (done) break
    }
  } catch (error) {
    stopped = true
    throw error
  } finally {
    if (stopped) await reader.cancel().catch(() => {})
    reader.releaseLock()
  }
}
