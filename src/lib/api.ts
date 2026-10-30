import { abortReason, throwIfAborted } from '@/lib/abort-signal'
import { consumeEventStream } from './event-stream.ts'
import { ApiError, requestJson, type HttpRequestOptions } from './http.ts'

export type ApiJsonOptions = Omit<HttpRequestOptions, 'data'>

// 通用 JSON 请求入口：path 相对运行时 API 根路径，返回解析后的 JSON。
// 类型参数 T 由调用方声明，不在此处做强校验（负载错误统一抛 ApiError）。
export async function apiJson<T = unknown>(path: string, options: ApiJsonOptions = {}): Promise<T> {
  return requestJson(path, options)
}

// SSE 读取器的实现在 @/lib/event-stream，这里再导出保持原有入口。
export { consumeEventStream } from './event-stream.ts'

export type TextPatch = { start?: number; text?: string } | null | undefined

type StreamResumeOptions<T> = {
  // 建立首连（POST /api/chat 之类）。
  open: () => Promise<Response>
  // 凭 runId + 游标重挂：GET /api/runs/:id/events?after=<cursor>。
  resume: (runId: string, cursor: number) => Promise<Response>
  onEvent: (event: string, data: T) => boolean | void
  maxAttempts?: number
  baseDelayMs?: number
  // 所有者停止消费时中断：不再重挂、不再等待退避；open/resume 自己负责把它交给 fetch。
  // Runtime 侧的 run 与连接解耦，中断消费不会停止运行。
  signal?: AbortSignal
}

const STREAM_RESUME_MAX_ATTEMPTS = 5
const STREAM_RESUME_BASE_DELAY_MS = 400

// 带游标重挂的 SSE 消费：run 首帧提供 runId，后续帧携带递增游标（SSE id 行）。
// 传输层在终态前断开（连接重置/被判定慢客户端/代理提前收尾）时，凭游标重挂，
// 服务端从环形缓冲补发缺失帧；缓冲溢出会先发 resync_required，交由调用方快照对齐。
// 两种终态不重挂：明确的 done/error 帧，或 onEvent 返回 false（如 resync_required）。
// onEvent 抛错（业务 error 帧）也不重挂；重试只在外部传输错误上发生，且每次
// 重挂有进展（游标前移）就重置计数——只有连续无进展的失败才会耗尽尝试次数；
// 干净 EOF 没有终态帧也按传输中断处理。
const STREAM_ENDED_BEFORE_TERMINAL_ERROR = 'SSE 流在终态事件前结束'

// 重挂退避同样响应中断，并清掉定时器，避免所有者释放后仍留下待触发的等待。
function resumeBackoff(delayMs: number, signal?: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    if (signal?.aborted) {
      reject(abortReason(signal))
      return
    }
    const aborted = () => {
      clearTimeout(timer)
      if (signal) reject(abortReason(signal))
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', aborted)
      resolve()
    }, delayMs)
    signal?.addEventListener('abort', aborted, { once: true })
  })
}

export async function streamEventsWithResume<T = Record<string, unknown>>({
  open,
  resume,
  onEvent,
  maxAttempts = STREAM_RESUME_MAX_ATTEMPTS,
  baseDelayMs = STREAM_RESUME_BASE_DELAY_MS,
  signal,
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
        if (event === 'done' || event === 'error' || result === false) terminalSeen = true
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
    throwIfAborted(signal)
    attempts += 1
    if (attempts > maxAttempts) throw transportError ?? new Error('流式连接中断且重挂失败')
    await resumeBackoff(baseDelayMs * 2 ** (attempts - 1), signal)
    const cursorBefore = cursor
    try {
      await handleResponse(await resume(runId, cursor))
      if (handlerError) throw handlerError
      // 只有明确收到终态帧才能结束；干净 EOF 仍可能是代理提前收尾，
      // 必须沿当前游标再次重挂，避免把未完成的运行误报成成功。
      if (!terminalSeen) throw new Error(STREAM_ENDED_BEFORE_TERMINAL_ERROR)
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
