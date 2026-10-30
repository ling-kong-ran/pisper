// @public “更多”启动台的定时任务磁贴角标：只读已启用的任务数，不加载定时任务页。
// 响应按不可信数据校验：tasks 不是数组时抛出协议错误，单条非法任务不计数。
import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'

export function parseEnabledScheduleCount(value: unknown): number {
  const tasks = value && typeof value === 'object' ? (value as { tasks?: unknown }).tasks : null
  if (!Array.isArray(tasks)) throw invalidResponseError()
  return tasks.filter(
    (task) => task && typeof task === 'object' && (task as { enabled?: unknown }).enabled === true,
  ).length
}

export function fetchEnabledScheduleCount(signal?: AbortSignal) {
  return requestJson('/api/schedules', { signal, parse: parseEnabledScheduleCount })
}
