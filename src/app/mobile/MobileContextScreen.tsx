// 右侧屏承接原来底栏里的资产，以及改动、扩展和终端，主屏因此可以一直留在对话。
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react'
import { Blocks, FileDiff, FolderOpen, TerminalSquare } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import type { Notify } from '@/app/routes/route-context'
import { SessionFilesPane } from '@/features/chat/components/files/SessionFilesPane'
import type { TerminalPanelLabels } from '@/features/terminal/components/TerminalPanel'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import type { ChatAttachment } from '@/types/chat'
import {
  LocalPathRevealError,
  requestLocalPathReveal,
  revealPathThroughRuntime,
} from '@/lib/platform/local-path-reveal'
import type { MobileContextTab } from '@/components/layout/mobile-shell-layout'

const AssetsPage = lazy(() =>
  import('@/features/assets/pages/AssetsPage').then((module) => ({ default: module.AssetsPage })),
)
const PluginsPage = lazy(() =>
  import('@/features/plugins/pages/PluginsPage').then((module) => ({
    default: module.PluginsPage,
  })),
)
const TerminalPanel = lazy(() =>
  import('@/features/terminal/components/TerminalPanel').then((module) => ({
    default: module.TerminalPanel,
  })),
)

const TABS: Array<{ id: MobileContextTab; icon: typeof FolderOpen }> = [
  { id: 'assets', icon: FolderOpen },
  { id: 'changes', icon: FileDiff },
  { id: 'extensions', icon: Blocks },
  { id: 'terminal', icon: TerminalSquare },
]

type MobileContextScreenProps = {
  visible: boolean
  sessionStreaming: boolean | null
  onCloseTerminal: () => void
  onOpenChat: () => void
  tab: MobileContextTab
  onTabChange: (tab: MobileContextTab) => void
  activeSessionId: string
  query: string
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  onUseAsset: (asset: ChatAttachment) => void
  terminalSupported: boolean
  terminalLabels: TerminalPanelLabels
  resolveSessionCwd: (sessionId: string) => Promise<string>
}

// 内嵌页面的操作独立注册，避免覆盖仍在中栏的聊天操作。
function usePanelAction() {
  const [action, setAction] = useState<(() => void) | null>(null)
  const register = useCallback((next: () => void) => {
    setAction(() => next)
    return () => setAction((current) => (current === next ? null : current))
  }, [])
  return { action, register }
}

