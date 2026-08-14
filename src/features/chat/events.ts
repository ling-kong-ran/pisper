import { STORAGE_KEYS } from '@/app/storage'
import {
  createSessionOpenRequest,
  parseSessionOpenRequest,
  type SessionOpenDisposition,
} from './dock-layout'

export const SESSION_SELECTED_EVENT = 'pisper:session-selected'
export const ACTIVE_SESSION_CHANGED_EVENT = 'pisper:active-session-changed'
export const SESSIONS_UPDATED_EVENT = 'pisper:sessions-updated'
export const COMMAND_PALETTE_REQUESTED_EVENT = 'pisper:command-palette-requested'

type SessionMessageTarget = { sessionId: string; entryId: string }

function parseSessionMessageTarget(raw: string | null): SessionMessageTarget | null {
  if (!raw) return null
  try {
    const value = JSON.parse(raw) as Partial<SessionMessageTarget>
    return typeof value.sessionId === 'string' && typeof value.entryId === 'string'
      ? { sessionId: value.sessionId, entryId: value.entryId }
      : null
  } catch {
    return null
  }
}

export function requestCommandPalette() {
  window.dispatchEvent(new Event(COMMAND_PALETTE_REQUESTED_EVENT))
}

export function requestSessionSelection(
  id: string,
  disposition: SessionOpenDisposition = 'open',
  targetEntryId = '',
) {
  const request = createSessionOpenRequest(id, disposition, targetEntryId)
  if (!request) return
  localStorage.setItem(STORAGE_KEYS.activeSession, id)
  localStorage.setItem(STORAGE_KEYS.sessionOpenRequest, JSON.stringify(request))
  if (targetEntryId) {
    localStorage.setItem(
      STORAGE_KEYS.sessionMessageTarget,
      JSON.stringify({ sessionId: id, entryId: targetEntryId }),
    )
  } else {
    localStorage.removeItem(STORAGE_KEYS.sessionMessageTarget)
  }
  window.dispatchEvent(new CustomEvent(SESSION_SELECTED_EVENT, { detail: request }))
}

export function getSessionMessageTarget(sessionId: string) {
  const target = parseSessionMessageTarget(localStorage.getItem(STORAGE_KEYS.sessionMessageTarget))
  return target?.sessionId === sessionId ? target.entryId : ''
}

export function clearSessionMessageTarget(sessionId: string, entryId: string) {
  const target = parseSessionMessageTarget(localStorage.getItem(STORAGE_KEYS.sessionMessageTarget))
  if (target?.sessionId === sessionId && target.entryId === entryId) {
    localStorage.removeItem(STORAGE_KEYS.sessionMessageTarget)
  }
}

export function consumeSessionSelectionRequest() {
  const request = parseSessionOpenRequest(localStorage.getItem(STORAGE_KEYS.sessionOpenRequest))
  localStorage.removeItem(STORAGE_KEYS.sessionOpenRequest)
  return request
}

export function announceActiveSession(id: string, model = '') {
  window.dispatchEvent(
    new CustomEvent(ACTIVE_SESSION_CHANGED_EVENT, { detail: { id: id || '', model: model || '' } }),
  )
}

export function announceSessionsUpdated() {
  window.dispatchEvent(new Event(SESSIONS_UPDATED_EVENT))
}
