import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from 'react'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import { ChevronDown, Maximize2, Minus, Plus, TerminalSquare, X } from 'lucide-react'
import type {
  DesktopTerminalEvent,
  DesktopTerminalProfile,
  DesktopTerminalCreated,
} from '@/types/update'

type TerminalTab = {
  id: string
  title: string
  profileId: string
  cwd: string
  status: 'starting' | 'running' | 'exited' | 'error'
  exitCode?: number | null
  error?: string
}

type TerminalRuntime = {
  terminal: Terminal
  fit: FitAddon
  element: HTMLDivElement
  resizeObserver: ResizeObserver
  disposables: Array<{ dispose: () => void }>
}

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
  starting: string
  processExited: (code: number | null) => string
}

type TerminalPanelProps = {
  open: boolean
  height: number
  labels: TerminalPanelLabels
  resolveActiveSessionCwd: () => Promise<string>
  onOpenChange: (open: boolean) => void
  onHeightChange: (height: number) => void
}

const DEFAULT_COLS = 100
const DEFAULT_ROWS = 24
const MIN_HEIGHT = 180
const MAX_HEIGHT = 640
const WORKBENCH_RESERVED_HEIGHT = 420

function maximumTerminalHeight(viewportHeight: number) {
  return Math.max(
    MIN_HEIGHT,
    Math.min(
      MAX_HEIGHT,
      Math.floor(viewportHeight * 0.62),
      viewportHeight - WORKBENCH_RESERVED_HEIGHT,
    ),
  )
}

function terminalTheme() {
  const style = getComputedStyle(document.documentElement)
  const color = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback
  return {
    background: color('--terminal-bg', '#111318'),
    foreground: color('--terminal-fg', '#e5e7eb'),
    cursor: color('--terminal-cursor', '#7dd3fc'),
    selectionBackground: color('--terminal-selection', '#334155'),
    black: '#111318',
    red: '#f87171',
    green: '#86efac',
    yellow: '#fde68a',
    blue: '#93c5fd',
    magenta: '#d8b4fe',
    cyan: '#67e8f9',
    white: '#e5e7eb',
    brightBlack: '#6b7280',
    brightRed: '#fca5a5',
    brightGreen: '#bbf7d0',
    brightYellow: '#fef08a',
    brightBlue: '#bfdbfe',
    brightMagenta: '#e9d5ff',
    brightCyan: '#a5f3fc',
    brightWhite: '#f9fafb',
  }
}

function terminalId() {
  return `terminal-${Date.now()}-${crypto.randomUUID().slice(0, 8)}`
}

function terminalTitle(profile: DesktopTerminalProfile, cwd: string) {
  const normalized = cwd.replace(/[\\/]+$/, '')
  const workspace = normalized.split(/[\\/]/).at(-1)
  return workspace ? `${profile.label} · ${workspace}` : profile.label
}

