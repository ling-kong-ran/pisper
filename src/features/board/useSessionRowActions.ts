// 会话行 ⋯ 菜单的动作（规格 2.2、3.3：全部代理行的 ⋯ 菜单同侧栏行）：打开、置顶 / 取消置顶、重命名、
// 标记已处理（失败）、复制会话链接、归档。侧栏与全部代理共用这一份实现，结果提示与失败文案一致。
// 动作经 useBoardActions 走看板 store 的乐观更新，不直接调用接口；菜单的样子在 SessionRowMenu。
import { useCallback, useMemo, useRef } from 'react'
import type { Notify } from '@/app/route-context'
import { sessionPath } from '@/app/routes'
import { forgetLastSession } from '@/app/session-home'
import { useI18n } from '@/app/use-i18n'
import type { PromptDialogOptions } from '@/hooks/useAppDialog'
import type { BoardActionResult } from './board-actions'
import { useBoardActions } from './useBoardActions'

export type SessionRowTarget = { id: string; title: string; pinned: boolean }

export type SessionRowActions = {
  open: (row: SessionRowTarget) => void
  pin: (row: SessionRowTarget) => void
  rename: (row: SessionRowTarget) => void
  markHandled: (row: SessionRowTarget) => void
  copyLink: (row: SessionRowTarget) => void
  archive: (row: SessionRowTarget) => void
  // 动作失败的统一提示（进行中的重复点击不算失败）。
  report: (result: BoardActionResult) => void
}

// 复制会话链接：沿用当前地址的协议、主机与路由形式（哈希路由时链接也带 #）。
export function sessionShareHref(id: string) {
  const { origin, pathname, search, hash } = window.location
  return hash.startsWith('#/')
    ? `${origin}${pathname}${search}#${sessionPath(id)}`
    : `${origin}${sessionPath(id)}`
}

export function useSessionRowActions({
  notify,
  requestText,
  onOpenSession,
  onArchived,
}: {
  notify: Notify
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  onOpenSession: (sessionId: string) => void
  // 归档成功之后：侧栏归档的是当前会话时由外壳回到主页。
  onArchived?: (sessionId: string) => void
}): SessionRowActions {
  const { t } = useI18n()
  const actions = useBoardActions()
  const actionsRef = useRef(actions)
  actionsRef.current = actions

  const report = useCallback(
    (result: BoardActionResult) => {
      if (result.ok || result.reason === 'pending') return
      notify(t('board:action.failed'), 'error')
    },
    [notify, t],
  )

  const rename = useCallback(
    async (row: SessionRowTarget) => {
      const name = await requestText({
        title: t('board:sidebar.renameTitle'),
        inputLabel: t('board:sidebar.renameLabel'),
        value: row.title,
        confirmLabel: t('board:sidebar.renameSave'),
        maxLength: 200,
      })
      const trimmed = name?.trim()
      if (!trimmed || trimmed === row.title) return
      const result = await actionsRef.current.rename(row.id, trimmed)
      if (result.ok) notify(t('board:sidebar.renamed'))
      else report(result)
    },
    [notify, report, requestText, t],
  )

  return useMemo(
    () => ({
      open: (row) => onOpenSession(row.id),
      pin: (row) => void actionsRef.current.pin(row.id, !row.pinned).then(report),
      rename: (row) => void rename(row),
      markHandled: (row) => void actionsRef.current.markHandled(row.id).then(report),
      copyLink: (row) => {
        const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
        if (!clipboard) {
          notify(t('board:action.copyFailed'), 'error')
          return
        }
        void clipboard.writeText(sessionShareHref(row.id)).then(
          () => notify(t('board:action.copied')),
          () => notify(t('board:action.copyFailed'), 'error'),
        )
      },
      archive: (row) =>
        void actionsRef.current.archive(row.id, true).then((result) => {
          if (!result.ok) {
            report(result)
            return
          }
          notify(t('board:action.archived', { title: row.title }))
          // 归档的是“上次停留的会话”时清掉记录，主页随后显示新任务草稿，不再落回它。
          forgetLastSession(row.id)
          onArchived?.(row.id)
        }),
      report,
    }),
    [notify, onArchived, onOpenSession, rename, report, t],
  )
}
