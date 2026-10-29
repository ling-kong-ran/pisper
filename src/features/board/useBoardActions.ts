// 看板内联动作：允许 / 拒绝 / 重试 / 标记已处理 / 置顶 / 归档。
// 乐观更新、失败回滚与缓存失效都在 board-actions 中完成；这里只暴露进行中状态供按钮禁用。
import { useMemo } from 'react'
import {
  approveBoardApproval,
  archiveBoardSession,
  denyBoardApproval,
  markBoardSessionHandled,
  pinBoardSession,
  retryBoardSession,
} from './board-actions'
import { boardActionKey, useBoardOverlay, type BoardActionKind } from './board-store'
import { useBoardSync } from './useBoardSync'

export function useBoardActions() {
  // 重试的运行流由看板同步持有，动作入口挂载期间必须保持同步存活。
  useBoardSync()
  const pending = useBoardOverlay((state) => state.pending)
  return useMemo(
    () => ({
      approve: approveBoardApproval,
      deny: denyBoardApproval,
      retry: retryBoardSession,
      markHandled: markBoardSessionHandled,
      pin: pinBoardSession,
      archive: archiveBoardSession,
      pending,
      isPending: (kind: BoardActionKind, sessionId: string, approvalId = '') =>
        Boolean(pending[boardActionKey(kind, sessionId, approvalId)]),
    }),
    [pending],
  )
}
