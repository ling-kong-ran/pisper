// 会话顶栏（桌面 52px 的标题 ▾ 菜单）与手机顶栏（56px 的标题 ▾ 底部表单）共用的部分：运行时长、归档、侧聊入口，
// 以及两端相同的会话操作。菜单项先整理成一份数据（entries），桌面渲染成下拉菜单，手机渲染成底部表单的行。顺序按规格 v2 第 2.5 节“会话其余操作”：重命名 · 侧聊 · 归档 | 移动工作区 ·
// 压缩上下文（带当前用量）· 目标模式 · 工作流运行记录 · 语音模式。“导出 Markdown”（A9）只在 Runtime 声明
// sessionExport 时出现（不放一个点了没反应的项）。每一项都只调用现有实现，两种页头不各写一份。
import { Fragment, useContext, useEffect, useState, type ReactNode } from 'react'
import {
  type LucideIcon,
  Archive,
  AudioLines,
  FileDown,
  FolderOpen,
  MessageSquare,
  Pencil,
  Shrink,
  Target,
  Workflow,
} from 'lucide-react'
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
  DropdownMenuShortcut,
} from '@/components/ui/dropdown-menu'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import type { SessionSummary } from '@/types/chat'
import { chatApi } from './chat-api'
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
  // 桌面标题菜单里的“侧聊”（规格 2.5）；手机顶栏的“⋯”自己放侧聊入口，不传。
  onSideChat?: () => void
  // 当前上下文用量（0–100），写在“压缩上下文”一项的右侧；拿不到时不写。
  contextPercent?: number | null
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

// 一项会话操作：桌面下拉菜单与手机底部表单共用。separator 表示这一项之前有分隔线。
export type SessionMenuEntry = {
  key: string
  icon: LucideIcon
  label: string
  onSelect: () => void
  disabled?: boolean
  // 目标模式这类开关项：桌面是勾选菜单项，手机是带 aria-pressed 的行。
  checked?: boolean
  // 右侧的读数（压缩上下文的当前用量）。
  trailing?: string
  trailingTitle?: string
  separator?: boolean
  // 冒烟与守卫用的 data 属性名（不含 data- 前缀）。
  marker?: string
}

// “⋯”菜单的公共段与工作流运行记录对话框：调用方把返回的 items 放进自己的 DropdownMenuContent，
// dialog 放在菜单之外（菜单关闭后对话框仍要留着）。
export function useSessionMenu(session: SessionSummary, actions: SessionMenuActions) {
  const { t, language } = useI18n()
  const chrome = useContext(SessionPageChromeContext)
  const [workflowsOpen, setWorkflowsOpen] = useState(false)
  const [archiving, setArchiving] = useState(false)
  const [exporting, setExporting] = useState(false)
  const exportAvailable = useRuntimeCapabilitiesStore(
    (state) => state.capabilities.features.sessionExport === true,
  )
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

  // 导出 Markdown（规格 2.5 / 7.4，A9）：Runtime 只读已投影的转录，不含思考过程与完整个人路径；
  // 这里只负责下载。文件名按会话标题去掉路径分隔与保留字符。
  const exportMarkdown = async () => {
    if (exporting) return
    setExporting(true)
    try {
      const blob = await chatApi.exportSessionMarkdown(session.id, language)
      const url = URL.createObjectURL(blob)
      const anchor = document.createElement('a')
      anchor.href = url
      anchor.download = `${title.replace(/[\\/:*?"<>|\p{Cc}]/gu, '-').slice(0, 80) || 'session'}.md`
      anchor.click()
      window.setTimeout(() => URL.revokeObjectURL(url), 1000)
      notify(t('chat:sessionMenu.exported', { title }))
    } catch {
      notify(t('chat:sessionMenu.exportFailed'), 'error')
    } finally {
      setExporting(false)
    }
  }

  const percent =
    typeof actions.contextPercent === 'number' && Number.isFinite(actions.contextPercent)
      ? Math.round(Math.min(100, Math.max(0, actions.contextPercent)))
      : null

  const entries: SessionMenuEntry[] = [
    {
      key: 'rename',
      icon: Pencil,
      label: t('chat:focusSession.renameChat'),
      onSelect: actions.onRename,
    },
    ...(actions.onSideChat
      ? [
          {
            key: 'side-chat',
            icon: MessageSquare,
            label: t('board:inspector.sideChat'),
            onSelect: actions.onSideChat,
            marker: 'session-side-chat',
          },
        ]
      : []),
    ...(exportAvailable
      ? [
          {
            key: 'export',
            icon: FileDown,
            label: t('chat:sessionMenu.exportMarkdown'),
            onSelect: () => void exportMarkdown(),
            disabled: exporting,
            marker: 'session-export',
          },
        ]
      : []),
    {
      key: 'archive',
      icon: Archive,
      label: t('board:card.archive'),
      onSelect: () => void archive(),
      disabled: archiving,
    },
    {
      key: 'workspace',
      icon: FolderOpen,
      label: t('board:session.moveWorkspace'),
      onSelect: actions.onWorkspace,
      disabled: actions.workspaceDisabled,
      separator: true,
    },
    {
      key: 'compact',
      icon: Shrink,
      label: t('chat:focusSession.compactContextNow'),
      onSelect: () => actions.onCompact?.(),
      disabled: actions.compactDisabled || !actions.onCompact,
      trailing: percent !== null ? `${percent}%` : undefined,
      trailingTitle: percent !== null ? t('board:session.contextUsage', { percent }) : undefined,
    },
    ...(actions.goalAvailable
      ? [
          {
            key: 'goal',
            icon: Target,
            label: t('board:session.goalMode'),
            onSelect: actions.onToggleGoal,
            disabled: actions.goalDisabled,
            checked: actions.goalEngaged,
          },
        ]
      : []),
    ...(actions.workflowsAvailable
      ? [
          {
            key: 'workflows',
            icon: Workflow,
            label: t('board:session.workflowRuns'),
            onSelect: () => setWorkflowsOpen(true),
          },
        ]
      : []),
    {
      key: 'voice',
      icon: AudioLines,
      label: t('chat:voiceMode.open'),
      onSelect: actions.onVoiceMode,
    },
  ]

  const items = (
    <>
      {entries.map((entry) => {
        const Icon = entry.icon
        const marker = entry.marker ? { [`data-${entry.marker}`]: true } : {}
        const item =
          entry.checked !== undefined ? (
            <DropdownMenuCheckboxItem
              checked={entry.checked}
              disabled={entry.disabled}
              onCheckedChange={entry.onSelect}
              {...marker}
            >
              <Icon />
              {entry.label}
            </DropdownMenuCheckboxItem>
          ) : (
            <DropdownMenuItem disabled={entry.disabled} onSelect={entry.onSelect} {...marker}>
              <Icon />
              {entry.label}
              {entry.trailing && (
                <DropdownMenuShortcut
                  className="font-mono tabular-nums"
                  title={entry.trailingTitle}
                  data-session-context-usage
                >
                  {entry.trailing}
                </DropdownMenuShortcut>
              )}
            </DropdownMenuItem>
          )
        return (
          <Fragment key={entry.key}>
            {entry.separator && <DropdownMenuSeparator />}
            {item}
          </Fragment>
        )
      })}
      {actions.extraItems && <DropdownMenuSeparator />}
      {actions.extraItems}
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

  return { entries, items, dialog }
}
