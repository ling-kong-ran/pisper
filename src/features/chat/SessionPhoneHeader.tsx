// 手机会话顶栏（规格 v2 第 7.1、7.2 节，56px）：左 ☰（应用壳交来，带需要你计数，点开抽屉）；
// 中间“标题 ▾”，下一行“项目 · ◠ 运行中 3:12”（或等待批准 / 失败 / 已停止）；
// 右边有改动时是改动芯片（文件图标 + N，打开检查器底部表单），没有改动时是 ✎ 新任务（打开派发底部表单）。
// 标题 ▾ 打开会话菜单底部表单：会话树（N 个分支）· 侧聊 · 检查器 | 重命名 · 归档 | 移动工作区 · 压缩上下文
// （带用量）· 工作流记录 · 语音对话 | 归档——与桌面标题菜单同一份数据（useSessionMenu 的 entries）。
// 改动数与检查器“改动”标签读同一个查询缓存，不另开轮询。
import { useContext, useId, useState, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import { ChevronDown, FileDiff, GitFork, MessageSquare, SquarePen } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { SheetRow, SheetTile, SheetTileGrid } from '@/components/app/SheetActions'
import { SheetDragHandle } from '@/components/app/SheetDragHandle'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { formatCompactDuration, workspaceName } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { EntityRecord, SessionSummary } from '@/types/chat'
import { getSessionChangeSummary } from './session-change-summary-api'
import {
  openSessionSideChat,
  useRunElapsed,
  useSessionMenu,
  type SessionMenuActions,
  type SessionMenuEntry,
} from './session-header-menu'
import { SessionInspectorContext, SessionPageChromeContext } from './session-workspace-context'
import type { SessionTreeView } from './SessionTreeControl'
import { machineProjectLabel } from './session-workspaces'
import { SessionRunningChip } from './SessionRunningChip'

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
  runningActivity?: readonly EntityRecord[]
}

// 运行状态：只有一种成立时才显示；琥珀只表示需要你（等待批准、失败待处理）。
type PhoneRunStatus = { kind: 'approval' | 'running' | 'failed' | 'stopped'; label: string }

// 顶栏图标键：44 的浅灰圆底（手机参考稿的做法），和输入框里的 + 键同一种无边框填充样式。
const ICON_BUTTON =
  'relative grid size-11 flex-none place-items-center rounded-full bg-surface-muted text-content focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus active:bg-surface-hover dark:bg-surface-active'

