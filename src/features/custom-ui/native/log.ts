// 组件日志（规格 5.6、计划 §4.2）：ctx.log 与宿主捕获的错误写进本页的环形缓冲（工作室日志页读），
// warn / error 逐条 POST 到 /api/custom-ui/components/:id/errors（Runtime 每组件每分钟收 20 条）。
// 客户端先按同样的速率丢弃并计数，避免一个狂刷日志的组件把请求打满、换来一串 429。
import type { ComponentReport, ComponentReportKind } from '@/features/custom-ui/custom-ui-api'
import type { ComponentSurface } from '@shared/custom-ui-manifest.mjs'

export const LOG_BUFFER_LIMIT = 200
export const REPORTS_PER_MINUTE = 20
const MESSAGE_MAX = 500
const STACK_MAX = 4000
const REPORT_INSTANCE_ID = /^[a-zA-Z0-9._:-]{1,64}$/

export type ComponentLogLevel = 'info' | 'warn' | 'error'
export type ComponentLogEntry = {
  at: number
  componentId: string
  instanceId?: string
  surface?: ComponentSurface
  kind: ComponentReportKind
  level: ComponentLogLevel
  message: string
  stack?: string
}

/** 把任意日志参数格式化成一行文字（≤500）；对象用 JSON，循环引用等失败时退回 String()。 */
export function formatLogArguments(args: readonly unknown[]): string {
  const parts = args.map((value) => {
    if (typeof value === 'string') return value
    if (value instanceof Error) return `${value.name}: ${value.message}`
    try {
      const json = JSON.stringify(value)
      return json === undefined ? String(value) : json
    } catch {
      return String(value)
    }
  })
  return parts.join(' ').slice(0, MESSAGE_MAX)
}

export function errorDetails(error: unknown): { message: string; stack?: string } {
  if (error instanceof Error) {
    const message = `${error.name}: ${error.message}`.slice(0, MESSAGE_MAX)
    return typeof error.stack === 'string'
      ? { message, stack: error.stack.slice(0, STACK_MAX) }
      : { message }
  }
  return { message: formatLogArguments([error]) }
}

// [让代理修复] 带上的错误摘要（计划 §5.4 第 4 步）：消息一行 + 调用栈前 1 KB；
// 调用栈第一行通常就是消息本身，不重复。
const FIX_STACK_MAX = 1024

export function componentFixSummary(message: string, stack?: string): string {
  const head = message.trim()
  const trace = (stack ?? '').trim()
  const body = trace.startsWith(head) ? trace.slice(head.length).trim() : trace
  return body ? `${head}\n${body.slice(0, FIX_STACK_MAX)}` : head
}

export type ComponentLogBuffer = {
  append(entry: ComponentLogEntry): void
  read(componentId: string): ComponentLogEntry[]
  clear(componentId: string): void
  subscribe(listener: (componentId: string) => void): () => void
}

export function createLogBuffer(limit = LOG_BUFFER_LIMIT): ComponentLogBuffer {
  const logs = new Map<string, ComponentLogEntry[]>()
  const listeners = new Set<(componentId: string) => void>()
  return {
    append(entry) {
      const list = logs.get(entry.componentId) ?? []
      list.push(entry)
      if (list.length > limit) list.splice(0, list.length - limit)
      logs.set(entry.componentId, list)
      for (const listener of [...listeners]) listener(entry.componentId)
    },
    read: (componentId) => [...(logs.get(componentId) ?? [])],
    clear(componentId) {
      logs.delete(componentId)
      for (const listener of [...listeners]) listener(componentId)
    },
    subscribe(listener) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
  }
}

export type ErrorReporter = {
  report(componentId: string, report: ComponentReport): boolean
  dropped(componentId: string): number
}

/** 按组件限速的上报器；send 失败静默（上报本身不能再产生需要上报的错误）。 */
export function createErrorReporter({
  send,
  now = () => Date.now(),
  perMinute = REPORTS_PER_MINUTE,
}: {
  send: (componentId: string, report: ComponentReport) => Promise<unknown>
  now?: () => number
  perMinute?: number
}): ErrorReporter {
  const windows = new Map<string, { at: number; count: number; dropped: number }>()
  return {
    report(componentId, report) {
      const at = now()
      let window = windows.get(componentId)
      if (!window || at - window.at >= 60_000) {
        window = { at, count: 0, dropped: window?.dropped ?? 0 }
        windows.set(componentId, window)
      }
      if (window.count >= perMinute) {
        window.dropped += 1
        return false
      }
      window.count += 1
      // Runtime 只收 ≤64 位的实例 id（custom-ui-errors.mjs）；更长的不带，免得整条被拒。
      const { instanceId, ...rest } = report
      const payload: ComponentReport = {
        ...rest,
        ...(instanceId && REPORT_INSTANCE_ID.test(instanceId) ? { instanceId } : {}),
        message: report.message.slice(0, MESSAGE_MAX) || report.kind,
        ...(report.stack ? { stack: report.stack.slice(0, STACK_MAX) } : {}),
      }
      void send(componentId, payload).catch(() => undefined)
      return true
    },
    dropped: (componentId) => windows.get(componentId)?.dropped ?? 0,
  }
}

export type ComponentLogger = {
  info(...args: unknown[]): void
  warn(...args: unknown[]): void
  error(...args: unknown[]): void
  // 宿主自己记的条目（加载失败、渲染错误、回调异常、慢提交、能力错误）。
  record(kind: ComponentReportKind, level: ComponentLogLevel, error: unknown): void
}

export function createComponentLogger({
  componentId,
  instanceId,
  surface,
  buffer,
  reporter,
  now = () => Date.now(),
}: {
  componentId: string
  instanceId?: string
  surface?: ComponentSurface
  buffer: ComponentLogBuffer
  reporter: ErrorReporter
  now?: () => number
}): ComponentLogger {
  const write = (
    kind: ComponentReportKind,
    level: ComponentLogLevel,
    message: string,
    stack?: string,
  ) => {
    buffer.append({
      at: now(),
      componentId,
      ...(instanceId ? { instanceId } : {}),
      ...(surface ? { surface } : {}),
      kind,
      level,
      message,
      ...(stack ? { stack } : {}),
    })
    if (level === 'info') return
    reporter.report(componentId, {
      ...(instanceId ? { instanceId } : {}),
      ...(surface ? { surface } : {}),
      kind,
      level,
      message,
      ...(stack ? { stack } : {}),
    })
  }
  return {
    info: (...args) => write('log', 'info', formatLogArguments(args)),
    warn: (...args) => write('log', 'warn', formatLogArguments(args)),
    error: (...args) => write('log', 'error', formatLogArguments(args)),
    record(kind, level, error) {
      const details = errorDetails(error)
      write(kind, level, details.message, details.stack)
    },
  }
}
