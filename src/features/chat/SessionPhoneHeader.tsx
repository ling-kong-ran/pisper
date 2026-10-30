// 手机会话顶栏（规格 7.3，56px）：左侧“看板 N”（应用壳交来的看板入口，打开看板，带琥珀需要你计数）；
// 中间是标题（点击改名），下一行是项目标签、运行状态（◐ 运行中 12s / 等待批准 / 失败 / 已停止）
// 与“+N 改动 ›”芯片（有改动才显示，点开检查器底部表单）；右侧“⋯”放分支、侧聊、标签、检查器，
// 以及与桌面页头相同的会话操作（重命名、移动工作区、压缩上下文、归档等）。
// 改动数与检查器“改动”标签读同一个查询缓存，不另开轮询。
import { useContext, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import { ChevronRight, FileDiff, GitBranch, MessageSquare, MoreHorizontal, Tag } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { formatCompactDuration, workspaceName } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { SessionSummary } from '@/types/chat'
import { getSessionChangeSummary } from './session-change-summary-api'
import {
  openSessionSideChat,
  useRunElapsed,
  useSessionMenu,
  type SessionMenuActions,
} from './session-header-menu'
import { SessionInspectorContext } from './session-workspace-context'
import type { SessionTreeView } from './SessionTreeControl'

type SessionPhoneHeaderProps = SessionMenuActions & {
  leading: ReactNode
  session: SessionSummary
  cwd: string
  streaming: boolean
  runStartedAt?: string | null
  approvals: number
  failed: boolean
  stopped: boolean
  treeAvailable: boolean
  branches: number
  onOpenTree: (view: SessionTreeView, opener?: HTMLElement | null) => void
}

// 运行状态：只有一种成立时才显示；颜色只用于对应状态，琥珀只表示“等待批准”。
type PhoneRunStatus = { kind: 'approval' | 'running' | 'failed' | 'stopped'; label: string }

export function SessionPhoneHeader({
  leading,
  session,
  cwd,
  streaming,
  runStartedAt,
  approvals,
  failed,
  stopped,
  treeAvailable,
  branches,
  onOpenTree,
  ...actions
}: SessionPhoneHeaderProps) {
  const { t, language } = useI18n()
  const inspector = useContext(SessionInspectorContext)
  const elapsed = useRunElapsed(runStartedAt, streaming)
  const menu = useSessionMenu(session, actions)
  const title = session.name || t('navigation:pageHeader.newChat')
  const project = workspaceName(cwd, language)
  // 与检查器“改动”标签同一个查询键：打开表单时直接复用缓存，运行结束（streaming 翻转）后重新读取。
  const changeSummary = useQuery({
    queryKey: ['session-change-summary', session.id, streaming],
    queryFn: () => getSessionChangeSummary(session.id),
    staleTime: 10_000,
  })
  const changedFiles =
    changeSummary.data?.status === 'known' ? (changeSummary.data.changedFiles ?? 0) : 0

  const status: PhoneRunStatus | null =
    approvals > 0
      ? { kind: 'approval', label: t('board:status.approval') }
      : streaming
        ? {
            kind: 'running',
            label:
              elapsed === null
                ? t('board:status.running')
                : t('board:phone.running', { elapsed: formatCompactDuration(elapsed) }),
          }
        : failed
          ? { kind: 'failed', label: t('board:status.failed') }
          : stopped
            ? { kind: 'stopped', label: t('board:status.stopped') }
            : null

  const showChanges = () => inspector?.show('changes')

  return (
    <header
      data-session-header="phone"
      className="flex h-14 min-w-0 shrink-0 items-center gap-1 bg-chrome px-1"
    >
      <div className="flex flex-none items-center">{leading}</div>
      {/* 触控目标 ≥44px：标题按钮的伪元素铺满整个中间区（56px 高），项目与状态行只是说明文字；
          改动芯片叠在上面（z-10），自己的命中区补到 44px 高，仍留在副标题行里。 */}
      <div className="relative flex min-w-0 flex-1 flex-col items-center justify-center self-stretch">
        <button
          type="button"
          className="inline-flex h-7 max-w-full min-w-0 items-end rounded-sm px-1.5 text-[15px] leading-5 font-semibold text-content outline-none after:absolute after:inset-0 after:rounded-md after:content-[''] focus-visible:after:outline-2 focus-visible:after:-outline-offset-2 focus-visible:after:outline-focus"
          title={t('chat:focusSession.renameChat')}
          aria-label={`${title} · ${t('chat:focusSession.renameChat')}`}
          onClick={actions.onRename}
        >
          <span className="truncate" data-session-title>
            {title}
          </span>
        </button>
        <div className="flex h-7 max-w-full min-w-0 items-center gap-1.5 text-xs text-content-muted">
          <span
            className="min-w-0 truncate rounded-xs border border-stroke bg-surface-muted px-1 font-mono leading-4 text-content-secondary"
            title={cwd}
            data-session-project
          >
            {project}
          </span>
          {status && (
            <span
              className={cn(
                'flex flex-none items-center gap-1',
                status.kind === 'approval' && 'font-medium text-st-attn',
                status.kind === 'running' && 'text-st-run',
                // 失败在等人处理，归需要你（规格 v2 1.3），与批准同用琥珀。
                status.kind === 'failed' && 'font-medium text-st-attn',
                status.kind === 'stopped' && 'text-content-muted',
              )}
              data-session-status={status.kind}
            >
              <StatusGlyph
                status={
                  status.kind === 'running'
                    ? 'run'
                    : status.kind === 'stopped'
                      ? 'stopped'
                      : 'attention'
                }
                className="size-3.5"
              />
              <span className="tabular-nums">{status.label}</span>
            </span>
          )}
          {changedFiles > 0 && (
            <button
              type="button"
              data-session-changes={changedFiles}
              aria-haspopup="dialog"
              aria-controls={inspector?.panelId}
              aria-label={t('board:phone.changesLabel', { count: changedFiles })}
              // 芯片视觉 22px；命中区向上补 22px、向下补到顶栏底边，合计约 47px（≥44），左右各补 6px。
              className="relative z-10 flex h-[22px] flex-none items-center gap-0.5 rounded-full border border-stroke bg-panel pr-1 pl-2 font-medium text-content-secondary after:absolute after:-inset-x-1.5 after:-top-[22px] after:-bottom-[3px] after:content-[''] focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus active:bg-surface-hover"
              onClick={showChanges}
            >
              {t('board:phone.changes', { count: changedFiles })}
              <ChevronRight className="size-3.5" aria-hidden="true" />
            </button>
          )}
        </div>
      </div>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            id={`task-${session.id}-more`}
            className="size-11 flex-none"
            aria-label={t('chat:workbench.more')}
            title={t('chat:workbench.more')}
          >
            <MoreHorizontal aria-hidden="true" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-60">
          {treeAvailable && (
            <DropdownMenuItem onSelect={() => onOpenTree('branches')} data-session-branches>
              <GitBranch />
              {branches > 0
                ? t('board:session.branches', { count: branches })
                : t('board:session.branchesNone')}
            </DropdownMenuItem>
          )}
          <DropdownMenuItem
            onSelect={() => openSessionSideChat(session.id, inspector)}
            data-session-side-chat
          >
            <MessageSquare />
            {t('board:inspector.sideChat')}
          </DropdownMenuItem>
          {treeAvailable && (
            <DropdownMenuItem onSelect={() => onOpenTree('labeled')} data-session-labels>
              <Tag />
              {t('board:session.labels')}
            </DropdownMenuItem>
          )}
          {inspector && (
            // 没有改动时顶栏不显示芯片，终端、文件与预览仍从这里进入。
            <DropdownMenuItem onSelect={showChanges} data-inspector-toggle>
              <FileDiff />
              {t('board:phone.openInspector')}
            </DropdownMenuItem>
          )}
          <DropdownMenuSeparator />
          {menu.items}
        </DropdownMenuContent>
      </DropdownMenu>
      {menu.dialog}
    </header>
  )
}
