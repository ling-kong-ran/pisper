// 右侧屏承接原来底栏里的资产，以及改动、扩展、文件和终端，主屏因此可以一直留在对话。
import { lazy, Suspense, useCallback, useEffect, useState } from 'react'
import { Blocks, File, FileDiff, Folder, FolderOpen, FolderUp, TerminalSquare } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import type { Notify } from '@/app/routes/route-context'
import { SessionFilesPane } from '@/features/chat/components/files/SessionFilesPane'
import type { TerminalPanelLabels } from '@/features/terminal/components/TerminalPanel'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import type { ChatAttachment } from '@/types/chat'
import {
  joinWorkspacePath,
  listWorkspaceEntries,
  parentWorkspacePath,
  type WorkspaceListEntry,
} from '@/features/chat/api/workspace-entries'
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
  { id: 'files', icon: File },
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

function WorkspaceFiles({ sessionId }: { sessionId: string }) {
  const { t } = useI18n()
  const [path, setPath] = useState('')
  const [entries, setEntries] = useState<WorkspaceListEntry[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    setError('')
    void listWorkspaceEntries(path, controller.signal)
      .then((data) => {
        if (!controller.signal.aborted) setEntries(data)
      })
      .catch((caught: unknown) => {
        if (controller.signal.aborted) return
        setEntries([])
        setError(caught instanceof Error ? caught.message : String(caught))
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [path, sessionId])

  const parent = parentWorkspacePath(path)

  return (
    <div className="flex h-full min-h-0 flex-col" data-swipe-ignore>
      <div className="flex flex-none items-center gap-2 border-b border-[var(--stroke-soft)] px-3 py-2">
        <button
          type="button"
          className="grid size-8 place-items-center rounded-md text-muted-foreground enabled:hover:bg-muted disabled:opacity-40"
          disabled={!path}
          aria-label={t('navigation:mobileShell.filesUp')}
          onClick={() => setPath(parent)}
        >
          <FolderUp size={16} />
        </button>
        <p className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
          {path || t('navigation:mobileShell.workspaceRoot')}
        </p>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-2">
        {loading ? (
          <p className="px-2 py-3 text-sm text-muted-foreground">
            {t('navigation:mobileShell.filesLoading')}
          </p>
        ) : error ? (
          <p className="px-2 py-3 text-sm text-[var(--danger)]" role="alert">
            {error}
          </p>
        ) : entries.length === 0 ? (
          <p className="px-2 py-3 text-sm text-muted-foreground">
            {t('navigation:mobileShell.filesEmpty')}
          </p>
        ) : (
          <ul className="flex flex-col gap-0.5">
            {entries.map((entry) => (
              <li key={`${entry.kind}:${entry.name}`}>
                {entry.kind === 'directory' ? (
                  <button
                    type="button"
                    className="flex h-10 w-full items-center gap-2 rounded-md px-2 text-left text-sm hover:bg-muted"
                    onClick={() => setPath(joinWorkspacePath(path, entry.name))}
                  >
                    <Folder size={16} aria-hidden="true" />
                    <span className="truncate">{entry.name}</span>
                  </button>
                ) : (
                  <div className="flex h-10 items-center gap-2 px-2 text-sm text-muted-foreground">
                    <File size={16} aria-hidden="true" />
                    <span className="truncate">{entry.name}</span>
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  )
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
    files: t('navigation:mobileShell.files'),
    terminal: t('navigation:mobileShell.terminal'),
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
        role="tablist"
        aria-label={t('navigation:mobileShell.context')}
        data-swipe-ignore
      >
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
        {tab === 'files' && (
          <WorkspaceFiles key={activeSessionId || 'workspace'} sessionId={activeSessionId} />
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
