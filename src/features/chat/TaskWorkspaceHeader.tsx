import { useContext, useRef, type ComponentProps, type ReactNode } from 'react'
import {
  ListFilter,
  MoreHorizontal,
  Plus,
  Pencil,
  FolderOpen,
  X,
  PanelLeft,
  PanelRight,
  PanelTop,
  PanelBottom,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { cn } from '@/lib/utils'
import { useContextPagesStore } from './session-context-pages-store'
import { ChatDockContext } from './chat-dock-context'
import { requestSessionSelection } from './events'
import type { SessionSplitDisposition } from './dock-layout'

export function TaskWorkspaceHeader({
  sessionId,
  notify,
  onRename,
  onWorkspace,
  onClose,
  actions,
  children,
  className,
  ...props
}: ComponentProps<'header'> & {
  sessionId: string
  notify?: Notify
  onRename: () => void
  onWorkspace: () => void
  onClose?: () => void
  actions?: ReactNode
}) {
  const { t } = useI18n()
  const context = useContext(ChatDockContext)
  const summaryOpen = useContextPagesStore(
    (store) => store.sessions[sessionId]?.summaryOpen || false,
  )
  const splitTarget = useRef('')
  const splitDirections: Array<[SessionSplitDisposition, string, typeof PanelLeft]> = [
    ['left', t('chat:focusSession.splitToLeft'), PanelLeft],
    ['right', t('chat:focusSession.splitToRight'), PanelRight],
    ['above', t('chat:focusSession.splitToTop'), PanelTop],
    ['below', t('chat:focusSession.splitToBottom'), PanelBottom],
  ]
  const others = context?.sessions.filter((session) => session.id !== sessionId) || []
  return (
    <header {...props} className={cn('flex min-w-0 shrink-0 items-center gap-1', className)}>
      {children}
      <DropdownMenu
        onOpenChange={(open) => {
          if (open) splitTarget.current = ''
        }}
      >
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            id={`task-${sessionId}-more`}
            className="size-9 shrink-0 max-[650px]:size-11"
            aria-label={t('chat:workbench.more')}
            title={t('chat:workbench.more')}
          >
            <MoreHorizontal size={16} />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="end"
          className="min-w-56 rounded-2xl p-1.5 [&_[data-slot=dropdown-menu-item]]:min-h-9 [&_[data-slot=dropdown-menu-item]]:font-normal max-[650px]:[&_[data-slot=dropdown-menu-item]]:min-h-11"
          onCloseAutoFocus={(event) => {
            if (!splitTarget.current) return
            event.preventDefault()
            const target = splitTarget.current
            requestAnimationFrame(() =>
              document.getElementById(`task-${target}-workspace`)?.focus(),
            )
          }}
        >
          <DropdownMenuItem onSelect={onRename}>
            <Pencil />
            {t('chat:focusSession.renameChat')}
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={onWorkspace}>
            <FolderOpen />
            {t('chat:workbench.workspace')}
          </DropdownMenuItem>
          {actions && <DropdownMenuSeparator />}
          {actions}
          {!context?.mobileLayout && others.length > 0 && (
            <>
              <DropdownMenuSeparator />
              {splitDirections.map(([direction, label, Icon]) => (
                <DropdownMenuSub key={direction}>
                  <DropdownMenuSubTrigger>
                    <Icon />
                    {label}
                  </DropdownMenuSubTrigger>
                  <DropdownMenuSubContent>
                    {others.map((session) => (
                      <DropdownMenuItem
                        key={session.id}
                        onSelect={() => {
                          splitTarget.current = session.id
                          requestSessionSelection(session.id, direction)
                        }}
                      >
                        {session.name || t('chat:chatPage.untitledChat')}
                      </DropdownMenuItem>
                    ))}
                  </DropdownMenuSubContent>
                </DropdownMenuSub>
              ))}
            </>
          )}
          {onClose && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={onClose}>
                <X />
                {t('chat:workbench.closePane')}
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      <Button
        variant="ghost"
        className="size-9 shrink-0 max-[650px]:size-11"
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
        <ListFilter size={16} />
      </Button>
      <Button
        variant="ghost"
        className="size-9 shrink-0 max-[650px]:size-11"
        id={`task-${sessionId}-new-content-tab`}
        aria-label={t('chat:workbench.newTab')}
        title={t('chat:workbench.newTab')}
        onClick={() => {
          const result = useContextPagesStore
            .getState()
            .update(sessionId, 'new', { type: 'add', kind: 'new' })
          if (result === 'limit') notify?.(t('chat:sessionContext.pageLimit'), 'info')
        }}
      >
        <Plus size={16} />
      </Button>
    </header>
  )
}
