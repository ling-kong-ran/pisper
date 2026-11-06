// 记忆页的 API：页面和展示组件都经这里，不直接拼路径。
import { apiJson } from '@/lib/api'
import type {
  MemoryDashboard,
  MemoryDetail,
  MemoryEvent,
  MemoryEvolutionData,
  MemoryListStatus,
  MemoryNode,
  MemorySessionDigest,
  MemorySpace,
} from './memory-types'

const node = (id: string) => `/api/memory/nodes/${encodeURIComponent(id)}`

export function fetchMemoryDashboard(
  options: {
    spaceId?: string
    query?: string
    status?: MemoryListStatus
    before?: string
    limit?: number
  } = {},
  signal?: AbortSignal,
) {
  const params = new URLSearchParams()
  if (options.spaceId) params.set('spaceId', options.spaceId)
  if (options.query?.trim()) params.set('query', options.query.trim())
  if (options.status && options.status !== 'current') params.set('status', options.status)
  if (options.before) params.set('before', options.before)
  if (options.limit) params.set('limit', String(options.limit))
  return apiJson<MemoryDashboard>(`/api/memory?${params}`, { signal })
}

export function fetchMemoryEvolution(spaceId: string, signal?: AbortSignal) {
  const params = new URLSearchParams({ spaceId })
  return apiJson<MemoryEvolutionData>(`/api/memory/evolution?${params}`, { signal })
}

export function fetchMemoryDetail(id: string, signal?: AbortSignal) {
  return apiJson<MemoryDetail>(node(id), { signal })
}

export function fetchMemoryEvents(
  options: { spaceId?: string; before?: string; limit?: number } = {},
  signal?: AbortSignal,
) {
  const params = new URLSearchParams()
  if (options.spaceId) params.set('spaceId', options.spaceId)
  if (options.before) params.set('before', options.before)
  if (options.limit) params.set('limit', String(options.limit))
  return apiJson<{ events: MemoryEvent[] }>(`/api/memory/events?${params}`, { signal })
}

export function fetchMemorySessionDigests(
  options: { spaceId?: string; before?: string; limit?: number } = {},
  signal?: AbortSignal,
) {
  const params = new URLSearchParams()
  if (options.spaceId) params.set('spaceId', options.spaceId)
  if (options.before) params.set('before', options.before)
  if (options.limit) params.set('limit', String(options.limit))
  return apiJson<{ sessions: MemorySessionDigest[] }>(`/api/memory/sessions?${params}`, { signal })
}

export function undoMemoryEvent(id: string) {
  return apiJson(`/api/memory/events/${encodeURIComponent(id)}/undo`, {
    method: 'POST',
    body: '{}',
  })
}

export function consolidateMemory() {
  return apiJson<{ ran: boolean }>('/api/memory/consolidate', { method: 'POST', body: '{}' })
}

export function clearMemoryEpisodes(spaceId = '') {
  return apiJson<{ cleared: number }>('/api/memory/episodes/clear', {
    method: 'POST',
    body: JSON.stringify({ spaceId }),
  })
}

export function exportMemoryMarkdown(spaceId = '') {
  const params = new URLSearchParams()
  if (spaceId) params.set('spaceId', spaceId)
  return apiJson<{ markdown: string }>(`/api/memory/export?${params}`)
}

export type MemoryDraft = Pick<MemoryNode, 'spaceId' | 'title' | 'content' | 'type'> & {
  importance?: number
  sourcePath?: string
  pinned?: boolean
}

export function createMemory(draft: MemoryDraft) {
  return apiJson<MemoryNode>('/api/memory/nodes', { method: 'POST', body: JSON.stringify(draft) })
}

export function updateMemory(id: string, draft: Partial<MemoryDraft>) {
  return apiJson<MemoryNode>(node(id), { method: 'PATCH', body: JSON.stringify(draft) })
}

export function deleteMemory(id: string) {
  return apiJson(node(id), { method: 'DELETE' })
}

export function memoryAction(id: string, action: 'restore' | 'confirm' | 'pin' | 'unpin') {
  return apiJson<MemoryNode>(`${node(id)}/${action}`, { method: 'POST', body: '{}' })
}

export function createMemorySpace(name: string) {
  return apiJson<MemorySpace>('/api/memory/spaces', {
    method: 'POST',
    body: JSON.stringify({ name, kind: 'custom' }),
  })
}

export function renameMemorySpace(id: string, name: string) {
  return apiJson<MemorySpace>(`/api/memory/spaces/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    body: JSON.stringify({ name }),
  })
}

export function deleteMemorySpace(id: string) {
  return apiJson(`/api/memory/spaces/${encodeURIComponent(id)}`, { method: 'DELETE' })
}