export function MobileContextScreen({
  visible,
  sessionStreaming,
  onCloseTerminal,
  onOpenChat,
  tab,
  onTabChange,
  activeSessionId,
  query,
  notify,
  requestConfirm,
  requestText,
  onUseAsset,
  terminalSupported,
  terminalLabels,
  resolveSessionCwd,
}: MobileContextScreenProps) {
  const { t } = useI18n()
  const labels: Record<MobileContextTab, string> = {
    assets: t('navigation:navigation.assets'),
    changes: t('navigation:mobileShell.changes'),
    extensions: t('navigation:mobileShell.extensions'),
    terminal: t('navigation:mobileShell.terminal'),
  }
  const [openingFiles, setOpeningFiles] = useState(false)
  const openingFilesRef = useRef(false)
  const openWorkspace = async () => {
    if (openingFilesRef.current || !activeSessionId) return
    openingFilesRef.current = true
    setOpeningFiles(true)
    try {
      // 必须使用会话目录，不能把 Runtime 的默认工作区当作当前项目。
      const path = await resolveSessionCwd(activeSessionId)
      if (!path.trim()) {
        notify(t('navigation:mobileShell.workspaceUnavailable'), 'error')
        return
      }
      await requestLocalPathReveal(path, revealPathThroughRuntime)
    } catch (error) {
      notify(
        error instanceof LocalPathRevealError && error.reason === 'unavailable'
          ? t('common:markdownMessage.revealLocalPathUnavailable')
          : error instanceof LocalPathRevealError && error.reason === 'timeout'
            ? t('common:markdownMessage.revealLocalPathTimeout')
            : t('navigation:mobileShell.openWorkspaceFailed'),
        'error',
      )
    } finally {
      openingFilesRef.current = false
      setOpeningFiles(false)
    }
  }
  const assetsAction = usePanelAction()
  const pluginsAction = usePanelAction()
  const [visited, setVisited] = useState<Set<MobileContextTab>>(() => new Set([tab]))
  useEffect(() => {
    setVisited((current) => (current.has(tab) ? current : new Set([...current, tab])))
  }, [tab])
  const loaded = (id: MobileContextTab) => visited.has(id) || tab === id

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col bg-background" data-mobile-context-screen>
      <div
        className="flex flex-none gap-1 overflow-x-auto border-b border-[var(--stroke-soft)] px-2 py-1.5 [-ms-overflow-style:none] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        data-swipe-ignore
      >
        <div className="flex gap-1" role="tablist" aria-label={t('navigation:mobileShell.context')}>
          {TABS.map(({ id, icon: Icon }) => {
            const selected = tab === id
            return (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={selected}
                className={cn(
                  'flex h-8 flex-none items-center gap-1.5 rounded-full px-2.5 text-[12px] font-medium',
                  selected
                    ? 'bg-[var(--star-soft)] text-[var(--star-strong)]'
                    : 'text-[var(--text-muted)]',
                )}
                onClick={() => onTabChange(id)}
              >
                <Icon size={14} aria-hidden="true" />
                {labels[id]}
              </button>
            )
          })}
        </div>
        <Button
          variant="ghost"
          size="sm"
          className="shrink-0"
          disabled={openingFiles || !activeSessionId}
          aria-busy={openingFiles}
          aria-label={t('navigation:mobileShell.openWorkspace')}
          title={t('navigation:mobileShell.openWorkspace')}
          onClick={() => void openWorkspace()}
        >
          <FolderOpen size={14} aria-hidden="true" />
          {t('navigation:mobileShell.files')}
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-hidden">
        {loaded('assets') && (
          <div
            hidden={tab !== 'assets'}
            className="h-full overflow-auto px-3 py-3 [&_.asset-grid]:!grid-cols-1"
          >
            <Button
              className="mb-3"
              disabled={!assetsAction.action}
              onClick={() => assetsAction.action?.()}
            >
              {t('assets:assetsPage.addLink')}
            </Button>
            <Suspense fallback={null}>
              <AssetsPage
                key={activeSessionId}
                query={query}
                notify={notify}
                registerPrimaryAction={assetsAction.register}
                requestConfirm={requestConfirm}
                onUse={onUseAsset}
              />
            </Suspense>
          </div>
        )}
        {tab === 'changes' &&
          (activeSessionId ? (
            sessionStreaming === null ? (
              <Button className="m-3" variant="outline" onClick={onOpenChat}>
                {t('navigation:mobileShell.changesOpenChat')}
              </Button>
            ) : (
              <div className="flex h-full min-h-0 flex-col">
                <SessionFilesPane
                  key={activeSessionId}
                  sessionId={activeSessionId}
                  streaming={sessionStreaming}
                  requestConfirm={requestConfirm}
                />
              </div>
            )
          ) : (
            <p className="px-4 py-6 text-sm text-muted-foreground">
              {t('navigation:mobileShell.noSessionChanges')}
            </p>
          ))}
        {loaded('extensions') && (
          <div hidden={tab !== 'extensions'} className="h-full overflow-auto px-3 py-3">
            <Button
              className="mb-3"
              disabled={!pluginsAction.action}
              onClick={() => pluginsAction.action?.()}
            >
              {t('plugins:pluginsPage.savePolicy')}
            </Button>
            <Suspense fallback={null}>
              <PluginsPage
                query={query}
                notify={notify}
                registerPrimaryAction={pluginsAction.register}
                requestText={requestText}
                requestConfirm={requestConfirm}
              />
            </Suspense>
          </div>
        )}
        {loaded('terminal') && (
          <div hidden={tab !== 'terminal'} className="h-full min-h-0">
            {terminalSupported ? (
              <Suspense fallback={null}>
                <TerminalPanel
                  layout="fill"
                  open={visible && tab === 'terminal'}
                  height={480}
                  labels={terminalLabels}
                  activeSessionId={activeSessionId}
                  resolveSessionCwd={resolveSessionCwd}
                  onOpenChange={(open) => {
                    if (!open) onCloseTerminal()
                  }}
                  onHeightChange={() => {}}
                />
              </Suspense>
            ) : (
              <div className="grid h-full place-content-center gap-2 px-6 text-center text-sm text-muted-foreground">
                <TerminalSquare className="mx-auto" size={22} aria-hidden="true" />
                <p>{t('navigation:mobileShell.terminalUnavailable')}</p>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
