import { apiJson } from '@/lib/api'
import { queryClient } from '@/lib/startup-queries'

export type SessionChangeSummary = {
  status: 'known' | 'partial' | 'unavailable'
  changedFiles: number | null
  pendingFiles: number | null
  added: number | null
  removed: number | null
  unknownFiles: number
  capped: boolean
}

export function parseSessionChangeSummary(value: unknown): SessionChangeSummary {
  if (!value || typeof value !== 'object') throw new Error('Invalid change summary')
  const result = value as Record<string, unknown>
  if (
    (result.status !== 'known' && result.status !== 'partial' && result.status !== 'unavailable') ||
    !Number.isInteger(result.unknownFiles) ||
    Number(result.unknownFiles) < 0 ||
    typeof result.capped !== 'boolean' ||
    !(['changedFiles', 'pendingFiles', 'added', 'removed'] as const).every((key) =>
      result.status === 'known'
        ? Number.isInteger(result[key]) && Number(result[key]) >= 0
        : result[key] === null,
    )
  )
    throw new Error('Invalid change summary')
  return result as SessionChangeSummary
}

export async function getSessionChangeSummary(id: string) {
  return parseSessionChangeSummary(
    await apiJson<unknown>(`/api/sessions/${encodeURIComponent(id)}/change-summary`),
  )
}

export function invalidateSessionChangeSummary(id: string) {
  return queryClient.invalidateQueries({ queryKey: ['session-change-summary', id] })
}
