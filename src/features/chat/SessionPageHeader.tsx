// 会话页头（规格 4.3 第一条，44px）：标题 15/600、项目标签、运行状态标签、上下文环、
// 分支（追忆分支 · 会话树）、侧聊、标签（Turn 标签）、“⋯”菜单与检查器开关。
// 取代第 1 期的过渡返回条：返回入口在泳道条最左侧，标题只出现这一次。
// “⋯”里的每一项都复用现有实现：重命名 / 移动工作区 / 压缩上下文 / 目标模式 / 工作流运行记录 / 语音模式 / 归档。
// 触屏（pointer: coarse）：右侧按钮本来就抬到 44px，页头加高到 48px 才完整放得下（44px 页头扣掉底边只剩 43，
// 上缘那半像素会落到泳道条上）；标题与项目标签保持可见尺寸，命中区用伪元素补到 44px。
import { useContext, type ReactNode } from 'react'
import {
  GitBranch,
  MessageSquare,
  MoreHorizontal,
  PanelRight,
  PanelRightClose,
  Pencil,
  Tag,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { StatusTag } from '@/components/app/StatusTag'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Kbd } from '@/components/ui/kbd'
import { formatCompactDuration, workspaceName } from '@/lib/format'
import { ariaKeyShortcuts } from '@/lib/shortcut-aria'
import { formatShortcut } from '@/lib/shortcuts'
import { TOUCH_HIT_AREA } from '@/lib/touch-target'
import { cn } from '@/lib/utils'
import { useShortcutStore } from '@/stores/shortcut-store'
import type { SessionSummary } from '@/types/chat'
import { openSessionSideChat, useRunElapsed, useSessionMenu } from './session-header-menu'
import { SessionInspectorContext } from './session-workspace-context'
import type { SessionTreeView } from './SessionTreeControl'

