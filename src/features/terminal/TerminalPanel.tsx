// 终端面板：桌面桥接的 xterm 终端，按会话绑定工作目录，
// 支持多标签、缩放与断开重连提示。
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
import { SESSIONS_UPDATED_EVENT } from '@/features/chat/events'
import { apiJson } from '@/lib/api'
import {
  activeSessionTerminalId,
  markOrphanedSessionTerminals,
  visibleSessionTerminals,
} from '@/features/terminal/terminal-session-scope'

import { Button } from '@/components/ui/button'

type TerminalTab = {
  id: string
  title: string
  profileId: string
  cwd: string
  sessionId: string
  orphaned: boolean
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
  const dark = document.documentElement.dataset.theme === 'dark'
  const ansi = dark
    ? {
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
    : {
        black: '#1f2937',
        red: '#b91c1c',
        green: '#15803d',
        yellow: '#a16207',
        blue: '#1d4ed8',
        magenta: '#7e22ce',
        cyan: '#0e7490',
        white: '#64748b',
        brightBlack: '#64748b',
        brightRed: '#dc2626',
        brightGreen: '#16a34a',
        brightYellow: '#ca8a04',
        brightBlue: '#2563eb',
        brightMagenta: '#9333ea',
        brightCyan: '#0891b2',
        brightWhite: '#334155',
      }
  return {
    background: color('--terminal-bg', dark ? '#111318' : '#f8fafc'),
    foreground: color('--terminal-fg', dark ? '#e5e7eb' : '#1f2937'),
    cursor: color('--terminal-cursor', dark ? '#7dd3fc' : '#1783ff'),
    selectionBackground: color('--terminal-selection', dark ? '#334155' : '#bfdbfe'),
    ...ansi,
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
  activeSessionId,
  resolveSessionCwd,
  onOpenChange,
  onHeightChange,
}: TerminalPanelProps) {
  const bridge = window.pisperDesktop
  const [profiles, setProfiles] = useState<DesktopTerminalProfile[]>([])
  const [tabs, setTabs] = useState<TerminalTab[]>([])
  const [activeIds, setActiveIds] = useState<Record<string, string>>({})
  const [profileMenuOpen, setProfileMenuOpen] = useState(false)
  const [panelError, setPanelError] = useState('')
  const hostsRef = useRef<Map<string, HTMLDivElement>>(new Map())
  const runtimesRef = useRef<Map<string, TerminalRuntime>>(new Map())
  const outputBufferRef = useRef<Map<string, Uint8Array[]>>(new Map())
  const liveTerminalIdsRef = useRef<Set<string>>(new Set())
  const activeIdRef = useRef('')
  const openRef = useRef(open)
  const visibleTabs = visibleSessionTerminals(tabs, activeSessionId)
  const activeId = activeSessionTerminalId(tabs, activeIds, activeSessionId)
  activeIdRef.current = activeId
  openRef.current = open

  const activeTab = visibleTabs.find((tab) => tab.id === activeId)
  // 选择活动终端：按会话维度记录当前激活的终端标签。
  const selectTerminal = useCallback(
    (id: string) => setActiveIds((current) => ({ ...current, [activeSessionId]: id })),
    [activeSessionId],
  )

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

  // 销毁终端运行时：断开 ResizeObserver、释放插件、销毁 xterm 与 DOM 节点。
  const disposeRuntime = useCallback((id: string) => {
    const runtime = runtimesRef.current.get(id)
    if (!runtime) return
    runtime.resizeObserver.disconnect()
    runtime.disposables.forEach((disposable) => disposable.dispose())
    runtime.terminal.dispose()
    runtime.element.remove()
    runtimesRef.current.delete(id)
  }, [])

  // 调整终端尺寸：仅活动且面板打开时重新 fit 并把 cols/rows 同步到桌面桥接。
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

  // 挂载终端运行时：已存在则移动到新宿主 DOM；否则创建 xterm + Fit/链接
  // 插件，桥接键盘输入（onData/onBinary），并用 ResizeObserver 跟踪宿主尺寸。
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
    const markDeletedSessions = () => {
      void apiJson<{ sessions?: Array<{ id: string }> }>('/api/sessions')
        .then((data) =>
          setTabs((current) =>
            markOrphanedSessionTerminals(
              current,
              (data.sessions || []).map((session) => session.id),
              activeSessionId,
            ),
          ),
        )
        .catch(() => {})
    }
    window.addEventListener(SESSIONS_UPDATED_EVENT, markDeletedSessions)
    return () => window.removeEventListener(SESSIONS_UPDATED_EVENT, markDeletedSessions)
  }, [activeSessionId])

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

  // 处理桌面桥接终端事件：输出写入 xterm（运行时未就绪时先缓冲）；
  // 退出/错误更新标签状态并回显提示文本。
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

  // 创建终端：选默认 profile → 解析会话工作目录 → 注册标签与运行时 →
  // 调桥接 terminalCreate（附带 cols/rows）并订阅事件；创建失败回显错误。
  const createTerminal = useCallback(
    async (requestedProfile?: DesktopTerminalProfile) => {
      const profile = requestedProfile || profiles.find((item) => item.default) || profiles[0]
      if (!profile || !bridge?.terminalCreate) return
      setProfileMenuOpen(false)
      setPanelError('')
      onOpenChange(true)
      const sessionId = activeSessionId
      const cwd = await resolveSessionCwd(sessionId).catch(() => '')
      const id = terminalId()
      const tab: TerminalTab = {
        id,
        profileId: profile.id,
        title: terminalTitle(profile, cwd),
        cwd,
        sessionId,
        orphaned: false,
        status: 'starting',
      }
      liveTerminalIdsRef.current.add(id)
      setTabs((current) => [...current, tab])
      setActiveIds((current) => ({ ...current, [sessionId]: id }))
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
      activeSessionId,
      bridge,
      handleTerminalEvent,
      labels.starting,
      mountRuntime,
      onOpenChange,
      profiles,
      resizeRuntime,
      resolveSessionCwd,
    ],
  )

  // 关闭终端：注销事件订阅、关闭桥接进程、销毁运行时，
  // 并把该会话的激活终端回退到剩余的第一个。
  const closeTerminal = useCallback(
    async (id: string) => {
      liveTerminalIdsRef.current.delete(id)
      await bridge?.terminalClose?.(id).catch(() => false)
      disposeRuntime(id)
      outputBufferRef.current.delete(id)
      setTabs((current) => {
        const closing = current.find((tab) => tab.id === id)
        const visible = closing
          ? visibleSessionTerminals(current, closing.sessionId).filter((tab) => tab.id !== id)
          : []
        setActiveIds((currentActive) => {
          const next = { ...currentActive }
          for (const [sessionId, terminalId] of Object.entries(next)) {
            if (terminalId === id) next[sessionId] = visible[0]?.id || ''
          }
          return next
        })
        return current.filter((tab) => tab.id !== id)
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
      className={`terminal-panel [&.is-open]:min-h-[180px] [&.is-open]:basis-[auto] relative z-[3] flex min-h-[35px] [flex:0_0_35px] flex-col [border-top:1px_solid_var(--stroke)] bg-[var(--terminal-bg)] text-[var(--terminal-fg)] ${open ? 'is-open' : ''}`}
      style={open ? { height } : undefined}
      aria-label={labels.terminal}
    >
      {open && (
        <div
          className="terminal-resize-handle after:absolute after:top-[3px] after:right-[47%] after:left-[47%] after:h-[2px] after:rounded-[1px] after:bg-transparent after:[content:''] [&:hover::after]:bg-[var(--brand-blue)] absolute z-[4] [top:-4px] right-0 left-0 h-[8px] [cursor:ns-resize]"
          role="separator"
          aria-orientation="horizontal"
          aria-label={labels.resizeTerminal}
          onPointerDown={beginResize}
        />
      )}
      <div className="flex h-[34px] [flex:0_0_34px] items-center gap-[4px] [border-bottom:1px_solid_var(--terminal-border)] [padding:0_6px]">
        <button
          className="terminal-title flex h-[27px] min-h-[27px] items-center gap-[6px] border-0 rounded-[4px] bg-transparent text-[var(--terminal-muted)] text-[12px] hover:bg-[var(--terminal-hover)] hover:text-[var(--terminal-fg)] [&_small]:min-w-[17px] [&_small]:rounded-[8px] [&_small]:bg-[var(--terminal-active)] [&_small]:p-[1px_5px] [&_small]:!text-[9px] [&_small]:text-center max-[650px]:[&_>_span]:hidden [flex:0_0_auto] [padding:0_7px] font-[650]"
          title={open ? labels.hideTerminal : labels.showTerminal}
          onClick={() => onOpenChange(!open)}
        >
          <TerminalSquare size={15} />
          <span>{labels.terminal}</span>
          {visibleTabs.length > 0 && <small>{visibleTabs.length}</small>}
          <ChevronDown className={open ? '' : '-rotate-90'} size={14} />
        </button>
        {open && (
          <div
            className="terminal-tabs [&::-webkit-scrollbar]:hidden flex min-w-0 [flex:1_1_auto] items-center gap-[2px] overflow-x-auto [scrollbar-width:none]"
            role="tablist"
          >
            {visibleTabs.map((tab) => (
              <button
                className={`terminal-tab flex h-[27px] min-h-[27px] items-center gap-[6px] border-0 rounded-[4px] bg-transparent text-[var(--terminal-muted)] text-[12px] hover:bg-[var(--terminal-hover)] hover:text-[var(--terminal-fg)] [&.active]:bg-[var(--terminal-active)] [&.active]:text-[var(--terminal-fg)] [&_>_i]:w-[6px] [&_>_i]:h-[6px] [&_>_i]:[flex:0_0_6px] [&_>_i]:rounded-[50%] [&_>_i]:bg-[#6b7280] [&_>_i[data-status='running']]:bg-[#22c55e] [&_>_i[data-status='starting']]:bg-[#eab308] [&_>_i[data-status='error']]:bg-[#ef4444] [&_span]:overflow-hidden [&_span]:flex-1 [&_span]:text-ellipsis [&_span]:whitespace-nowrap [&_>_svg]:[flex:0_0_auto] [&_>_svg]:opacity-0 [&:hover_>_svg]:opacity-[.75] [&.active_>_svg]:opacity-[.75] max-[900px]:max-w-[150px] max-[900px]:basis-[150px] max-[650px]:max-w-[120px] max-[650px]:basis-[120px] max-w-[190px] [flex:0_1_190px] justify-start [padding:0_7px] ${activeId === tab.id ? 'active' : ''}`}
                role="tab"
                aria-selected={activeId === tab.id}
                title={tab.cwd || tab.title}
                onClick={() => selectTerminal(tab.id)}
                key={tab.id}
              >
                <i data-status={tab.status} />
                <span>
                  {tab.orphaned ? `${labels.orphanedTerminal} · ${tab.title}` : tab.title}
                </span>
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
        <div className="flex [flex:0_0_auto] items-center gap-[2px]">
          {open && (
            <div className="relative">
              <Button
                variant="ghost"
                size="icon"
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
              </Button>
              {profileMenuOpen && (
                <div className="terminal-profile-menu [&_button]:flex [&_button]:w-full [&_button]:min-h-[32px] [&_button]:items-center [&_button]:gap-[9px] [&_button]:border-0 [&_button]:rounded-[4px] [&_button]:bg-transparent [&_button]:p-[0_8px] [&_button]:text-[12px] [&_button]:text-left [&_button:hover]:bg-[var(--surface-hover)] absolute z-[10] right-0 [bottom:calc(100%_+_7px)] w-[210px] overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--solid)] shadow-[var(--sh-floating)] text-[var(--text)] [padding:4px]">
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
            <Button
              variant="ghost"
              size="icon"
              title={labels.maximizeTerminal}
              aria-label={labels.maximizeTerminal}
              onClick={() => onHeightChange(maximumTerminalHeight(window.innerHeight))}
            >
              <Maximize2 size={14} />
            </Button>
          )}
          <Button
            variant="ghost"
            size="icon"
            title={open ? labels.hideTerminal : labels.showTerminal}
            aria-label={open ? labels.hideTerminal : labels.showTerminal}
            onClick={() => onOpenChange(!open)}
          >
            {open ? <Minus size={15} /> : <TerminalSquare size={15} />}
          </Button>
        </div>
      </div>
      {open && (
        <div className="relative min-h-0 flex-1 overflow-hidden">
          {!visibleTabs.length ? (
            <button
              className="terminal-empty hover:text-[var(--terminal-fg)] [&_strong]:text-[13px] flex w-full h-full min-h-[120px] items-center justify-center flex-col gap-[7px] border-0 bg-transparent text-[var(--terminal-muted)] text-[12px]"
              onClick={() => void createTerminal()}
              disabled={!profiles.length}
            >
              <TerminalSquare size={22} />
              <strong>{labels.openTerminal}</strong>
              <span>{labels.usesActiveSessionWorkspace}</span>
            </button>
          ) : (
            visibleTabs.map((tab) => (
              <div
                className={`terminal-host [&.active]:block absolute inset-0 hidden [padding:7px_8px_3px] ${activeId === tab.id ? 'active' : ''}`}
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
          {panelError && (
            <div className="absolute [right:8px] [bottom:8px] max-w-[min(440px,80%)] [border:1px_solid_var(--terminal-error-border)] rounded-[5px] bg-[var(--terminal-error-bg)] [padding:7px_9px] text-[var(--terminal-error-fg)] text-[11px]">
              {panelError}
            </div>
          )}
        </div>
      )}
    </section>
  )
}
