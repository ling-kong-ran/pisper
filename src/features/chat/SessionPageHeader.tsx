// 会话顶栏（规格 v2 第 4.1 节，52px）：左边是标题 ▾（标题菜单收纳会话的其余操作）、项目标签（等宽浅色）
// 与 ⑂ 会话树按钮（有分支时写数量）；右边只有改动按钮“N 个文件 +x −y”，它同时是检查器开关（没有改动时只剩图标）。
// 第 1–4 期页头上的状态胶囊、上下文环、分支与标签两个按钮、侧聊按钮和“⋯”都并进了标题菜单或侧栏：
// 运行状态在侧栏会话行与输入框托盘里，上下文用量写在“压缩上下文”一项上，分支与标记在会话树对话框的页签里。
// Windows 自绘标题栏时右侧再让出 144px 给窗口按钮，顶栏空白处是拖拽区（交互元素由桌面壳排除）。
// 触屏（pointer: coarse）：按钮抬到 44px，标题与项目标签保持可见尺寸，命中区用伪元素补到 44px。
import { useContext } from 'react'
import { useQuery } from '@tanstack/react-query'
import { ChevronDown, FileDiff, GitFork, PanelRight } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { workspaceName } from '@/lib/format'
import { ariaKeyShortcuts } from '@/lib/shortcut-aria'
import { formatShortcut } from '@/lib/shortcuts'
import { TOUCH_HIT_AREA } from '@/lib/touch-target'
import { cn } from '@/lib/utils'
import { useShortcutStore } from '@/stores/shortcut-store'
import type { SessionSummary } from '@/types/chat'
import { getSessionChangeSummary } from './session-change-summary-api'
import { openSessionSideChat, useSessionMenu, type SessionMenuActions } from './session-header-menu'
import { SessionInspectorContext } from './session-workspace-context'
import type { SessionTreeView } from './SessionTreeControl'

type SessionPageHeaderProps = Omit<SessionMenuActions, 'onSideChat' | 'extraItems'> & {
  session: SessionSummary
  cwd: string
  streaming: boolean
  treeAvailable: boolean
  branches: number
  onOpenTree: (view: SessionTreeView, opener?: HTMLElement | null) => void
}

