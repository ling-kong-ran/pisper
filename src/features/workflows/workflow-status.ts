// @public “更多”启动台的工作流磁贴角标：只读正在运行的工作流数量，不加载工作流页。
// 响应按不可信数据校验；字段缺失或非法时抛出协议错误，调用方据此不显示角标（诚实降级）。
import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'

export function parseRunningWorkflowCount(value: unknown): number {
  const limits = value && typeof value === 'object' ? (value as { limits?: unknown }).limits : null
  const running =
    limits && typeof limits === 'object' ? (limits as { running?: unknown }).running : undefined
  if (typeof running !== 'number' || !Number.isFinite(running) || running < 0)
    throw invalidResponseError()
  return Math.floor(running)
}

export function fetchRunningWorkflowCount(signal?: AbortSignal) {
  return requestJson('/api/workflows', { signal, parse: parseRunningWorkflowCount })
}