export function SessionPhoneHeader({
  leading,
  session,
  cwd: sessionCwd,
  streaming,
  runStartedAt,
  approvals,
  failed,
  stopped,
  treeAvailable,
  branches,
  onOpenTree,
  runningActivity = [],
  ...actions
}: SessionPhoneHeaderProps) {
  // 远程机器上的会话：cwd 只是本机占位目录，这里显示机器和对方的目录。
  const cwd = session.machine ? `${session.machine.name}:${session.machine.cwd}` : sessionCwd
  const { t, language } = useI18n()
  const inspector = useContext(SessionInspectorContext)
  const chrome = useContext(SessionPageChromeContext)
  const elapsed = useRunElapsed(runStartedAt, streaming)
  const menu = useSessionMenu(session, actions)
  const [menuOpen, setMenuOpen] = useState(false)
  const menuId = useId()
  const title = session.name || t('navigation:pageHeader.newChat')
  // 无项目会话的临时文件夹名没有意义，标题栏显示“无项目”。
  const project = workspaceName(session.projectless ? '' : cwd, language)
  const projectLabel = session.machine ? machineProjectLabel(session.machine) : project
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

  // 会话菜单的手机专属行在前：会话树、侧聊与检查器（没有改动时顶栏不显示芯片，终端、文件与预览从这里进）。
  const phoneEntries: SessionMenuEntry[] = [
    ...(treeAvailable
      ? [
          {
            key: 'tree',
            icon: GitFork,
            label:
              branches > 0
                ? t('board:session.treeBranches', { count: branches })
                : t('board:session.tree'),
            shortLabel: t('board:session.tree'),
            onSelect: () => onOpenTree('branches'),
            marker: 'session-tree',
          },
        ]
      : []),
    {
      key: 'side-chat',
      icon: MessageSquare,
      label: t('board:inspector.sideChat'),
      onSelect: () => openSessionSideChat(session.id, inspector),
      marker: 'session-side-chat',
    },
    ...(inspector
      ? [
          {
            key: 'inspector',
            icon: FileDiff,
            label: t('board:phone.openInspector'),
            shortLabel: t('board:phone.openInspectorShort'),
            onSelect: showChanges,
            marker: 'inspector-toggle',
          },
        ]
      : []),
  ]
  return (
    <header
      data-session-header="phone"
      className="grid h-14 min-w-0 shrink-0 grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center bg-panel px-3"
    >
      <div className="flex min-w-0 items-center">{leading}</div>
      <button
        type="button"
        data-session-menu-trigger
        aria-haspopup="dialog"
        aria-expanded={menuOpen}
        aria-controls={menuOpen ? menuId : undefined}
        aria-label={t('board:session.titleMenu', { title })}
        className="flex h-11 max-w-[min(58vw,260px)] min-w-0 flex-col items-center justify-center rounded-[var(--r-md)] px-2 outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus active:bg-surface-hover"
        onClick={() => setMenuOpen(true)}
      >
        <span className="flex max-w-full min-w-0 items-center gap-0.5 text-base leading-5 font-semibold text-content">
          <span className="truncate" data-session-title>
            {title}
          </span>
          <ChevronDown className="size-4 flex-none text-content-muted" aria-hidden="true" />
        </span>
        <span className="flex max-w-full min-w-0 items-center gap-1 text-xs leading-4 text-content-muted">
          <span className="min-w-0 truncate font-mono" title={cwd} data-session-project>
            {projectLabel}
          </span>
          {status && (
            <>
              <span aria-hidden="true">·</span>
              <span
                className={cn(
                  'flex flex-none items-center gap-1',
                  (status.kind === 'approval' || status.kind === 'failed') &&
                    'font-medium text-warning-strong',
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
                  className="size-3"
                />
                <span className="tabular-nums">{status.label}</span>
              </span>
            </>
          )}
        </span>
      </button>
      <div className="flex min-w-0 items-center justify-end gap-2">
        <SessionRunningChip
          sessionId={session.id}
          streaming={streaming}
          activity={runningActivity}
          phoneButtonClass={ICON_BUTTON}
        />
        {changedFiles > 0 && inspector ? (
          <button
            type="button"
            data-session-changes={changedFiles}
            aria-haspopup="dialog"
            aria-controls={inspector.panelId}
            aria-label={t('board:phone.changesLabel', { count: changedFiles })}
            className={ICON_BUTTON}
            onClick={showChanges}
          >
            <FileDiff className="size-5" strokeWidth={1.75} aria-hidden="true" />
            <span
              aria-hidden="true"
              className="absolute top-1.5 right-1 grid h-4 min-w-4 place-items-center rounded-full bg-content px-1 text-[10px] leading-none font-semibold text-panel tabular-nums"
            >
              {changedFiles > 99 ? '99+' : changedFiles}
            </span>
          </button>
        ) : chrome?.phone ? (
          <button
            type="button"
            data-phone-new-task
            aria-label={t('navigation:shell.newTask')}
            className={ICON_BUTTON}
            onClick={chrome.phone.onNewTask}
          >
            <SquarePen className="size-5" strokeWidth={1.75} aria-hidden="true" />
          </button>
        ) : null}
      </div>
      <Sheet open={menuOpen} onOpenChange={setMenuOpen}>
        <SheetContent
          id={menuId}
          side="bottom"
          showCloseButton={false}
          data-session-menu-sheet
          className="max-h-[85dvh] gap-0 rounded-t-(--r-sheet) px-2 pt-1 pb-[calc(8px+var(--pisper-safe-area-bottom))]"
        >
          <SheetDragHandle onDismiss={() => setMenuOpen(false)} />
          <SheetTitle className="truncate px-3 pt-1 pb-0.5 text-base font-semibold">
            {title}
          </SheetTitle>
          <SheetDescription className="truncate px-3 pb-2 font-mono text-xs text-content-muted">
            {project}
          </SheetDescription>
          <SheetTileGrid count={phoneEntries.length}>
            {phoneEntries.map((entry) => {
              const marker = entry.marker ? { [`data-${entry.marker}`]: true } : {}
              return (
                <SheetTile
                  key={entry.key}
                  icon={entry.icon}
                  label={entry.shortLabel || entry.label}
                  aria-label={entry.label}
                  onClick={() => {
                    setMenuOpen(false)
                    entry.onSelect()
                  }}
                  {...marker}
                />
              )
            })}
          </SheetTileGrid>
          <div className="min-h-0 overflow-y-auto" role="list">
            {menu.entries.map((entry) => {
              const marker = entry.marker ? { [`data-${entry.marker}`]: true } : {}
              return (
                <div role="listitem" key={entry.key}>
                  {entry.separator && (
                    <div aria-hidden="true" className="mx-3 my-1 h-px bg-stroke-soft" />
                  )}
                  <SheetRow
                    icon={entry.icon}
                    label={entry.label}
                    disabled={entry.disabled}
                    trailing={
                      entry.trailing && (
                        <span
                          className="flex-none font-mono text-sm text-content-muted tabular-nums"
                          title={entry.trailingTitle}
                          data-session-context-usage
                        >
                          {entry.trailing}
                        </span>
                      )
                    }
                    onClick={() => {
                      setMenuOpen(false)
                      entry.onSelect()
                    }}
                    {...marker}
                  />
                </div>
              )
            })}
          </div>
        </SheetContent>
      </Sheet>
      {menu.dialog}
    </header>
  )
}
