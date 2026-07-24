import { STORAGE_KEYS } from '../../app/storage'
import {
  createSessionOpenRequest,
  parseSessionOpenRequest,
  type SessionOpenDisposition,
} from './dock-layout'

export const SESSION_SELECTED_EVENT = 'vesper:session-selected'
export const ACTIVE_SESSION_CHANGED_EVENT = 'vesper:active-session-changed'
export const SESSIONS_UPDATED_EVENT = 'vesper:sessions-updated'

export function requestSessionSelection(id: string, disposition: SessionOpenDisposition = 'open') {
  const request = createSessionOpenRequest(id, disposition)
  if (!request) return
  localStorage.setItem(STORAGE_KEYS.activeSession, id)
  localStorage.setItem(STORAGE_KEYS.sessionOpenRequest, JSON.stringify(request))
  window.dispatchEvent(new CustomEvent(SESSION_SELECTED_EVENT, { detail: request }))
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
