// @public 会话整理 API：Runtime 持有状态，界面只广播服务端确认后的字段。
import { apiJson } from '@/lib/api'
import { announceSessionsUpdated, type SessionOrganizationUpdate } from './events'

export type SessionOrganizationPatch = {
  pinned?: boolean
  archived?: boolean
  read?: boolean
}

export class SessionOrganizationProtocolError extends Error {
  readonly code = 'invalid_session_organization_response'
}

export function parseSessionOrganization(value: unknown): SessionOrganizationUpdate {
  if (!value || typeof value !== 'object') throw new SessionOrganizationProtocolError()
  const { id, pinned, archived, unread, needsAttention, attentionReason } = value as Record<
    string,
    unknown
  >
  if (
    typeof id !== 'string' ||
    !id ||
    typeof pinned !== 'boolean' ||
    typeof archived !== 'boolean' ||
    typeof unread !== 'boolean' ||
    typeof needsAttention !== 'boolean' ||
    (attentionReason !== null && attentionReason !== 'approval' && attentionReason !== 'failure')
  )
    throw new SessionOrganizationProtocolError()
  return { id, pinned, archived, unread, needsAttention, attentionReason }
}

export async function updateSessionOrganization(
  id: string,
  patch: SessionOrganizationPatch,
): Promise<SessionOrganizationUpdate> {
  if (!id || !Object.keys(patch).length) throw new SessionOrganizationProtocolError()
  const response = await apiJson<unknown>(`/api/sessions/${encodeURIComponent(id)}/organization`, {
    method: 'PATCH',
    body: JSON.stringify(patch),
  })
  const update = parseSessionOrganization(response)
  if (update.id !== id) throw new SessionOrganizationProtocolError()
  announceSessionsUpdated(update)
  return update
}
