import { ApiError, requestJson, type HttpRequestOptions } from './http.ts'

export type ApiJsonOptions = Omit<HttpRequestOptions, 'data'>

// 通用 JSON 请求入口：path 相对运行时 API 根路径，返回解析后的 JSON。
// 类型参数 T 由调用方声明，不在此处做强校验（负载错误统一抛 ApiError）。
export async function apiJson<T = unknown>(path: string, options: ApiJsonOptions = {}): Promise<T> {
  return requestJson(path, options)
}

// 消费 SSE 事件流：逐字节读取响应体，按行解析 event:/data: 字段，
// 每遇到空行（记录分隔）或 EOF 时回调 onEvent(event, data)。
// 回调返回 false 可提前停止读取；解析容错：即使中间的 WebView/代理
// 丢失了空行分隔符，也能在下一个 event: 出现时冲刷上一笔待发记录。
export async function consumeEventStream<T = Record<string, unknown>>(
  response: Response,
  onEvent: (event: string, data: T) => boolean | void,
): Promise<void> {
  if (!response.ok) {
    const data = (await response.json().catch(() => ({}))) as { error?: string }
    throw new ApiError(data.error || `请求失败 (${response.status})`, {
      status: response.status,
      data,
    })
  }
  if (!response.body) throw new Error('响应不包含可读取的数据流')
  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  let event = 'message'
  let dataLines: string[] = []
  let stopped = false

  const dispatch = () => {
    if (!dataLines.length) {
      event = 'message'
      return true
    }
    const data = JSON.parse(dataLines.join('\n')) as T
    const keepReading = onEvent(event || 'message', data) !== false
    event = 'message'
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

export type TextPatch = { start?: number; text?: string } | null | undefined

// 流式文本补丁：在指定位置插入/替换文本。
// start 表示替换起点（越界时钳制到文本两端），patch 为 null 时原样返回——
// 调用方用 null 表达“无需修改”，避免把空补丁误当作清空操作。
export function applyTextPatch(value: unknown, patch: TextPatch): string {
  const text = String(value || '')
  if (!patch) return text
  const start = Math.max(0, Math.min(text.length, Number(patch?.start) || 0))
  return `${text.slice(0, start)}${String(patch?.text || '')}`
}
