import { useEffect, useRef, useState } from 'react'
import {
  FolderOpen,
  MoreHorizontal,
  PanelBottom,
  PanelLeft,
  PanelRight,
  PanelTop,
  Pencil,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { workspaceName } from '@/lib/format'
import type { SessionSummary } from '@/types/chat'
import { useViewportMenuOffset } from './use-viewport-menu-offset'

export function SessionActionsMenu({
  session,
  canSplit,
  streaming,
  switchingCwd,
  onSplitLeft,
  onSplitRight,
  onSplitTop,
  onSplitBottom,
  onClosePanel,
  onWorkspace,
  onRename,
}: {
  session: SessionSummary
  canSplit?: boolean
  streaming?: boolean
  switchingCwd?: boolean
  onSplitLeft: () => void
  onSplitRight: () => void
  onSplitTop: () => void
  onSplitBottom: () => void
  onClosePanel: () => void
  onWorkspace: () => void
  onRename: () => void
}) {
  const { t, language } = useI18n()
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)

  useViewportMenuOffset(open, menuRef)

  useEffect(() => {
    if (!open) return undefined
    const close = (event: MouseEvent) => {
      const target = event.target instanceof Node ? event.target : null
      if (!rootRef.current?.contains(target)) setOpen(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('keydown', escape)
    }
  }, [open])

  const run = (action?: () => void) => {
    setOpen(false)
    action?.()
  }
  const splitActions = [
    [
      PanelLeft,
      t('chat:focusSession.splitToLeft'),
      t('chat:focusSession.moveTheCurrentTabIntoANewGroupOnTheLeft'),
      onSplitLeft,
    ],
    [
      PanelRight,
      t('chat:focusSession.splitToRight'),
      t('chat:focusSession.moveTheCurrentTabIntoANewGroupOnTheRight'),
      onSplitRight,
    ],
    [
      PanelTop,
      t('chat:focusSession.splitToTop'),
      t('chat:focusSession.moveTheCurrentTabIntoANewGroupOnTheTop'),
      onSplitTop,
    ],
    [
      PanelBottom,
      t('chat:focusSession.splitToBottom'),
      t('chat:focusSession.moveTheCurrentTabIntoANewGroupOnTheBottom'),
      onSplitBottom,
    ],
  ] as const

  return (
    <div ref={rootRef} className="session-actions-menu-root">
      <button
        type="button"
        className="icon-button"
        title={t('chat:focusSession.chatActions')}
        aria-label={t('chat:focusSession.openChatActionsMenu')}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={!session}
        onClick={() => setOpen((visible) => !visible)}
      >
        <MoreHorizontal size={17} />
      </button>
      {open && (
        <div ref={menuRef} className="permission-mode-menu session-actions-menu" role="menu">
          {splitActions.map(([Icon, label, description, action]) => (
            <button
              type="button"
              role="menuitem"
              disabled={!canSplit}
              onClick={() => run(action)}
              key={label}
            >
              <Icon size={15} />
              <span>
                <strong>{label}</strong>
                <small>
                  {canSplit ? description : t('chat:focusSession.thisGroupHasOnlyOneChat')}
                </small>
              </span>
            </button>
          ))}
          <button
            type="button"
            role="menuitem"
            disabled={streaming || switchingCwd}
            onClick={() => run(onWorkspace)}
          >
            <FolderOpen size={15} />
            <span>
              <strong>{t('chat:focusSession.setWorkingDirectory')}</strong>
              <small>
                {streaming
                  ? t('chat:focusSession.cannotSwitchWhileTheAgentIsRunning')
                  : workspaceName(session?.cwd, language)}
              </small>
            </span>
          </button>
          <button type="button" role="menuitem" onClick={() => run(onRename)}>
            <Pencil size={15} />
            <span>
              <strong>{t('chat:focusSession.renameChat')}</strong>
              <small>{session?.name || t('chat:focusSession.newChat')}</small>
            </span>
          </button>
          <button type="button" role="menuitem" onClick={() => run(onClosePanel)}>
            <X size={15} />
            <span>
              <strong>{t('chat:focusSession.closeTab')}</strong>
              <small>{t('chat:focusSession.keepTheHistoryAndCloseOnlyThisView')}</small>
            </span>
          </button>
        </div>
      )}
    </div>
  )
}
