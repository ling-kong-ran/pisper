// 非路由的旧单会话视图顶栏（48px）：会话页（路由）一律用 SessionPageHeader / SessionPhoneHeader。
// 只剩回放冒烟的夹具在渲染没有页面外壳的 FocusSession；A4 删除了分屏菜单与“关闭窗格”（Dockview 已删），
// 整个组件在 A6 重做浮层时随夹具一起退役。
import { useContext, type ComponentProps, type ReactNode } from 'react'
import { ListFilter, MoreHorizontal, Plus, Pencil, FolderOpen } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { cn } from '@/lib/utils'
import { useContextPagesStore } from './session-context-pages-store'
import { ChatDockContext } from './chat-dock-context'

export function TaskWorkspaceHeader({
  sessionId,
  notify,
  onRename,
  onWorkspace,
  actions,
  children,
  className,
  ...props
}: ComponentProps<'header'> & {
  sessionId: string
  notify?: Notify
  onRename: () => void
  onWorkspace: () => void
  actions?: ReactNode
}) {
  const { t } = useI18n()
  const context = useContext(ChatDockContext)
  const summaryOpen = useContextPagesStore(
    (store) => store.sessions[sessionId]?.summaryOpen || false,
  )
  const openNewContentTab = () => {
    const result = useContextPagesStore
      .getState()
      .update(sessionId, 'new', { type: 'add', kind: 'new' })
    if (result === 'limit') notify?.(t('chat:sessionContext.pageLimit'), 'info')
  }
  // 手机标签条已有“新会话”加号；新建内容页收进更多菜单，避免同一屏出现两个含义不同的加号。
  const compactActions = Boolean(context?.mobileLayout)
  // 顶栏几何由本组件持有：固定 48px、底部 1px 细线，放在调用方类名之后收敛旧高度与无边框写法，
  // 与 TaskWorkspace 的会话区让位（conversationHeaderInset）保持同一数值。手机外壳没有分栏让位，
  // 改用设计稿的 52px 单行顶栏，与浮动菜单键共用同一条垂直中线。
  const actionClass = 'size-7 shrink-0 max-[650px]:size-11 pointer-coarse:size-11'
  // 手机顶栏按钮与左侧菜单键同为 22px 图标；桌面与窄窗口保持 16px。
  const actionIconClass = 'size-4 [[data-phone-shell]_&]:size-[22px]'
  return (
    <header
      {...props}
      className={cn(
        'flex min-w-0 shrink-0 items-center gap-1',
        className,
        'h-12 border-b border-stroke-soft py-0 [[data-phone-shell]_&]:h-[52px]',
      )}
    >
      {children}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            id={`task-${sessionId}-more`}
            className={actionClass}
            aria-label={t('chat:workbench.more')}
            title={t('chat:workbench.more')}
          >
            <MoreHorizontal className={actionIconClass} />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="end"
          className="min-w-56 max-[650px]:[&_[data-slot=dropdown-menu-item]]:min-h-11"
        >
          <DropdownMenuItem onSelect={onRename}>
            <Pencil />
            {t('chat:focusSession.renameChat')}
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={onWorkspace}>
            <FolderOpen />
            {t('chat:workbench.workspace')}
          </DropdownMenuItem>
          {compactActions && (
            <DropdownMenuItem onSelect={openNewContentTab}>
              <Plus />
              {t('chat:workbench.newTab')}
            </DropdownMenuItem>
          )}
          {actions && <DropdownMenuSeparator />}
          {actions}
        </DropdownMenuContent>
      </DropdownMenu>
      <Button
        variant="ghost"
        className={actionClass}
        id={`task-${sessionId}-summary-toggle`}
        aria-label={t('chat:workbench.summary')}
        title={t('chat:workbench.summary')}
        aria-expanded={summaryOpen}
        aria-controls={`task-${sessionId}-summary`}
        onClick={() =>
          useContextPagesStore
            .getState()
            .update(sessionId, 'new', { type: 'workspace', summaryOpen: !summaryOpen })
        }
      >
        <ListFilter className={actionIconClass} />
      </Button>
      {!compactActions && (
        <Button
          variant="ghost"
          className={actionClass}
          id={`task-${sessionId}-new-content-tab`}
          aria-label={t('chat:workbench.newTab')}
          title={t('chat:workbench.newTab')}
          onClick={openNewContentTab}
        >
          <Plus size={16} />
        </Button>
      )}
    </header>
  )
}
