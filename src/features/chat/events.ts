// 会话事件总线：用 window CustomEvent 在应用内广播会话选择/创建/更新等
// 事件。请求同时写入 localStorage（跨页面/重启持久），供壳层与其他
// 会话面板消费；consum 函数取出即删，避免重复处理。
import { STORAGE_KEYS } from '@/app/storage'
import {
  createSessionOpenRequest,
  parseSessionOpenRequest,
  type SessionOpenDisposition,
} from './dock-layout'

export const SESSION_SELECTED_EVENT = 'pisper:session-selected'
export const SESSION_CREATE_REQUESTED_EVENT = 'pisper:session-create-requested'
export const ACTIVE_SESSION_CHANGED_EVENT = 'pisper:active-session-changed'
export const SESSIONS_UPDATED_EVENT = 'pisper:sessions-updated'
export const COMMAND_PALETTE_REQUESTED_EVENT = 'pisper:command-palette-requested'

type SessionMessageTarget = { sessionId: string; entryId: string }
export type SessionCreateRequest = { cwd: string }

// 校验/归一化会话创建请求：cwd 必须是非空字符串，否则返回 null。
function normalizeSessionCreateRequest(value: unknown): SessionCreateRequest | null {
  if (!value || typeof value !== 'object') return null
  const cwd = (value as Partial<SessionCreateRequest>).cwd
  return typeof cwd === 'string' && cwd.trim() ? { cwd } : null
}

// 解析持久化的创建请求（JSON 解析失败返回 null）。
function parseSessionCreateRequest(raw: string | null): SessionCreateRequest | null {
  if (!raw) return null
  try {
    return normalizeSessionCreateRequest(JSON.parse(raw))
  } catch {
    return null
  }
}

// 解析“消息定位目标”（会话+条目），两个字段都必须是字符串。
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

// 请求打开命令面板（广播给应用壳）。
export function requestCommandPalette() {
  window.dispatchEvent(new Event(COMMAND_PALETTE_REQUESTED_EVENT))
}

// 请求创建会话（带工作目录）：持久化请求并广播；
// 非法 cwd 返回 false 表示未触发。
export function requestSessionCreation(cwd: string) {
  const request = normalizeSessionCreateRequest({ cwd })
  if (!request) return false
  localStorage.setItem(STORAGE_KEYS.sessionCreateRequest, JSON.stringify(request))
  window.dispatchEvent(new CustomEvent(SESSION_CREATE_REQUESTED_EVENT, { detail: request }))
  return true
}

// 消费创建请求（取出即删），用于跨页面/重启后补建会话。
export function consumeSessionCreationRequest() {
  const request = parseSessionCreateRequest(localStorage.getItem(STORAGE_KEYS.sessionCreateRequest))
  localStorage.removeItem(STORAGE_KEYS.sessionCreateRequest)
  return request
}

// 请求选中/打开会话：写 activeSession + 打开请求 +（可选）消息定位目标，
// 并广播 SESSION_SELECTED 事件给各 Dock 面板。
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

// 取会话的消息定位目标（定位到指定条目），不匹配返回空串。
export function getSessionMessageTarget(sessionId: string) {
  const target = parseSessionMessageTarget(localStorage.getItem(STORAGE_KEYS.sessionMessageTarget))
  return target?.sessionId === sessionId ? target.entryId : ''
}

// 清除消息定位目标：仅当与会话+条目都匹配时删除，避免误清其它定位。
export function clearSessionMessageTarget(sessionId: string, entryId: string) {
  const target = parseSessionMessageTarget(localStorage.getItem(STORAGE_KEYS.sessionMessageTarget))
  if (target?.sessionId === sessionId && target.entryId === entryId) {
    localStorage.removeItem(STORAGE_KEYS.sessionMessageTarget)
  }
}

// 消费会话打开请求（取出即删），供启动恢复上次的打开意图。
export function consumeSessionSelectionRequest() {
  const request = parseSessionOpenRequest(localStorage.getItem(STORAGE_KEYS.sessionOpenRequest))
  localStorage.removeItem(STORAGE_KEYS.sessionOpenRequest)
  return request
}

// 广播“活动会话已变更”（带 id 与模型），供状态栏/其它面板订阅。
export function announceActiveSession(id: string, model = '') {
  window.dispatchEvent(
    new CustomEvent(ACTIVE_SESSION_CHANGED_EVENT, { detail: { id: id || '', model: model || '' } }),
  )
}

// 广播“会话列表已更新”，供依赖列表快照的面板刷新。
export function announceSessionsUpdated() {
  window.dispatchEvent(new Event(SESSIONS_UPDATED_EVENT))
}
