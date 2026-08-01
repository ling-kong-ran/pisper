import type { AxiosRequestConfig } from 'axios'
import { ApiError, requestJson } from './http.ts'

export type ApiJsonOptions = Omit<AxiosRequestConfig, 'data'> & { body?: unknown }

export async function apiJson<T = unknown>(path: string, options: ApiJsonOptions = {}): Promise<T> {
  const { body, headers, ...rest } = options
  return requestJson(path, {
    ...rest,
    headers,
    data: body,
  })
}

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

export function applyTextPatch(value: unknown, patch: TextPatch): string {
  const text = String(value || '')
  if (!patch) return text
  const start = Math.max(0, Math.min(text.length, Number(patch?.start) || 0))
  return `${text.slice(0, start)}${String(patch?.text || '')}`
}