export function TerminalPanel({
  open,
  height,
  labels,
  resolveActiveSessionCwd,
  onOpenChange,
  onHeightChange,
}: TerminalPanelProps) {
  const bridge = window.pisperDesktop
  const [profiles, setProfiles] = useState<DesktopTerminalProfile[]>([])
  const [tabs, setTabs] = useState<TerminalTab[]>([])
  const [activeId, setActiveId] = useState('')
  const [profileMenuOpen, setProfileMenuOpen] = useState(false)
  const [panelError, setPanelError] = useState('')
  const hostsRef = useRef<Map<string, HTMLDivElement>>(new Map())
  const runtimesRef = useRef<Map<string, TerminalRuntime>>(new Map())
  const outputBufferRef = useRef<Map<string, Uint8Array[]>>(new Map())
  const liveTerminalIdsRef = useRef<Set<string>>(new Set())
  const activeIdRef = useRef(activeId)
  const openRef = useRef(open)
  activeIdRef.current = activeId
  openRef.current = open

  const activeTab = tabs.find((tab) => tab.id === activeId)

  useEffect(() => {
    if (!open) return
    const clampHeight = () => {
      const maximum = maximumTerminalHeight(window.innerHeight)
      if (height > maximum) onHeightChange(maximum)
    }
    clampHeight()
    window.addEventListener('resize', clampHeight)
    return () => window.removeEventListener('resize', clampHeight)
  }, [height, onHeightChange, open])
  const supported = Boolean(bridge?.terminalProfiles && bridge.terminalCreate)

  useEffect(() => {
    if (!supported) return
    bridge
      ?.terminalProfiles?.()
      .then(setProfiles)
      .catch((error) => setPanelError(error instanceof Error ? error.message : String(error)))
  }, [bridge, supported])

  const disposeRuntime = useCallback((id: string) => {
    const runtime = runtimesRef.current.get(id)
    if (!runtime) return
    runtime.resizeObserver.disconnect()
    runtime.disposables.forEach((disposable) => disposable.dispose())
    runtime.terminal.dispose()
    runtime.element.remove()
    runtimesRef.current.delete(id)
  }, [])

  const resizeRuntime = useCallback(
    (id: string) => {
      const runtime = runtimesRef.current.get(id)
      if (!runtime || id !== activeIdRef.current || !openRef.current) return
      runtime.fit.fit()
      const { cols, rows } = runtime.terminal
      void bridge?.terminalResize?.(id, cols, rows).catch(() => {})
    },
    [bridge],
  )

  const mountRuntime = useCallback(
    (id: string) => {
      const existing = runtimesRef.current.get(id)
      const host = hostsRef.current.get(id)
      if (existing) {
        if (host && existing.element.parentElement !== host) {
          host.append(existing.element)
          existing.resizeObserver.disconnect()
          existing.resizeObserver.observe(host)
        }
        return existing
      }
      if (!host) return undefined
      const element = document.createElement('div')
      element.className = 'terminal-xterm'
      host.append(element)
      const terminal = new Terminal({
        allowProposedApi: false,
        convertEol: false,
        cursorBlink: true,
        cursorStyle: 'bar',
        fontFamily: "'Cascadia Mono', 'SFMono-Regular', Consolas, 'Liberation Mono', monospace",
        fontSize: 13,
        letterSpacing: 0,
        lineHeight: 1.2,
        scrollback: 8_000,
        theme: terminalTheme(),
      })
      const fit = new FitAddon()
      terminal.loadAddon(fit)
      terminal.loadAddon(
        new WebLinksAddon((_event, uri) => window.open(uri, '_blank', 'noopener,noreferrer')),
      )
      terminal.open(element)
      fit.fit()
      const disposables = [
        terminal.onData((data) => {
          void bridge?.terminalWrite?.(id, new TextEncoder().encode(data)).catch((error) => {
            setPanelError(error instanceof Error ? error.message : String(error))
          })
        }),
        terminal.onBinary((data) => {
          const bytes = Uint8Array.from(data, (character) => character.charCodeAt(0))
          void bridge?.terminalWrite?.(id, bytes).catch(() => {})
        }),
      ]
      const resizeObserver = new ResizeObserver(() => resizeRuntime(id))
      resizeObserver.observe(host)
      const runtime = { terminal, fit, element, resizeObserver, disposables }
      runtimesRef.current.set(id, runtime)
      const buffered = outputBufferRef.current.get(id) || []
      for (const data of buffered) terminal.write(data)
      outputBufferRef.current.delete(id)
      return runtime
    },
    [bridge, resizeRuntime],
  )

  useEffect(() => {
    if (!open || !activeId) return
    requestAnimationFrame(() => {
      const runtime = mountRuntime(activeId)
      runtime?.fit.fit()
      runtime?.terminal.focus()
    })
  }, [activeId, mountRuntime, open])

  useEffect(() => {
    const observer = new MutationObserver(() => {
      for (const runtime of runtimesRef.current.values())
        runtime.terminal.options.theme = terminalTheme()
    })
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-theme'],
    })
    return () => observer.disconnect()
  }, [])

  useEffect(() => {
    const runtimes = runtimesRef.current
    return () => {
      void bridge?.terminalCloseAll?.().catch(() => {})
      for (const id of [...runtimes.keys()]) disposeRuntime(id)
    }
  }, [bridge, disposeRuntime])

  const handleTerminalEvent = useCallback(
    (event: DesktopTerminalEvent) => {
      if (!liveTerminalIdsRef.current.has(event.terminalId)) return
      const runtime = mountRuntime(event.terminalId)
      if (event.type === 'output') {
        const data = Uint8Array.from(event.data)
        if (runtime) runtime.terminal.write(data)
        else {
          const buffered = outputBufferRef.current.get(event.terminalId) || []
          buffered.push(data)
          outputBufferRef.current.set(event.terminalId, buffered)
        }
        return
      }
      if (event.type === 'exit') {
        setTabs((current) =>
          current.map((tab) =>
            tab.id === event.terminalId ? { ...tab, status: 'exited', exitCode: event.code } : tab,
          ),
        )
        runtime?.terminal.write(`\r\n\x1b[90m${labels.processExited(event.code)}\x1b[0m\r\n`)
      } else {
        setTabs((current) =>
          current.map((tab) =>
            tab.id === event.terminalId ? { ...tab, status: 'error', error: event.message } : tab,
          ),
        )
        runtime?.terminal.write(`\r\n\x1b[31m${event.message}\x1b[0m\r\n`)
      }
    },
    [labels, mountRuntime],
  )

  const createTerminal = useCallback(
    async (requestedProfile?: DesktopTerminalProfile) => {
      const profile = requestedProfile || profiles.find((item) => item.default) || profiles[0]
      if (!profile || !bridge?.terminalCreate) return
      setProfileMenuOpen(false)
      setPanelError('')
      onOpenChange(true)
      const cwd = await resolveActiveSessionCwd().catch(() => '')
      const id = terminalId()
      const tab: TerminalTab = {
        id,
        profileId: profile.id,
        title: terminalTitle(profile, cwd),
        cwd,
        status: 'starting',
      }
      liveTerminalIdsRef.current.add(id)
      setTabs((current) => [...current, tab])
      setActiveId(id)
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
      const runtime = mountRuntime(id)
      runtime?.terminal.write(`\x1b[90m${labels.starting}\x1b[0m\r\n`)
      try {
        const created: DesktopTerminalCreated = await bridge.terminalCreate(
          {
            terminalId: id,
            profileId: profile.id,
            cwd,
            cols: runtimesRef.current.get(id)?.terminal.cols || DEFAULT_COLS,
            rows: runtimesRef.current.get(id)?.terminal.rows || DEFAULT_ROWS,
          },
          handleTerminalEvent,
        )
        if (!liveTerminalIdsRef.current.has(id)) {
          await bridge.terminalClose?.(id).catch(() => false)
          return
        }
        setTabs((current) =>
          current.map((item) =>
            item.id === id
              ? {
                  ...item,
                  status: 'running',
                  cwd: created.cwd,
                  profileId: created.profileId,
                  title: terminalTitle(profile, created.cwd),
                }
              : item,
          ),
        )
        requestAnimationFrame(() => resizeRuntime(id))
      } catch (error) {
        if (!liveTerminalIdsRef.current.has(id)) return
        const message = error instanceof Error ? error.message : String(error)
        setTabs((current) =>
          current.map((item) =>
            item.id === id ? { ...item, status: 'error', error: message } : item,
          ),
        )
        runtimesRef.current.get(id)?.terminal.write(`\r\n\x1b[31m${message}\x1b[0m\r\n`)
      }
    },
    [
      bridge,
      handleTerminalEvent,
      labels.starting,
      mountRuntime,
      onOpenChange,
      profiles,
      resizeRuntime,
      resolveActiveSessionCwd,
    ],
  )

  const closeTerminal = useCallback(
    async (id: string) => {
      liveTerminalIdsRef.current.delete(id)
      await bridge?.terminalClose?.(id).catch(() => false)
      disposeRuntime(id)
      outputBufferRef.current.delete(id)
      setTabs((current) => {
        const index = current.findIndex((tab) => tab.id === id)
        const next = current.filter((tab) => tab.id !== id)
        setActiveId((currentActive) => {
          if (currentActive !== id) return currentActive
          return next[Math.min(index, next.length - 1)]?.id || ''
        })
        return next
      })
    },
    [bridge, disposeRuntime],
  )

  const beginResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId)
    const startY = event.clientY
    const startHeight = height
    const move = (moveEvent: PointerEvent) => {
      const maximum = maximumTerminalHeight(window.innerHeight)
      onHeightChange(
        Math.max(MIN_HEIGHT, Math.min(maximum, startHeight + startY - moveEvent.clientY)),
      )
    }
    const finish = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', finish)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', finish, { once: true })
  }

  if (!supported) return null

  return (
    <section
      className={`terminal-panel ${open ? 'is-open' : ''}`}
      style={open ? { height } : undefined}
      aria-label={labels.terminal}
    >
      {open && (
        <div
          className="terminal-resize-handle"
          role="separator"
          aria-orientation="horizontal"
          aria-label={labels.resizeTerminal}
          onPointerDown={beginResize}
        />
      )}
      <div className="terminal-toolbar">
        <button
          className="terminal-title"
          title={open ? labels.hideTerminal : labels.showTerminal}
          onClick={() => onOpenChange(!open)}
        >
          <TerminalSquare size={15} />
          <span>{labels.terminal}</span>
          {tabs.length > 0 && <small>{tabs.length}</small>}
          <ChevronDown className={open ? '' : '-rotate-90'} size={14} />
        </button>
        {open && (
          <div className="terminal-tabs" role="tablist">
            {tabs.map((tab) => (
              <button
                className={`terminal-tab ${activeId === tab.id ? 'active' : ''}`}
                role="tab"
                aria-selected={activeId === tab.id}
                title={tab.cwd || tab.title}
                onClick={() => setActiveId(tab.id)}
                key={tab.id}
              >
                <i data-status={tab.status} />
                <span>{tab.title}</span>
                <X
                  size={13}
                  aria-label={labels.closeTerminal}
                  onClick={(event) => {
                    event.stopPropagation()
                    void closeTerminal(tab.id)
                  }}
                />
              </button>
            ))}
          </div>
        )}
        <div className="terminal-actions">
          {open && (
            <div className="terminal-profile-menu-root">
              <button
                className="icon-button"
                title={labels.newTerminal}
                aria-label={labels.newTerminal}
                disabled={!profiles.length}
                onClick={() =>
                  profiles.length > 1
                    ? setProfileMenuOpen((value) => !value)
                    : void createTerminal()
                }
              >
                <Plus size={15} />
              </button>
              {profileMenuOpen && (
                <div className="terminal-profile-menu">
                  {profiles.map((profile) => (
                    <button onClick={() => void createTerminal(profile)} key={profile.id}>
                      <TerminalSquare size={14} />
                      <span>{profile.label}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}
          {open && activeTab && (
            <button
              className="icon-button"
              title={labels.maximizeTerminal}
              aria-label={labels.maximizeTerminal}
              onClick={() => onHeightChange(maximumTerminalHeight(window.innerHeight))}
            >
              <Maximize2 size={14} />
            </button>
          )}
          <button
            className="icon-button"
            title={open ? labels.hideTerminal : labels.showTerminal}
            aria-label={open ? labels.hideTerminal : labels.showTerminal}
            onClick={() => onOpenChange(!open)}
          >
            {open ? <Minus size={15} /> : <TerminalSquare size={15} />}
          </button>
        </div>
      </div>
      {open && (
        <div className="terminal-content">
          {!tabs.length ? (
            <button
              className="terminal-empty"
              onClick={() => void createTerminal()}
              disabled={!profiles.length}
            >
              <TerminalSquare size={22} />
              <strong>{labels.openTerminal}</strong>
              <span>{labels.usesActiveSessionWorkspace}</span>
            </button>
          ) : (
            tabs.map((tab) => (
              <div
                className={`terminal-host ${activeId === tab.id ? 'active' : ''}`}
                ref={(element) => {
                  if (element) hostsRef.current.set(tab.id, element)
                  else hostsRef.current.delete(tab.id)
                }}
                role="tabpanel"
                hidden={activeId !== tab.id}
                key={tab.id}
              />
            ))
          )}
          {panelError && <div className="terminal-error">{panelError}</div>}
        </div>
      )}
    </section>
  )
}
