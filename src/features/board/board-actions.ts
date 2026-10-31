// 看板内联动作：乐观补丁只叠加在看板视图上，失败时撤掉补丁并恢复审批详情，
// 结束后统一刷新权威缓存。动作从不抛出，结果对象交给界面决定如何提示。
import { chatApi } from '@/features/chat/chat-api'
import { announceSessionsUpdated, type SessionOrganizationUpdate } from '@/features/chat/events'
import { queryClient } from '@/lib/startup-queries'
import { boardApi } from './board-api'
import {
  applyBoardPatch,
  BOARD_SESSIONS_QUERY_KEY,
  beginBoardAction,
  boardActionKey,
  boardApprovalDetail,
  boardDismissAttentionAvailable,
  boardSessionApprovals,
  clearBoardPatch,
  confirmBoardSession,
  endBoardAction,
  ownBoardStream,
  refreshBoardFeed,
  removeBoardApproval,
  restoreBoardApproval,
  updateBoardFeedSession,
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
  void refreshBoardFeed().catch(() => undefined)
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
  // 可批准的详情：/live 的完整详情，或预览完整的 card 审批；预览被截断时视同没有详情。
  const approval = boardApprovalDetail(sessionId, approvalId)
  // 没有完整详情时既看不到完整命令也判断不了危险等级：任何入口（包括已经过确认层的）
  // 都不发批准请求，确认层只能确认它展示过的完整参数。critical 另须经过确认层。
  if (approved && (!approval || (approval.severity === 'critical' && !confirmCritical)))
    return {
      ok: false,
      reason: 'confirmation-required',
      error: new BoardConfirmationRequiredError(),
    }
  const kind = approved ? 'approve' : 'deny'
  const key = boardActionKey(kind, sessionId, approvalId)
  if (!beginBoardAction(key, kind)) return pendingResult()
  const remaining = boardSessionApprovals(sessionId).filter((item) => item.id !== approvalId)
  removeBoardApproval(sessionId, approvalId)
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
    // 事件流在线时等这张卡片不再带着这条审批的那一帧；其余数据源立即补读。
    await confirmBoardSession(
      sessionId,
      (entry) => !entry || !entry.card.approvals.some((item) => item.id === approvalId),
      { fresh: true },
    )
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

// 行内回答代理的提问（规格 3.3：只有一题单选、选项 ≤4 时）：一次点选只发一次请求；成功后卡片先回到“运行中”，
// 等权威快照不再带这组提问；失败（含已在别处回答的 409）撤掉补丁，交给界面提示。
export async function answerBoardQuestion(
  sessionId: string,
  questionId: string,
  label: string,
): Promise<BoardActionResult> {
  const key = boardActionKey('answer', sessionId, questionId)
  if (!beginBoardAction(key, 'answer')) return pendingResult()
  const token = applyBoardPatch(sessionId, {
    needsAttention: false,
    attentionReason: null,
    streaming: true,
  })
  try {
    await chatApi.answerQuestion(sessionId, questionId, { answers: [{ selected: [label] }] })
  } catch (error) {
    clearBoardPatch(sessionId, token)
    endBoardAction(key)
    refreshInBackground()
    return { ok: false, reason: 'failed', error }
  }
  try {
    await confirmBoardSession(sessionId, undefined, { fresh: true })
  } catch {
    // 动作已成功；刷新失败时下一轮轮询会补齐。
  } finally {
    clearBoardPatch(sessionId, token)
    endBoardAction(key)
  }
  return { ok: true }
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
    await confirmBoardSession(sessionId, (entry) => !entry || entry.session.streaming === true)
  } catch {
    // 运行已受理；刷新失败只影响展示，下一轮轮询会补齐。
  } finally {
    clearBoardPatch(sessionId, token)
    endBoardAction(key)
  }
  return { ok: true }
}

// 服务端确认的整理字段直接写回权威缓存（与侧栏现有做法一致），同时写进看板条目。
function writeOrganization(update: SessionOrganizationUpdate) {
  updateBoardFeedSession(update.id, {
    pinned: update.pinned,
    archived: update.archived,
    unread: update.unread,
    needsAttention: update.needsAttention,
    attentionReason: update.attentionReason,
  })
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
  request: { pinned?: boolean; archived?: boolean; read?: boolean; dismissAttention?: true },
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

// 标记已处理：第 2 期 Runtime（board.view）用 dismissAttention 在服务端清掉失败注意力，
// 卡片靠 card.outcome 显示“失败 · 已查看”；旧 Runtime 的“已读”不清失败标记，
// 看板把已读的失败视为已处理（客户端推导）。
export function markBoardSessionHandled(sessionId: string) {
  if (boardDismissAttentionAvailable())
    return organize(
      sessionId,
      'markHandled',
      { unread: false, needsAttention: false, attentionReason: null },
      { read: true, dismissAttention: true },
    )
  return organize(sessionId, 'markHandled', { unread: false }, { read: true })
}

export function pinBoardSession(sessionId: string, pinned = true) {
  return organize(sessionId, 'pin', { pinned }, { pinned })
}

export function archiveBoardSession(sessionId: string, archived = true) {
  return organize(sessionId, 'archive', { archived }, { archived })
}

// 改名（侧栏行 ⋯ 菜单）：服务端确认后把新名字写回权威缓存与看板条目，再广播给会话页等其他读者。
// 不做乐观更新：名字是用户刚输入的文字，失败时界面保持旧名并提示即可。
export async function renameBoardSession(
  sessionId: string,
  name: string,
): Promise<BoardActionResult> {
  const key = boardActionKey('rename', sessionId)
  if (!beginBoardAction(key, 'rename')) return pendingResult()
  try {
    const confirmed = await boardApi.renameSession(sessionId, name)
    updateBoardFeedSession(sessionId, { name: confirmed })
    queryClient.setQueryData<BoardSessionsData>(BOARD_SESSIONS_QUERY_KEY, (current) =>
      current
        ? {
            ...current,
            sessions: current.sessions.map((session) =>
              session.id === sessionId ? { ...session, name: confirmed } : session,
            ),
          }
        : current,
    )
    announceSessionsUpdated({ id: sessionId, name: confirmed })
    return { ok: true }
  } catch (error) {
    refreshInBackground()
    return { ok: false, reason: 'failed', error }
  } finally {
    endBoardAction(key)
  }
}
