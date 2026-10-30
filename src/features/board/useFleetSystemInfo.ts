// 舰队计量浮层底部的系统信息行：子代理并发、今日 token 与 Runtime 内存，只读现有接口，浮层打开时才请求。
// 拿不到的项返回 null，界面直接不显示，绝不用占位数字冒充；子代理并发来自诊断接口的 subAgents
// （第 2 期 Runtime 新增的可选字段），旧 Runtime 没有它就不显示。
import { useQuery } from '@tanstack/react-query'
import { apiJson } from '@/lib/api'
import { normalizeTokenUsage } from '@/lib/format'

export type FleetSystemInfo = {
  todayTokens: number | null
  memoryBytes: number | null
  // 子代理占用的并发槽位与上限（MAX_CONCURRENT_AGENTS）；两者缺一就不带这个字段。
  subAgents?: { running: number; max: number }
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {}
}

function finiteCount(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null
}

const USAGE_FIELDS = ['input', 'output', 'cacheRead', 'cacheWrite', 'totalTokens', 'total']

export function parseFleetSystemInfo(usage: unknown, diagnostics: unknown): FleetSystemInfo {
  // 响应里一个计数字段都没有时视为拿不到，不能被归一化成“0 tok”。
  const usageFields = record(usage)
  const hasUsage = USAGE_FIELDS.some((field) => finiteCount(usageFields[field]) !== null)
  const tokens = hasUsage ? normalizeTokenUsage(usage).totalTokens : null
  const agents = record(record(diagnostics).subAgents)
  const running = finiteCount(agents.running)
  const max = finiteCount(agents.maxConcurrent)
  return {
    todayTokens: finiteCount(tokens),
    memoryBytes: finiteCount(record(record(diagnostics).memory).rss),
    ...(running !== null && max !== null && max >= 1
      ? { subAgents: { running: Math.floor(running), max: Math.floor(max) } }
      : {}),
  }
}

// 两个接口互不依赖：任一失败只让对应项消失，不让整行报错。
export async function fetchFleetSystemInfo(signal?: AbortSignal): Promise<FleetSystemInfo> {
  const [usage, diagnostics] = await Promise.allSettled([
    apiJson<unknown>('/api/usage/today', { signal }),
    apiJson<unknown>('/api/runtime/diagnostics', { signal }),
  ])
  return parseFleetSystemInfo(
    usage.status === 'fulfilled' ? usage.value : undefined,
    diagnostics.status === 'fulfilled' ? diagnostics.value : undefined,
  )
}

export function useFleetSystemInfo(enabled: boolean) {
  return useQuery({
    queryKey: ['board', 'fleet-system-info'],
    queryFn: ({ signal }) => fetchFleetSystemInfo(signal),
    enabled,
    staleTime: 15_000,
    retry: false,
  })
}