export function SessionPageHeader({
  session,
  cwd,
  streaming,
  treeAvailable,
  branches,
  onOpenTree,
  ...actions
}: SessionPageHeaderProps) {
  const { t, language } = useI18n()
  const inspector = useContext(SessionInspectorContext)
  const menu = useSessionMenu(session, {
    ...actions,
    onSideChat: () => openSessionSideChat(session.id, inspector),
  })
  const title = session.name || t('navigation:pageHeader.newChat')
  const project = workspaceName(cwd, language)
  // 改动数与检查器“改动”标签、手机顶栏读同一个查询键：运行结束（streaming 翻转）后重新读取，不另开轮询。
  const changeSummary = useQuery({
    queryKey: ['session-change-summary', session.id, streaming],
    queryFn: () => getSessionChangeSummary(session.id),
    staleTime: 10_000,
  })
  const summary = changeSummary.data?.status === 'known' ? changeSummary.data : null
  const changedFiles = summary?.changedFiles ?? 0
  // 检查器开关显示当前绑定（可在设置里改绑或禁用）；会话树同理（⇧⌘B）。
  const inspectorBinding = useShortcutStore((state) => state.bindings.toggleInspector)
  const treeBinding = useShortcutStore((state) => state.bindings.sessionTree)
  const inspectorShortcut = formatShortcut(inspectorBinding)
  const treeShortcut = formatShortcut(treeBinding)
  const changesText =
    changedFiles > 0
      ? t('board:session.changes', {
          count: changedFiles,
          added: summary?.added ?? 0,
          removed: summary?.removed ?? 0,
        })
      : null
  const inspectorName = changesText
    ? `${t('board:inspector.label')} · ${changesText}`
    : t('board:inspector.label')
  const inspectorLabel = inspectorShortcut
    ? t('board:session.withShortcut', { label: inspectorName, shortcut: inspectorShortcut })
    : inspectorName
  const treeName =
    branches > 0 ? t('board:session.treeBranches', { count: branches }) : t('board:session.tree')
  const treeLabel = treeShortcut
    ? t('board:session.withShortcut', { label: treeName, shortcut: treeShortcut })
    : treeName

  return (
    <header
      data-session-header
      data-focus-region="header"
      data-window-drag-region
      className="flex h-[52px] min-w-0 shrink-0 items-center gap-2 pr-3 pl-4 [[data-custom-titlebar]_&]:pr-[144px]"
    >
      <div className="flex min-w-0 flex-1 items-center gap-1.5" data-window-drag-region>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            {/* 标题本身就是菜单入口（规格 4.1“标题 ▾”）；id 沿用旧“⋯”的 task-<id>-more，
                底部表单与检查器关闭后的焦点退路仍指向它。 */}
            <button
              type="button"
              id={`task-${session.id}-more`}
              data-session-menu
              className={cn(
                'group inline-flex h-8 min-w-0 shrink items-center gap-1 rounded-sm px-2 text-[15px] leading-6 font-medium text-content hover:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus data-[state=open]:bg-surface-hover pointer-coarse:h-11',
                TOUCH_HIT_AREA,
              )}
              aria-label={t('board:session.titleMenu', { title })}
            >
              <span className="truncate" data-session-title>
                {title}
              </span>
              <ChevronDown className="size-3.5 flex-none text-content-muted" aria-hidden="true" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="min-w-60">
            {menu.items}
          </DropdownMenuContent>
        </DropdownMenu>
        {/* 项目名是要读的信息（对比度 ≥4.5），不是控件：换工作区在标题菜单里。 */}
        <span
          className="min-w-0 max-w-[160px] flex-none truncate font-mono text-xs text-content-muted @max-[560px]:hidden"
          title={cwd}
          data-session-project
        >
          {project}
        </span>
        {treeAvailable && (
          <Button
            type="button"
            variant="ghost"
            className={cn(
              'h-7 min-w-7 flex-none gap-1 px-1.5 font-mono text-xs font-normal text-content-muted tabular-nums hover:text-content pointer-coarse:h-11 pointer-coarse:min-w-11',
            )}
            aria-haspopup="dialog"
            aria-label={treeLabel}
            title={treeLabel}
            aria-keyshortcuts={ariaKeyShortcuts(treeBinding)}
            data-session-branches={branches}
            onClick={(event) => onOpenTree('branches', event.currentTarget)}
          >
            <GitFork className="size-3.5" aria-hidden="true" />
            {branches > 0 && <span aria-hidden="true">{branches}</span>}
          </Button>
        )}
      </div>
      {inspector && (
        <Button
          type="button"
          variant="ghost"
          className={cn(
            'h-8 flex-none gap-1.5 px-2 text-[13px] font-normal text-content-secondary hover:text-content pointer-coarse:h-11',
            changesText ? '' : 'w-8 px-0 pointer-coarse:w-11',
            inspector.open && 'bg-surface-hover text-content ring-1 ring-stroke ring-inset',
          )}
          data-inspector-toggle
          data-session-changes={changedFiles}
          aria-controls={inspector.panelId}
          aria-expanded={inspector.open}
          aria-label={inspectorLabel}
          title={inspectorLabel}
          aria-keyshortcuts={ariaKeyShortcuts(inspectorBinding)}
          onClick={inspector.toggle}
        >
          {changesText && summary ? (
            <>
              <FileDiff className="size-3.5" aria-hidden="true" />
              <span aria-hidden="true">
                {t('board:session.changedFiles', { count: changedFiles })}
              </span>
              {/* 红绿只用于 diff 增删（规格 6.1）；数字用等宽、表格数字。 */}
              <span className="font-mono text-xs text-success tabular-nums" aria-hidden="true">
                +{summary.added ?? 0}
              </span>
              <span className="font-mono text-xs text-danger tabular-nums" aria-hidden="true">
                −{summary.removed ?? 0}
              </span>
            </>
          ) : (
            <PanelRight aria-hidden="true" />
          )}
        </Button>
      )}
      {menu.dialog}
    </header>
  )
}
