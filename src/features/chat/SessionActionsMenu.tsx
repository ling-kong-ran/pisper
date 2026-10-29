// 会话操作菜单：打开目录/导出/归档/删除等会话级动作。
import { useCallback, useEffect, useRef, useState } from 'react'
import {
  FolderOpen,
  MoreHorizontal,
  PanelBottom,
  PanelLeft,
  PanelRight,
  PanelTop,
  Pencil,
  TreePine,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { workspaceName } from '@/lib/format'
import type { SessionSummary } from '@/types/chat'
import { AnchoredPopupMenu } from './AnchoredPopupMenu'

import { Button } from '@/components/ui/button'

export function SessionActionsMenu({
  session,
  canSplit,
  canClose = true,
  streaming,
  switchingCwd,
  onSplitLeft,
  onSplitRight,
  onSplitTop,
  onSplitBottom,
  onClosePanel,
  onWorkspace,
  onRename,
  onSessionTree,
}: {
  session: SessionSummary
  canSplit?: boolean
  // 是否提供「关闭面板」入口：移动端单会话视图没有可关闭的面板。
  canClose?: boolean
  streaming?: boolean
  switchingCwd?: boolean
  onSplitLeft: () => void
  onSplitRight: () => void
  onSplitTop: () => void
  onSplitBottom: () => void
  onClosePanel: () => void
  onWorkspace: () => void
  onRename: () => void
  onSessionTree: () => void
}) {
  const { t, language } = useI18n()
  const [open, setOpen] = useState(false)
  // 输入框内始终向上展开以避开底部边界；空会话头部仍向下展开。
  const [placement, setPlacement] = useState<'top' | 'bottom'>('top')
  const rootRef = useRef<HTMLDivElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)

  // 关闭菜单并把焦点归还触发按钮：Esc 关闭时键盘用户不至于丢失焦点位置。
  const closeAndRestoreFocus = useCallback(() => {
    setOpen(false)
    triggerRef.current?.focus()
  }, [])

  useEffect(() => {
    if (!open) return undefined
    const close = (event: MouseEvent) => {
      const target = event.target instanceof Node ? event.target : null
      // 菜单已 portal 到 body，点击菜单内部不算外部点击。
      // 外部点击时焦点会随浏览器默认行为落到点击处，无需归还触发元素；
      // Esc 关闭由 AnchoredPopupMenu 统一走 onClose（见 closeAndRestoreFocus）。
      if (!rootRef.current?.contains(target) && !menuRef.current?.contains(target)) setOpen(false)
    }
    document.addEventListener('mousedown', close)
    return () => document.removeEventListener('mousedown', close)
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
    <div ref={rootRef} className="relative flex-none">
      <Button
        type="button"
        variant="ghost"
        size="icon"
        ref={triggerRef}
        title={t('chat:focusSession.chatActions')}
        aria-label={t('chat:focusSession.openChatActionsMenu')}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={!session}
        onClick={() => {
          if (!open)
            setPlacement(
              rootRef.current?.closest('.focus-composer, .composer-tool-tray') ? 'top' : 'bottom',
            )
          setOpen((visible) => !visible)
        }}
      >
        <MoreHorizontal size={17} />
      </Button>
      <AnchoredPopupMenu
        open={open}
        anchorRef={rootRef}
        menuRef={menuRef}
        placement={placement}
        align="end"
        onClose={closeAndRestoreFocus}
        className="anchored-popup-menu permission-mode-menu session-actions-menu w-[250px] max-w-[calc(100vw_-_16px)] overflow-hidden rounded-md bg-popover p-1 text-popover-foreground shadow-float [&_>_button]:grid [&_>_button]:min-h-11 [&_>_button]:w-full [&_>_button]:grid-cols-[auto_minmax(0,1fr)_auto] [&_>_button]:items-center [&_>_button]:gap-2 [&_>_button]:rounded-sm [&_>_button]:border-0 [&_>_button]:bg-transparent [&_>_button]:px-2 [&_>_button]:py-1.5 [&_>_button]:text-left [&_>_button]:text-content [&_>_button:hover]:bg-surface-hover [&_>_button.active]:bg-surface-hover [&_>_button:disabled]:cursor-not-allowed [&_>_button:disabled]:opacity-50 [&_>_button_>_span:nth-child(2)]:flex [&_>_button_>_span:nth-child(2)]:min-w-0 [&_>_button_>_span:nth-child(2)]:flex-col [&_>_button_>_span:nth-child(2)]:gap-0.5 [&_>_button_>_svg]:text-content-muted [&_small]:text-xs [&_small]:leading-snug [&_small]:text-content-muted [&_strong]:text-sm [&_strong]:font-medium"
        role="menu"
      >
        {canSplit &&
          splitActions.map(([Icon, label, description, action]) => (
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
        <button
          type="button"
          role="menuitem"
          disabled={streaming}
          onClick={() => run(onSessionTree)}
        >
          <TreePine size={15} />
          <span>
            <strong>{t('chat:sessionTree.menu')}</strong>
            <small>
              {streaming ? t('chat:sessionTree.waitForRun') : t('chat:sessionTree.menuDescription')}
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
        {canClose && (
          <button type="button" role="menuitem" onClick={() => run(onClosePanel)}>
            <X size={15} />
            <span>
              <strong>{t('chat:focusSession.closeTab')}</strong>
              <small>{t('chat:focusSession.keepTheHistoryAndCloseOnlyThisView')}</small>
            </span>
          </button>
        )}
      </AnchoredPopupMenu>
    </div>
  )
}
