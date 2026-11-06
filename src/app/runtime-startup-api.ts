import { ApiError, requestJson } from '@/lib/http'

export type RuntimeStartupStatus = {
  state: 'pending' | 'ready' | 'failed'
  failure: { code: string; message: string } | null
  setAsideFiles: string[]
  failedServices: string[]
}

function text(value: unknown) {
  return typeof value === 'string' ? value : ''
}

function listOf(value: unknown, field: string) {
  if (!Array.isArray(value)) return []
  return value
    .map((item) => text((item as Record<string, unknown> | null)?.[field]))
    .filter(Boolean)
}

export function parseRuntimeStartupStatus(value: unknown): RuntimeStartupStatus {
  const record = (value && typeof value === 'object' ? value : {}) as Record<string, unknown>
  const failure = record.failure as Record<string, unknown> | null | undefined
  return {
    state: record.state === 'ready' || record.state === 'failed' ? record.state : 'pending',
    failure: failure ? { code: text(failure.code), message: text(failure.message) } : null,
    setAsideFiles: listOf(record.setAside, 'file'),
    failedServices: listOf(record.failed, 'service'),
  }
}

// 旧 Runtime（组件还没跟上）没有这个接口：返回 null，调用方不再问。
export async function fetchRuntimeStartupStatus(signal?: AbortSignal) {
  try {
    return await requestJson('/api/startup-status', { signal, parse: parseRuntimeStartupStatus })
  } catch (error) {
    if (error instanceof ApiError && error.status === 404) return null
    throw error
  }
}
