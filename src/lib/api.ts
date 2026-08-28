import { ApiError, requestJson, type HttpRequestOptions } from './http.ts'

export type ApiJsonOptions = Omit<HttpRequestOptions, 'data'>

// 通用 JSON 请求入口：path 相对运行时 API 根路径，返回解析后的 JSON。
// 类型参数 T 由调用方声明，不在此处做强校验（负载错误统一抛 ApiError）。
export async function apiJson<T = unknown>(path: string, options: ApiJsonOptions = {}): Promise<T> {
  return requestJson(path, options)
}

// 消费 SSE 事件流：逐字节读取响应体，按行解析 event:/data:/id: 字段，
// 每遇到空行（记录分隔）或 EOF 时回调 onEvent(event, data, meta)。
// meta.id 是服务端为 run 内每帧编号的游标，供断流后重挂续传。
// 回调返回 false 可提前停止读取；解析容错：即使中间的 WebView/代理
// 丢失了空行分隔符，也能在下一个 event: 出现时冲刷上一笔待发记录。
export async function consumeEventStream<T = Record<string, unknown>>(
  response: Response,
  onEvent: (event: string, data: T, meta: { id: string | null }) => boolean | void,
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

export type TextPatch = { start?: number; text?: string } | null | undefined

type StreamResumeOptions<T> = {
  // 建立首连（POST /api/chat 之类）。
  open: () => Promise<Response>
  // 凭 runId + 游标重挂：GET /api/runs/:id/events?after=<cursor>。
  resume: (runId: string, cursor: number) => Promise<Response>
  onEvent: (event: string, data: T) => boolean | void
  maxAttempts?: number
  baseDelayMs?: number
}

const STREAM_RESUME_MAX_ATTEMPTS = 5
const STREAM_RESUME_BASE_DELAY_MS = 400

// 带游标重挂的 SSE 消费：run 首帧提供 runId，后续帧携带递增游标（SSE id 行）。
// 传输层在终态前断开（连接重置/被判定慢客户端/代理提前收尾）时，凭游标重挂，
// 服务端从环形缓冲补发缺失帧；缓冲溢出会先发 resync_required，交由调用方快照对齐。
// 两种终态不重挂：onEvent 返回 false（done）、onEvent 抛错（业务 error 帧）。
// 重试只在外部传输错误上发生，且每次重挂有进展（游标前移）就重置计数——
// 只有连续无进展的失败才会耗尽尝试次数。
export async function streamEventsWithResume<T = Record<string, unknown>>({
  open,
  resume,
  onEvent,
  maxAttempts = STREAM_RESUME_MAX_ATTEMPTS,
  baseDelayMs = STREAM_RESUME_BASE_DELAY_MS,
}: StreamResumeOptions<T>): Promise<void> {
  let runId = ''
  let cursor = 0
  let terminalSeen = false
  let handlerError: unknown = null

  const handleResponse = async (response: Response) => {
    await consumeEventStream<T>(response, (event, data, meta) => {
      if (meta.id != null) {
        const parsed = Number(meta.id)
        if (Number.isFinite(parsed) && parsed > cursor) cursor = parsed
      }
      const record = data as Record<string, unknown>
      if (event === 'run' && typeof record?.runId === 'string') runId = record.runId
      try {
        const result = onEvent(event, data)
        if (result === false) terminalSeen = true
        return result
      } catch (error) {
        // 标记业务错误（如服务端 error 帧）：与传输层断开区分，不触发重挂。
        handlerError = error
        throw error
      }
    })
  }

  let transportError: unknown = null
  try {
    await handleResponse(await open())
  } catch (error) {
    if (handlerError) throw handlerError
    transportError = error
  }
  if (handlerError) throw handlerError
  if (terminalSeen) return
  // 未收到 run 头帧说明 run 未建立（如参数被 409 拒绝），没有可重挂的对象。
  if (!runId) throw transportError ?? new Error('流在建立运行前中断')

  let attempts = 0
  for (;;) {
    attempts += 1
    if (attempts > maxAttempts) throw transportError ?? new Error('流式连接中断且重挂失败')
    await new Promise((resolve) => setTimeout(resolve, baseDelayMs * 2 ** (attempts - 1)))
    const cursorBefore = cursor
    try {
      await handleResponse(await resume(runId, cursor))
      if (handlerError) throw handlerError
      // 终态帧到达（done）或调用方在 resync_required 后主动停读，都算正常收尾；
      // 干净 EOF 但无终态说明 run 已关闭且终态帧被挤出缓冲，交给调用方整体校准。
      return
    } catch (error) {
      if (handlerError) throw handlerError
      // run 缓冲已清理（409）：续传无意义，直接抛出由调用方走快照校准。
      if (error instanceof ApiError && error.status === 409) throw error
      transportError = error
      // 本次重挂有进展（游标前移）说明链路可用，重置计数再续。
      if (cursor > cursorBefore) attempts = 0
    }
  }
}

// 流式文本补丁：在指定位置插入/替换文本。
// start 表示替换起点（越界时钳制到文本两端），patch 为 null 时原样返回——
// 调用方用 null 表达“无需修改”，避免把空补丁误当作清空操作。
export function applyTextPatch(value: unknown, patch: TextPatch): string {
  const text = String(value || '')
  if (!patch) return text
  const start = Math.max(0, Math.min(text.length, Number(patch?.start) || 0))
  return `${text.slice(0, start)}${String(patch?.text || '')}`
}
