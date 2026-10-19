import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import type { TerminalRegistry } from './terminal-registry'
import type { TerminalRenderer } from './terminal-context'

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

// 缓存按视图身份隔离。同一 PTY 可在底部面板与任务标签中显示，不搬走另一视图的 DOM。
export function createTerminalRenderer(registry: TerminalRegistry, id: string): TerminalRenderer {
  const element = document.createElement('div')
  element.className = 'terminal-xterm'
  const terminal = new Terminal({
    allowProposedApi: false,
    convertEol: false,
    cursorBlink: true,
    fontFamily: "'Cascadia Mono', 'SFMono-Regular', Consolas, monospace",
    fontSize: 13,
    lineHeight: 1.2,
    scrollback: 8000,
    theme: terminalTheme(),
  })
  const fit = new FitAddon()
  terminal.loadAddon(fit)
  terminal.loadAddon(
    new WebLinksAddon((_event, uri) => window.open(uri, '_blank', 'noopener,noreferrer')),
  )
  let host: HTMLElement | null = null
  let opened = false,
    disposed = false
  let sequence = 0
  let frame: number | null = null
  let report: (message: string) => void = () => {}
  const replay = () => {
    for (const chunk of registry.outputChunks(id))
      if (chunk.sequence > sequence) {
        terminal.write(chunk.data)
        sequence = chunk.sequence
      }
  }
  const resize = () => {
    if (!host?.isConnected || !host.clientWidth || !host.clientHeight || disposed) return
    const viewport = terminal.buffer.active.viewportY
    const following = viewport >= terminal.buffer.active.baseY
    fit.fit()
    if (!following) terminal.scrollToLine(viewport)
    terminal.refresh(0, terminal.rows - 1)
    void registry.resize(id, terminal.cols, terminal.rows).catch((error) => report(String(error)))
  }
  const schedule = () => {
    if (frame === null)
      frame = requestAnimationFrame(() => {
        frame = null
        resize()
      })
  }
  const observer = new ResizeObserver(schedule)
  const themeObserver = new MutationObserver(() => {
    terminal.options.theme = terminalTheme()
  })
  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['data-theme'],
  })
  const unsubscribe = registry.subscribeEvents((event) => {
    if (event.terminalId === id && event.type === 'output') replay()
  })
  const disposables = [
    terminal.onData((data) => {
      void registry
        .write(id, new TextEncoder().encode(data))
        .catch((error) => report(String(error)))
    }),
    terminal.onBinary((data) => {
      void registry
        .write(
          id,
          Uint8Array.from(data, (char) => char.charCodeAt(0)),
        )
        .catch((error) => report(String(error)))
    }),
  ]
  replay()
  return {
    attach(next, onError) {
      if (disposed) return
      host = next
      report = onError || (() => {})
      host.append(element)
      if (!opened) {
        terminal.open(element)
        opened = true
      }
      observer.disconnect()
      observer.observe(host)
      schedule()
    },
    detach() {
      observer.disconnect()
      element.remove()
      host = null
      report = () => {}
      if (frame !== null) {
        cancelAnimationFrame(frame)
        frame = null
      }
    },
    dispose() {
      if (disposed) return
      disposed = true
      observer.disconnect()
      themeObserver.disconnect()
      unsubscribe()
      for (const disposable of disposables) disposable.dispose()
      if (frame !== null) cancelAnimationFrame(frame)
      terminal.dispose()
      element.remove()
    },
  }
}
