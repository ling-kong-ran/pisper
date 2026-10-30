// 会话页头（桌面 44px）与手机顶栏（56px）共用的部分：运行时长、归档、侧聊入口，
// 以及“⋯”菜单里两端相同的会话操作（重命名 / 移动工作区 / 压缩上下文 / 目标模式 / 工作流运行记录 /
// 语音模式 / 归档）。每一项都只调用现有实现，两种页头不各写一份。
import { useContext, useEffect, useState, type ReactNode } from 'react'
import { Archive, AudioLines, FolderOpen, Pencil, Shrink, Target, Workflow } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  DropdownMenuCheckboxItem,
  DropdownMenuItem,
  DropdownMenuSeparator,
} from '@/components/ui/dropdown-menu'
import type { SessionSummary } from '@/types/chat'
import { updateSessionOrganization } from './session-organization-api'
import { useContextPagesStore } from './session-context-pages-store'
import { SessionPageChromeContext, type SessionInspectorControl } from './session-workspace-context'
import { SessionWorkflowRuns } from './SessionWorkflowRuns'

// 运行时长每秒推进；页面隐藏时浏览器会节流定时器，回到前台立即补一次。
export function useRunElapsed(startedAt: string | null | undefined, running: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!running) return
    setNow(Date.now())
    const timer = setInterval(() => setNow(Date.now()), 1_000)
    return () => clearInterval(timer)
  }, [running])
  const start = startedAt ? Date.parse(startedAt) : Number.NaN
  return running && Number.isFinite(start) ? Math.max(0, now - start) : null
}

// 侧聊是检查器里的临时标签：已有侧聊页就切过去，没有就新建一页。
export function openSessionSideChat(sessionId: string, inspector: SessionInspectorControl | null) {
  useContextPagesStore.getState().update(sessionId, 'side-chat', {
    type: 'open',
    kind: 'side-chat',
  })
  inspector?.show('side-chat')
}

export type SessionMenuActions = {
  onRename: () => void
  onWorkspace: () => void
  workspaceDisabled: boolean
  onCompact?: () => void
  compactDisabled: boolean
  goalAvailable: boolean
  goalEngaged: boolean
  goalDisabled: boolean
  onToggleGoal: () => void
  workflowsAvailable: boolean
  onVoiceMode: () => void
  notify: Notify
  // 桌面终端面板、主题切换等沿用旧顶栏的菜单项。
  extraItems?: ReactNode
}

// “⋯”菜单的公共段与工作流运行记录对话框：调用方把返回的 items 放进自己的 DropdownMenuContent，
// dialog 放在菜单之外（菜单关闭后对话框仍要留着）。
export function useSessionMenu(session: SessionSummary, actions: SessionMenuActions) {
  const { t } = useI18n()
  const chrome = useContext(SessionPageChromeContext)
  const [workflowsOpen, setWorkflowsOpen] = useState(false)
  const [archiving, setArchiving] = useState(false)
  const title = session.name || t('navigation:pageHeader.newChat')
  const { notify } = actions

  const archive = async () => {
    if (archiving) return
    setArchiving(true)
    try {
      await updateSessionOrganization(session.id, { archived: true })
      notify(t('board:action.archived', { title }))
      chrome?.onArchived?.(session.id)
    } catch {
      notify(t('board:action.failed'), 'error')
    } finally {
      setArchiving(false)
    }
  }

  const items = (
    <>
      <DropdownMenuItem onSelect={actions.onRename}>
        <Pencil />
        {t('chat:focusSession.renameChat')}
      </DropdownMenuItem>
      <DropdownMenuItem disabled={actions.workspaceDisabled} onSelect={actions.onWorkspace}>
        <FolderOpen />
        {t('board:session.moveWorkspace')}
      </DropdownMenuItem>
      <DropdownMenuItem
        disabled={actions.compactDisabled || !actions.onCompact}
        onSelect={() => actions.onCompact?.()}
      >
        <Shrink />
        {t('chat:focusSession.compactContextNow')}
      </DropdownMenuItem>
      {actions.goalAvailable && (
        <DropdownMenuCheckboxItem
          checked={actions.goalEngaged}
          disabled={actions.goalDisabled}
          onCheckedChange={actions.onToggleGoal}
        >
          <Target />
          {t('board:session.goalMode')}
        </DropdownMenuCheckboxItem>
      )}
      {actions.workflowsAvailable && (
        <DropdownMenuItem onSelect={() => setWorkflowsOpen(true)}>
          <Workflow />
          {t('board:session.workflowRuns')}
        </DropdownMenuItem>
      )}
      <DropdownMenuItem onSelect={actions.onVoiceMode}>
        <AudioLines />
        {t('chat:voiceMode.open')}
      </DropdownMenuItem>
      {actions.extraItems && <DropdownMenuSeparator />}
      {actions.extraItems}
      <DropdownMenuSeparator />
      <DropdownMenuItem disabled={archiving} onSelect={() => void archive()}>
        <Archive />
        {t('board:card.archive')}
      </DropdownMenuItem>
    </>
  )

  const dialog = (
    <Dialog open={workflowsOpen} onOpenChange={setWorkflowsOpen}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{t('board:session.workflowRuns')}</DialogTitle>
          <DialogDescription>{title}</DialogDescription>
        </DialogHeader>
        {workflowsOpen && (
          <SessionWorkflowRuns
            sessionId={session.id}
            empty={
              <p className="py-6 text-center text-sm text-content-muted">
                {t('board:session.workflowRunsEmpty')}
              </p>
            }
          />
        )}
      </DialogContent>
    </Dialog>
  )

  return { items, dialog }
}
