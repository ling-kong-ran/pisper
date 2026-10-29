import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import type { TerminalRegistry } from './terminal-registry'
import type { TerminalRenderer } from './terminal-context'

function terminalTheme() {
  const style = getComputedStyle(document.documentElement)
  const color = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback
  const dark = document.documentElement.dataset.theme === 'dark'
  // 中性色取冷调锌灰；红/绿/黄/蓝/品红读取界面的语义与代码 token，让终端与工具日志、diff 同一套颜色。
  const ansi = dark
    ? {
        black: '#0b0b0d',
        red: '#f0645a',
        green: '#3fb26f',
        yellow: '#e8a23a',
        blue: '#9aa1ff',
        magenta: '#d2a8ff',
        cyan: '#67e8f9',
        white: '#d0d0d6',
        brightBlack: '#8b8b95',
        brightRed: '#fca5a5',
        brightGreen: '#6fcf97',
        brightYellow: '#fde68a',
        brightBlue: '#bfc4ff',
        brightMagenta: '#e9d5ff',
        brightCyan: '#a5f3fc',
        brightWhite: '#ededf0',
      }
    : {
        black: '#17171a',
        red: '#c8281d',
        green: '#1a7f3c',
        yellow: '#a85207',
        blue: '#3d4bef',
        magenta: '#6b2fc0',
        cyan: '#0e7490',
        white: '#67676f',
        brightBlack: '#67676f',
        brightRed: '#dc2626',
        brightGreen: '#16a34a',
        brightYellow: '#ca8a04',
        brightBlue: '#2b38c4',
        brightMagenta: '#9333ea',
        brightCyan: '#0891b2',
        brightWhite: '#36363c',
      }
  return {
    background: color('--terminal-bg', dark ? '#111318' : '#f8fafc'),
    foreground: color('--terminal-fg', dark ? '#e4e4e7' : '#17171a'),
    cursor: color('--terminal-cursor', dark ? '#8d95ff' : '#3d4bef'),
    cursorAccent: color('--terminal-bg', dark ? '#111318' : '#f8fafc'),
    selectionBackground: color('--terminal-selection', dark ? '#2e3266' : '#c7ccfb'),
    ...ansi,
    red: color('--danger', ansi.red),
    green: color('--success', ansi.green),
    yellow: color('--warning-strong', ansi.yellow),
    blue: color('--code-keyword', ansi.blue),
    magenta: color('--code-fn', ansi.magenta),
    brightBlack: color('--text-muted', ansi.brightBlack),
  }
}

// 终端字体跟随界面等宽 token（--font-code-family），保证路径、命令与工具日志同一字形。
function terminalFontFamily() {
  return (
    getComputedStyle(document.documentElement).getPropertyValue('--font-code-family').trim() ||
    'ui-monospace, "SF Mono", "Cascadia Mono", "JetBrains Mono", Menlo, Consolas, "DejaVu Sans Mono", "Liberation Mono", monospace'
  )
}

// 缓存按视图身份隔离。同一 PTY 可在底部面板与任务标签中显示，不搬走另一视图的 DOM。
export function createTerminalRenderer(registry: TerminalRegistry, id: string): TerminalRenderer {
  const element = document.createElement('div')
  element.className = 'terminal-xterm'
  const terminal = new Terminal({
    allowProposedApi: false,
    convertEol: false,
    cursorBlink: true,
    fontFamily: terminalFontFamily(),
    fontSize: 13,
    lineHeight: 1.25,
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
