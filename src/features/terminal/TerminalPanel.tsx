// 底部终端只是视图入口；PTY 与输出由应用级 TerminalProvider 持有。
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
import { Maximize2, Minus, Plus, TerminalSquare, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { useTerminalService, useTerminals } from './terminal-context'
import { TaskTerminalView } from './TaskTerminalView'

export type TerminalPanelLabels = {
  terminal: string
  resizeTerminal: string
  hideTerminal: string
  showTerminal: string
  newTerminal: string
  closeTerminal: string
  maximizeTerminal: string
  openTerminal: string
  usesActiveSessionWorkspace: string
  orphanedTerminal: string
  starting: string
  processExited: (code: number | null) => string
}

type TerminalPanelProps = {
  open: boolean
  height: number
  labels: TerminalPanelLabels
  activeSessionId: string
  resolveSessionCwd: (sessionId: string) => Promise<string>
  onOpenChange: (open: boolean) => void
  onHeightChange: (height: number) => void
  // 兼容嵌入式宿主；只改变视图尺寸，不转移 Provider 对终端进程的所有权。
  layout?: 'dock' | 'fill'
}

const MIN_HEIGHT = 180
const CHAT_RESERVED_HEIGHT = 420
function maximumTerminalHeight(viewportHeight: number) {
  return Math.max(
    MIN_HEIGHT,
    Math.min(640, Math.floor(viewportHeight * 0.62), viewportHeight - CHAT_RESERVED_HEIGHT),
  )
}

export function TerminalPanel({
  open,
  height,
  labels,
  activeSessionId,
  resolveSessionCwd,
  onOpenChange,
  onHeightChange,
  layout = 'dock',
}: TerminalPanelProps) {
  const { t } = useI18n()
  const service = useTerminalService()
  const records = useTerminals()
  const [activeIds, setActiveIds] = useState<Record<string, string>>({})
  const [hidden, setHidden] = useState<Set<string>>(() => new Set())
  const [newViews, setNewViews] = useState<Record<string, { cwd: string; token: string }>>({})
  const [error, setError] = useState('')
  const resizeCleanup = useRef<() => void>(() => {})
  const visible = records.filter(
    (record) => (record.sessionId === activeSessionId || record.orphaned) && !hidden.has(record.id),
  )
  const active = visible.find((record) => record.id === activeIds[activeSessionId]) || visible[0]
  const pending = newViews[activeSessionId]
  const newTerminal = async () => {
    const owner = activeSessionId
    onOpenChange(true)
    setError('')
    try {
      const cwd = await resolveSessionCwd(owner)
      setNewViews((current) => ({ ...current, [owner]: { cwd, token: crypto.randomUUID() } }))
    } catch (caught) {
      setError(String(caught))
    }
  }
  const closeView = (id: string) => {
    setHidden((current) => new Set([...current, id]))
    setActiveIds((current) => ({
      ...current,
      [activeSessionId]: visible.find((record) => record.id !== id)?.id || '',
    }))
  }
  useEffect(() => {
    if (!open || layout === 'fill') return
    const clampHeight = () => {
      const max = maximumTerminalHeight(window.innerHeight)
      if (height > max) onHeightChange(max)
    }
    clampHeight()
    window.addEventListener('resize', clampHeight)
    return () => window.removeEventListener('resize', clampHeight)
  }, [height, layout, open, onHeightChange])
  useEffect(() => () => resizeCleanup.current(), [])
  const beginResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    resizeCleanup.current()
    event.currentTarget.setPointerCapture(event.pointerId)
    const startY = event.clientY,
      startHeight = height
    const move = (next: PointerEvent) =>
      onHeightChange(
        Math.max(
          MIN_HEIGHT,
          Math.min(maximumTerminalHeight(window.innerHeight), startHeight + startY - next.clientY),
        ),
      )
    const finish = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', finish)
      window.removeEventListener('pointercancel', finish)
    }
    resizeCleanup.current = finish
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', finish, { once: true })
    window.addEventListener('pointercancel', finish, { once: true })
  }
  if (!service || !open) return null
  const fill = layout === 'fill'
  return (
    <section
      aria-label={labels.terminal}
      className={`terminal-panel relative z-3 flex flex-col bg-background ${fill ? 'h-full min-h-0 flex-1' : 'shrink-0 border-t border-border'}`}
      style={fill ? undefined : { height }}
      data-swipe-ignore={fill ? true : undefined}
    >
      {!fill && (
        <div
          role="separator"
          tabIndex={0}
          aria-orientation="horizontal"
          aria-label={labels.resizeTerminal}
          aria-valuemin={MIN_HEIGHT}
          aria-valuemax={maximumTerminalHeight(window.innerHeight)}
          aria-valuenow={height}
          onPointerDown={beginResize}
          onKeyDown={(event) => {
            if (!['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return
            event.preventDefault()
            onHeightChange(
              event.key === 'Home'
                ? MIN_HEIGHT
                : event.key === 'End'
                  ? maximumTerminalHeight(window.innerHeight)
                  : Math.max(
                      MIN_HEIGHT,
                      Math.min(
                        maximumTerminalHeight(window.innerHeight),
                        height + (event.key === 'ArrowUp' ? 20 : -20),
                      ),
                    ),
            )
          }}
          className="absolute inset-x-0 -top-1 z-10 h-2 cursor-ns-resize focus-visible:outline-2 focus-visible:outline-ring"
        />
      )}
      <header className="flex h-10 shrink-0 items-center gap-1 border-b border-border px-2">
        <TerminalSquare className="size-4 shrink-0 text-content-muted" />
        <div
          role="tablist"
          aria-label={labels.terminal}
          className="flex min-w-0 flex-1 overflow-x-auto"
        >
          {visible.map((record) => (
            <div key={record.id} className="flex min-w-0 max-w-52 shrink-0 items-center rounded-md">
              <button
                role="tab"
                aria-selected={!pending && active?.id === record.id}
                className="min-w-0 truncate rounded-md px-2 py-1 text-xs hover:bg-surface-hover aria-selected:bg-surface-muted"
                title={`${record.cwd} · ${record.sessionId}`}
                onClick={() => {
                  setActiveIds((current) => ({ ...current, [activeSessionId]: record.id }))
                  setNewViews((current) => {
                    const next = { ...current }
                    delete next[activeSessionId]
                    return next
                  })
                }}
              >
                {record.orphaned ? `${labels.orphanedTerminal} · ` : ''}
                {record.profileId}
              </button>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={t('terminal:workspace.closeView')}
                title={t('terminal:workspace.closeView')}
                onClick={() => closeView(record.id)}
              >
                <X className="size-3" />
              </Button>
            </div>
          ))}
        </div>
        {hidden.size > 0 && (
          <Button variant="ghost" size="sm" onClick={() => setHidden(new Set())}>
            {t('terminal:workspace.restoreViews')}
          </Button>
        )}
        <Button
          variant="ghost"
          size="icon"
          aria-label={labels.newTerminal}
          title={labels.newTerminal}
          onClick={() => void newTerminal()}
        >
          <Plus className="size-4" />
        </Button>
        {!fill && (
          <Button
            variant="ghost"
            size="icon"
            aria-label={labels.maximizeTerminal}
            title={labels.maximizeTerminal}
            onClick={() => onHeightChange(maximumTerminalHeight(window.innerHeight))}
          >
            <Maximize2 className="size-4" />
          </Button>
        )}
        <Button
          variant="ghost"
          size="icon"
          aria-label={labels.hideTerminal}
          title={labels.hideTerminal}
          onClick={() => onOpenChange(false)}
        >
          <Minus className="size-4" />
        </Button>
      </header>
      {error && (
        <p role="alert" className="p-2 text-xs text-danger">
          {error}
        </p>
      )}
      <div className="min-h-0 flex-1">
        {pending ? (
          <TaskTerminalView
            key={pending.token}
            viewKey="bottom"
            sessionId={activeSessionId}
            cwd={pending.cwd}
            onCreated={(id) => {
              setActiveIds((current) => ({ ...current, [activeSessionId]: id }))
              setNewViews((current) => {
                if (current[activeSessionId]?.token !== pending.token) return current
                const next = { ...current }
                delete next[activeSessionId]
                return next
              })
            }}
          />
        ) : active ? (
          <TaskTerminalView
            viewKey="bottom"
            sessionId={active.sessionId}
            cwd={active.cwd}
            terminalId={active.id}
          />
        ) : (
          <div className="flex h-full flex-col items-center justify-center gap-2">
            <p className="text-sm text-content-muted">{labels.usesActiveSessionWorkspace}</p>
            <Button onClick={() => void newTerminal()}>{labels.openTerminal}</Button>
          </div>
        )}
      </div>
    </section>
  )
}