type SessionPageHeaderProps = {
  session: SessionSummary
  cwd: string
  streaming: boolean
  runStartedAt?: string | null
  approvals: number
  failed: boolean
  stopped: boolean
  // 上下文环（ContextUsageIndicator）由会话视图按当前用量构建后传入。
  usage: ReactNode
  treeAvailable: boolean
  branches: number
  onOpenTree: (view: SessionTreeView, opener?: HTMLElement | null) => void
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

export function SessionPageHeader({
  session,
  cwd,
  streaming,
  runStartedAt,
  approvals,
  failed,
  stopped,
  usage,
  treeAvailable,
  branches,
  onOpenTree,
  onRename,
  onWorkspace,
  workspaceDisabled,
  onCompact,
  compactDisabled,
  goalAvailable,
  goalEngaged,
  goalDisabled,
  onToggleGoal,
  workflowsAvailable,
  onVoiceMode,
  notify,
  extraItems,
}: SessionPageHeaderProps) {
  const { t, language } = useI18n()
  const inspector = useContext(SessionInspectorContext)
  const elapsed = useRunElapsed(runStartedAt, streaming)
  const menu = useSessionMenu(session, {
    onRename,
    onWorkspace,
    workspaceDisabled,
    onCompact,
    compactDisabled,
    goalAvailable,
    goalEngaged,
    goalDisabled,
    onToggleGoal,
    workflowsAvailable,
    onVoiceMode,
    notify,
    extraItems,
  })
  const title = session.name || t('navigation:pageHeader.newChat')
  const project = workspaceName(cwd, language)
  // 检查器开关显示当前绑定（可在设置里改绑或禁用）；禁用时只读“检查器”。
  const inspectorBinding = useShortcutStore((state) => state.bindings.toggleInspector)
  const inspectorShortcut = formatShortcut(inspectorBinding)
  const inspectorLabel = inspectorShortcut
    ? t('board:session.inspectorShortcut', { shortcut: inspectorShortcut })
    : t('board:inspector.label')

  const status =
    approvals > 0 ? (
      <StatusTag status="attention" data-session-status="approval">
        {t('board:status.approval')}
      </StatusTag>
    ) : streaming ? (
      <StatusTag status="run" data-session-status="running">
        {elapsed === null
          ? t('board:status.running')
          : t('board:session.running', { elapsed: formatCompactDuration(elapsed) })}
      </StatusTag>
    ) : failed ? (
      <StatusTag status="attention" data-session-status="failed">
        {t('board:status.failed')}
      </StatusTag>
    ) : stopped ? (
      <StatusTag status="stopped" tone="queue" data-session-status="stopped">
        {t('board:status.stopped')}
      </StatusTag>
    ) : null

  const openSideChat = () => openSessionSideChat(session.id, inspector)

  const iconButton =
    'h-8 gap-1.5 px-2 text-[13px] font-normal text-content-secondary hover:text-content pointer-coarse:h-11'
  return (
    <header
      data-session-header
      data-window-drag-region
      className="flex h-11 min-w-0 shrink-0 items-center gap-2 border-b border-stroke-soft pr-2 pl-4 pointer-coarse:h-12"
    >
      <div className="flex min-w-0 flex-1 items-center gap-2" data-window-drag-region>
        <button
          type="button"
          className={cn(
            'group inline-flex min-w-0 shrink items-center gap-1.5 rounded-sm px-1 py-0.5 text-[15px] leading-6 font-semibold text-content hover:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus',
            TOUCH_HIT_AREA,
          )}
          title={t('chat:focusSession.renameChat')}
          aria-label={`${title} · ${t('chat:focusSession.renameChat')}`}
          onClick={onRename}
        >
          <span className="truncate" data-session-title>
            {title}
          </span>
          <Pencil
            className="size-3 flex-none opacity-0 group-hover:opacity-60 group-focus-visible:opacity-60"
            aria-hidden="true"
          />
        </button>
        {/* 运行中不能换工作区，但项目名仍是要读的信息：禁用时不降低文字对比度（规格 6.2 ≥ 4.5:1），
            只去掉悬停反馈，并在提示与描述里说明原因。 */}
        <button
          type="button"
          className={cn(
            'inline-flex h-6 max-w-[160px] min-w-0 flex-none items-center gap-1 rounded-xs border border-stroke bg-surface-muted px-1.5 font-mono text-xs text-content-secondary hover:bg-surface-hover hover:text-content disabled:cursor-default disabled:hover:bg-surface-muted disabled:hover:text-content-secondary @max-[560px]:hidden',
            TOUCH_HIT_AREA,
          )}
          title={
            workspaceDisabled
              ? `${cwd}\n${t('chat:chatPage.stopTheActiveRunBeforeChangingTheWorkspace')}`
              : cwd
          }
          aria-label={t('chat:focusSession.changeWorkingDirectoryWorkspace', {
            workspace: project,
          })}
          aria-description={
            workspaceDisabled
              ? t('chat:chatPage.stopTheActiveRunBeforeChangingTheWorkspace')
              : undefined
          }
          disabled={workspaceDisabled}
          onClick={onWorkspace}
          data-session-project
        >
          <span className="truncate">{project}</span>
        </button>
        {status}
        {/* 环旁写明“上下文”，不和相邻的“运行中 · 12s”混读成进度；窄页头只留数字与提示。 */}
        <div className="flex flex-none items-center gap-1 [&_button]:h-8">
          <span
            className="text-xs text-content-muted only:hidden @max-[720px]:hidden"
            aria-hidden="true"
          >
            {t('board:session.contextLabel')}
          </span>
          {usage}
        </div>
      </div>
      <div className="flex flex-none items-center gap-0.5">
        {treeAvailable && (
          <Button
            type="button"
            variant="ghost"
            className={iconButton}
            aria-haspopup="dialog"
            data-session-branches
            onClick={(event) => onOpenTree('conversation', event.currentTarget)}
          >
            <GitBranch aria-hidden="true" />
            <span className="@max-[720px]:hidden">
              {branches > 0
                ? t('board:session.branches', { count: branches })
                : t('board:session.branchesNone')}
            </span>
          </Button>
        )}
        <Button
          type="button"
          variant="ghost"
          className={iconButton}
          data-session-side-chat
          onClick={openSideChat}
        >
          <MessageSquare aria-hidden="true" />
          <span className="@max-[720px]:hidden">{t('board:inspector.sideChat')}</span>
        </Button>
        {treeAvailable && (
          <Button
            type="button"
            variant="ghost"
            className={iconButton}
            aria-haspopup="dialog"
            data-session-labels
            onClick={(event) => onOpenTree('labeled', event.currentTarget)}
          >
            <Tag aria-hidden="true" />
            <span className="@max-[720px]:hidden">{t('board:session.labels')}</span>
          </Button>
        )}
        <span aria-hidden="true" className="mx-1 h-4 w-px bg-stroke" />
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              id={`task-${session.id}-more`}
              className="size-8 pointer-coarse:size-11"
              aria-label={t('chat:workbench.more')}
              title={t('chat:workbench.more')}
            >
              <MoreHorizontal aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="min-w-56">
            {menu.items}
          </DropdownMenuContent>
        </DropdownMenu>
        {inspector && (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className={cn(
              'size-8 pointer-coarse:size-11',
              inspector.open && 'bg-surface-hover text-content',
            )}
            data-inspector-toggle
            aria-controls={inspector.panelId}
            aria-expanded={inspector.open}
            aria-label={inspectorLabel}
            title={inspectorLabel}
            aria-keyshortcuts={ariaKeyShortcuts(inspectorBinding)}
            onClick={inspector.toggle}
          >
            {inspector.open ? (
              <PanelRightClose aria-hidden="true" />
            ) : (
              <PanelRight aria-hidden="true" />
            )}
            {inspectorShortcut && <Kbd className="sr-only">{inspectorShortcut}</Kbd>}
          </Button>
        )}
      </div>
      {menu.dialog}
    </header>
  )
}
