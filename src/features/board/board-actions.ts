// 看板内联动作：乐观补丁只叠加在看板视图上，失败时撤掉补丁并恢复审批详情，
// 结束后统一刷新权威缓存。动作从不抛出，结果对象交给界面决定如何提示。
import type { SessionOrganizationUpdate } from '@/features/chat/events'
import { queryClient } from '@/lib/startup-queries'
import { boardApi } from './board-api'
import {
  applyBoardPatch,
  BOARD_SESSIONS_QUERY_KEY,
  beginBoardAction,
  boardActionKey,
  clearBoardPatch,
  confirmBoardSessions,
  endBoardAction,
  ownBoardStream,
  removeBoardApproval,
  restoreBoardApproval,
  useBoardOverlay,
  type BoardSessionPatch,
  type BoardSessionsData,
} from './board-store'

export type BoardActionFailure = 'pending' | 'confirmation-required' | 'failed'

export type BoardActionResult =
  { ok: true } | { ok: false; reason: BoardActionFailure; error: unknown }

export class BoardConfirmationRequiredError extends Error {
  readonly code = 'board_confirmation_required'
}

const pendingResult = (): BoardActionResult => ({
  ok: false,
  reason: 'pending',
  error: null,
})

function refreshInBackground() {
  void confirmBoardSessions().catch(() => undefined)
}

type ApprovalOptions = {
  // critical（或详情未知）的审批必须经过确认层（按住或在确认层内回车）后才能带上 true；
  // ⌥A 与舰队浮层永远不传，因此不能直接批准任何高危操作。
  confirmCritical?: boolean
}

async function resolveApproval(
  sessionId: string,
  approvalId: string,
  approved: boolean,
  { confirmCritical = false }: ApprovalOptions,
): Promise<BoardActionResult> {
  const entry = useBoardOverlay.getState().live[sessionId]
  const approval = entry?.approvals.find((item) => item.id === approvalId)
  // 没有详情时既看不到完整命令也判断不了危险等级，同样必须先经过确认层。
  if (approved && (!approval || approval.severity === 'critical') && !confirmCritical)
    return {
      ok: false,
      reason: 'confirmation-required',
      error: new BoardConfirmationRequiredError(),
    }
  const kind = approved ? 'approve' : 'deny'
  const key = boardActionKey(kind, sessionId, approvalId)
  if (!beginBoardAction(key, kind)) return pendingResult()
  removeBoardApproval(sessionId, approvalId)
  const remaining = (entry?.approvals ?? []).filter((item) => item.id !== approvalId)
  // 最后一条审批处理后运行恢复，卡片先回到“运行中”；仍有其他审批时保持在“需要你”。
  const token = remaining.length
    ? null
    : applyBoardPatch(sessionId, { needsAttention: false, attentionReason: null, streaming: true })
  try {
    await boardApi.resolveApproval(sessionId, approvalId, approved)
  } catch (error) {
    if (token !== null) clearBoardPatch(sessionId, token)
    if (approval) restoreBoardApproval(sessionId, approval)
    endBoardAction(key)
    refreshInBackground()
    return { ok: false, reason: 'failed', error }
  }
  try {
    await confirmBoardSessions()
  } catch {
    // 动作已成功；刷新失败时撤掉补丁回到最后一份权威快照，下一轮轮询会补齐。
  } finally {
    if (token !== null) clearBoardPatch(sessionId, token)
    endBoardAction(key)
  }
  return { ok: true }
}

export function approveBoardApproval(
  sessionId: string,
  approvalId: string,
  options: ApprovalOptions = {},
) {
  return resolveApproval(sessionId, approvalId, true, options)
}

export function denyBoardApproval(sessionId: string, approvalId: string) {
  return resolveApproval(sessionId, approvalId, false, {})
}

export async function retryBoardSession(sessionId: string): Promise<BoardActionResult> {
  const key = boardActionKey('retry', sessionId)
  if (!beginBoardAction(key, 'retry')) return pendingResult()
  const token = applyBoardPatch(sessionId, { streaming: true })
  try {
    // 流由看板 store 持有；这里只等 Runtime 受理，运行本身在后台继续。
    await ownBoardStream(sessionId, 'retry', (onEvent, signal) =>
      boardApi.retry(sessionId, onEvent, signal),
    )
  } catch (error) {
    clearBoardPatch(sessionId, token)
    endBoardAction(key)
    refreshInBackground()
    return { ok: false, reason: 'failed', error }
  }
  try {
    await confirmBoardSessions()
  } catch {
    // 运行已受理；刷新失败只影响展示，下一轮轮询会补齐。
  } finally {
    clearBoardPatch(sessionId, token)
    endBoardAction(key)
  }
  return { ok: true }
}

// 服务端确认的整理字段直接写回权威缓存，与侧栏现有做法一致。
function writeOrganization(update: SessionOrganizationUpdate) {
  queryClient.setQueryData<BoardSessionsData>(BOARD_SESSIONS_QUERY_KEY, (current) =>
    current
      ? {
          ...current,
          sessions: current.sessions.map((session) =>
            session.id === update.id
              ? {
                  ...session,
                  pinned: update.pinned,
                  archived: update.archived,
                  unread: update.unread,
                  needsAttention: update.needsAttention,
                  attentionReason: update.attentionReason,
                }
              : session,
          ),
        }
      : current,
  )
}

async function organize(
  sessionId: string,
  kind: 'markHandled' | 'pin' | 'archive',
  patch: BoardSessionPatch,
  request: { pinned?: boolean; archived?: boolean; read?: boolean },
): Promise<BoardActionResult> {
  const key = boardActionKey(kind, sessionId)
  if (!beginBoardAction(key, kind)) return pendingResult()
  const token = applyBoardPatch(sessionId, patch)
  try {
    writeOrganization(await boardApi.updateOrganization(sessionId, request))
    return { ok: true }
  } catch (error) {
    refreshInBackground()
    return { ok: false, reason: 'failed', error }
  } finally {
    clearBoardPatch(sessionId, token)
    endBoardAction(key)
  }
}

// 第 1 期 Runtime 的“已读”不会清掉失败标记；看板把已读的失败视为已处理（失败 · 已查看）。
export function markBoardSessionHandled(sessionId: string) {
  return organize(sessionId, 'markHandled', { unread: false }, { read: true })
}

export function pinBoardSession(sessionId: string, pinned = true) {
  return organize(sessionId, 'pin', { pinned }, { pinned })
}

export function archiveBoardSession(sessionId: string, archived = true) {
  return organize(sessionId, 'archive', { archived }, { archived })
}
