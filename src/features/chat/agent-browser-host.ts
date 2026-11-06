// Agent 接管右侧浏览器的前端宿主：接收 Runtime 的指令（/api/desktop-browser/host），
// 在原生 WebView 上执行，把结果 POST 回去。只在有原生桥接的桌面主窗口启动。
// WebView 由这里创建并握着，所以用户切走标签或换会话时 Agent 的操作不会中断；
// 页签挂在界面上时 useNativeBrowser 接管它的位置和事件，卸载时只隐藏（ContextPage.agent）。
import { consumeEventStream } from '@/lib/event-stream'
import { resolveWebPreviewUrl } from '@/lib/web-preview'
import type { DesktopBrowserBridge, DesktopBrowserEvent } from '@/types/update'
import {
  clickEffectScript,
  clickScript,
  ensureOverlayScript,
  inspectScript,
  loadStateScript,
  locateScript,
  moveCursorScript,
  overlayVisibleScript,
  parseScriptResult,
  removeOverlayScript,
  typeScript,
} from './agent-browser-scripts'
import { activeBrowserPage, MAX_CONTEXT_PAGES } from './session-context-pages'
import { useContextPagesStore } from './session-context-pages-store'
import { openBrowserNewWindow } from './session-web-preview'
import { attachedNativeBrowsers } from './useNativeBrowser'

const ENDPOINT = '/api/desktop-browser/host'
const SCRIPT_TIMEOUT_MS = 10_000
const LOAD_TIMEOUT_MS = 30_000
const FIND_TIMEOUT_MS = 5_000
const CURSOR_MOVE_MS = 380
const OVERLAY_REFRESH_MS = 700
// 模型思考时光效不该闪灭，但也不能一直亮着。
const OVERLAY_IDLE_MS = 45_000
const MAX_PAGE_TEXT_CHARS = 20_000
const MAX_ELEMENTS = 100
const HIDDEN_BOUNDS = { x: 0, y: 0, width: 1280, height: 800 }

type Command = {
  requestId: string
  sessionId: string
  action: string
  url?: string
  selector?: string
  text?: string
  submit?: boolean
  waitMs?: number
}

type AgentTab = {
  pageId: string
  browserId: string
  cursor: { x: number; y: number }
  lastCommandAt: number
  overlayTimer: ReturnType<typeof setInterval> | null
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

function withTimeout<T>(work: Promise<T>, ms: number, message: string) {
  let timer: ReturnType<typeof setTimeout>
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(message)), ms)
  })
  return Promise.race([work, timeout]).finally(() => clearTimeout(timer))
}

const asString = (value: unknown) => (typeof value === 'string' ? value : '')

function parseCommand(value: unknown): Command | null {
  if (!value || typeof value !== 'object') return null
  const record = value as Record<string, unknown>
  const requestId = asString(record.requestId)
  const sessionId = asString(record.sessionId)
  const action = asString(record.action)
  if (!requestId || !sessionId || !action) return null
  return {
    requestId,
    sessionId,
    action,
    url: asString(record.url),
    selector: asString(record.selector),
    text: asString(record.text),
    submit: record.submit === true,
    waitMs: typeof record.waitMs === 'number' ? record.waitMs : 0,
  }
}

function createHost(browser: DesktopBrowserBridge, overlayLabel: () => string) {
  const tabs = new Map<string, AgentTab>()
  const queues = new Map<string, Promise<unknown>>()
  let nextId = 0

  const store = () => useContextPagesStore.getState()
  const pageOf = (sessionId: string, pageId: string) =>
    store().sessions[sessionId]?.pages.find((page) => page.id === pageId)

  const evaluate = async (browserId: string, script: string) =>
    parseScriptResult(
      await withTimeout(
        browser.evaluate(browserId, script),
        SCRIPT_TIMEOUT_MS,
        'The page did not respond in time.',
      ),
    )

  const stopOverlayTimer = (tab: AgentTab) => {
    if (tab.overlayTimer) clearInterval(tab.overlayTimer)
    tab.overlayTimer = null
  }

  const showOverlay = (tab: AgentTab) =>
    evaluate(tab.browserId, ensureOverlayScript({ ...tab.cursor, label: overlayLabel() }))

  // 导航后是新文档，覆盖层没了要补；停下来一段时间就收起。
  const keepOverlay = (tab: AgentTab) => {
    tab.lastCommandAt = Date.now()
    if (tab.overlayTimer) return
    tab.overlayTimer = setInterval(() => {
      if (Date.now() - tab.lastCommandAt > OVERLAY_IDLE_MS) {
        stopOverlayTimer(tab)
        void evaluate(tab.browserId, removeOverlayScript()).catch(() => {})
        return
      }
      void showOverlay(tab).catch(() => {})
    }, OVERLAY_REFRESH_MS)
  }

  // Agent 不再用这一页：光效收起，页签留给用户。界面上没挂着的 WebView 没人管了，要关掉。
  const release = (sessionId: string) => {
    const tab = tabs.get(sessionId)
    if (!tab) return
    tabs.delete(sessionId)
    stopOverlayTimer(tab)
    void evaluate(tab.browserId, removeOverlayScript()).catch(() => {})
    const page = pageOf(sessionId, tab.pageId)
    if (page) store().update(sessionId, 'browser', { type: 'browser', id: page.id, agent: false })
    if (attachedNativeBrowsers.has(tab.browserId)) return
    void browser.close(tab.browserId).catch(() => {})
    if (page)
      store().update(sessionId, 'browser', { type: 'resource', id: page.id, resourceId: '' })
  }

  // 用户关了 Agent 的页签：WebView 跟着关，下一条指令会报“没有打开的页面”。
  // 页面里的链接开了新窗口：Agent 跟过去，和人点链接后看新页签一样。
  const unsubscribe = useContextPagesStore.subscribe((state) => {
    for (const [sessionId, tab] of tabs) {
      const pages = state.sessions[sessionId]
      const page = pages?.pages.find((entry) => entry.id === tab.pageId)
      if (!page || page.resourceId !== tab.browserId) {
        tabs.delete(sessionId)
        stopOverlayTimer(tab)
        void browser.close(tab.browserId).catch(() => {})
        continue
      }
      const active = pages && activeBrowserPage(pages)
      if (active && active.id !== tab.pageId && active.resourceId.startsWith('native-popup-'))
        followPopup(sessionId, tab, active.id, active.resourceId)
    }
  })

  const followPopup = (sessionId: string, tab: AgentTab, pageId: string, browserId: string) => {
    stopOverlayTimer(tab)
    void evaluate(tab.browserId, removeOverlayScript()).catch(() => {})
    const opener = tab.pageId
    tab.pageId = pageId
    const previousBrowser = tab.browserId
    tab.browserId = browserId
    store().update(sessionId, 'browser', { type: 'browser', id: opener, agent: false })
    store().update(sessionId, 'browser', { type: 'browser', id: pageId, agent: true })
    // 原来的页签没挂在界面上时，它的 WebView 已经没人管了。
    if (!attachedNativeBrowsers.has(previousBrowser)) {
      void browser.close(previousBrowser).catch(() => {})
      store().update(sessionId, 'browser', { type: 'resource', id: opener, resourceId: '' })
    }
    // 弹窗页签没挂在界面上：事件先接到这里，等它挂上去时 Hook 再接管。
    if (!attachedNativeBrowsers.has(browserId))
      void browser.adopt(browserId, forwardEvents(sessionId, pageId)).catch(() => {})
    keepOverlay(tab)
  }

  // 页签没挂在界面上时的事件接收方，挂上去后 Hook 会换成自己的。
  const forwardEvents = (sessionId: string, pageId: string) => (event: DesktopBrowserEvent) => {
    if (event.type === 'newWindow') openBrowserNewWindow(sessionId, event.url, event.browserId)
    else if (event.type === 'title')
      store().update(sessionId, 'browser', { type: 'browser', id: pageId, title: event.title })
    else if (event.type === 'closed')
      store().update(sessionId, 'browser', { type: 'close', id: pageId })
    else
      store().update(sessionId, 'browser', {
        type: 'browser',
        id: pageId,
        url: event.url,
        draft: event.url,
      })
  }

  const syncPage = (sessionId: string, tab: AgentTab, info: Record<string, unknown>) => {
    const url = asString(info.url)
    const page = pageOf(sessionId, tab.pageId)
    if (!page || !url) return
    attachedNativeBrowsers.get(tab.browserId)?.noteUrl(url)
    store().update(sessionId, 'browser', {
      type: 'browser',
      id: page.id,
      url,
      ...(page.draft === page.url ? { draft: url } : {}),
      title: asString(info.title) || page.title,
    })
  }

  const waitForLoad = async (browserId: string, previousUrl: string) => {
    const startedAt = Date.now()
    await sleep(200)
    for (;;) {
      const state = await evaluate(browserId, loadStateScript()).catch(() => null)
      const elapsed = Date.now() - startedAt
      // 导航刚发出去时查到的可能还是旧文档，地址变了或等过一会儿才算数。
      if (state?.ready === true && (state.url !== previousUrl || elapsed > 1_500)) return state
      if (elapsed > LOAD_TIMEOUT_MS) {
        if (state) return state
        throw new Error('The page did not finish loading in time.')
      }
      await sleep(200)
    }
  }

  const openTab = async (sessionId: string, url: string) => {
    const existing = tabs.get(sessionId)
    if (existing && pageOf(sessionId, existing.pageId)) {
      const before = await evaluate(existing.browserId, loadStateScript()).catch(() => null)
      attachedNativeBrowsers.get(existing.browserId)?.noteUrl(url)
      await browser.navigate(existing.browserId, url)
      store().update(sessionId, 'browser', { type: 'select', id: existing.pageId })
      store().update(sessionId, 'browser', {
        type: 'browser',
        id: existing.pageId,
        url,
        draft: url,
      })
      return { tab: existing, previousUrl: asString(before?.url) }
    }
    const browserId = `agent-browser-${++nextId}`
    const pages = store().sessions[sessionId]
    const current = pages && activeBrowserPage(pages)
    // 当前是没用过的空白页签就用它，否则开一个，不占用用户正在看的网页。
    const reusable = current && !current.url && !current.resourceId ? current : null
    if (!reusable && (pages?.pages.length ?? 0) >= MAX_CONTEXT_PAGES)
      throw new Error('Too many pages are open in this session. Ask the user to close one.')
    const placeholder = { current: '' }
    const opened = await browser.open({ browserId, url, bounds: HIDDEN_BOUNDS }, (event) => {
      if (placeholder.current) forwardEvents(sessionId, placeholder.current)(event)
    })
    if (opened.status !== 'opened') throw new Error('The desktop browser is not available.')
    let pageId = reusable?.id ?? ''
    if (reusable) {
      store().update(sessionId, 'browser', { type: 'select', id: reusable.id })
    } else {
      const result = store().update(sessionId, 'browser', {
        type: 'open',
        kind: 'browser',
        resourceId: browserId,
      })
      const next = store().sessions[sessionId]
      pageId = (result !== 'limit' && next && activeBrowserPage(next)?.id) || ''
      if (!pageId) {
        void browser.close(browserId).catch(() => {})
        throw new Error('Too many pages are open in this session. Ask the user to close one.')
      }
    }
    placeholder.current = pageId
    // 先标记再写 resourceId：Hook 一看到 resourceId 就会接管，那时它得知道这是 Agent 的页签。
    store().update(sessionId, 'browser', {
      type: 'browser',
      id: pageId,
      url,
      draft: url,
      agent: true,
    })
    store().update(sessionId, 'browser', { type: 'resource', id: pageId, resourceId: browserId })
    const tab: AgentTab = {
      pageId,
      browserId,
      cursor: { x: 48, y: 48 },
      lastCommandAt: Date.now(),
      overlayTimer: null,
    }
    tabs.set(sessionId, tab)
    return { tab, previousUrl: '' }
  }

  const requireTab = (sessionId: string) => {
    const tab = tabs.get(sessionId)
    if (!tab || !pageOf(sessionId, tab.pageId))
      throw new Error('No page is open. Use the open action first.')
    return tab
  }

  // 等元素出现，把指针移过去：用户看到的是 Agent 先看向目标再动手。
  const moveTo = async (tab: AgentTab, selector: string) => {
    const startedAt = Date.now()
    for (;;) {
      const found = await evaluate(tab.browserId, locateScript(selector))
      if (found.found === true) {
        const point = { x: Number(found.x) || 0, y: Number(found.y) || 0 }
        await showOverlay(tab).catch(() => {})
        await evaluate(tab.browserId, moveCursorScript(point.x, point.y, CURSOR_MOVE_MS)).catch(
          () => {},
        )
        tab.cursor = point
        await sleep(CURSOR_MOVE_MS)
        return point
      }
      if (Date.now() - startedAt > FIND_TIMEOUT_MS)
        throw new Error(`No visible element matches ${selector}. Inspect the page again.`)
      await sleep(200)
    }
  }

  const settle = async (tab: AgentTab, waitMs: number) => {
    await sleep(Math.min(15_000, Math.max(250, waitMs)))
    return (await evaluate(tab.browserId, loadStateScript()).catch(() => null)) ?? {}
  }

  const run = async (command: Command): Promise<Record<string, unknown>> => {
    const { sessionId, action } = command
    if (action === 'open') {
      const url = resolveWebPreviewUrl(command.url ?? '', window.location.href)
      if (!url) throw new Error('Browser automation only supports http and https URLs.')
      const { tab, previousUrl } = await openTab(sessionId, url.href)
      keepOverlay(tab)
      const state = await waitForLoad(tab.browserId, previousUrl)
      await showOverlay(tab).catch(() => {})
      if (command.waitMs) await sleep(Math.min(15_000, command.waitMs))
      syncPage(sessionId, tab, state)
      return state
    }
    if (action === 'close') {
      release(sessionId)
      return {}
    }
    const tab = requireTab(sessionId)
    keepOverlay(tab)
    await showOverlay(tab).catch(() => {})
    if (action === 'inspect') {
      const snapshot = await evaluate(
        tab.browserId,
        inspectScript({ maxElements: MAX_ELEMENTS, maxText: MAX_PAGE_TEXT_CHARS }),
      )
      syncPage(sessionId, tab, snapshot)
      return snapshot
    }
    if (action === 'click') {
      const selector = command.selector ?? ''
      const point = await moveTo(tab, selector)
      await evaluate(tab.browserId, clickEffectScript(point.x, point.y)).catch(() => {})
      await evaluate(tab.browserId, clickScript(selector))
      const state = await settle(tab, command.waitMs ?? 0)
      syncPage(sessionId, tab, state)
      return state
    }
    if (action === 'type') {
      const selector = command.selector ?? ''
      await moveTo(tab, selector)
      await evaluate(
        tab.browserId,
        typeScript(selector, command.text ?? '', command.submit === true),
      )
      const state = await settle(tab, command.waitMs ?? 0)
      syncPage(sessionId, tab, state)
      return state
    }
    if (action === 'wait') {
      const waitMs = Math.min(15_000, Math.max(0, command.waitMs || 1_000))
      await sleep(waitMs)
      const state = await evaluate(tab.browserId, loadStateScript())
      syncPage(sessionId, tab, state)
      return { ...state, waitMs }
    }
    if (action === 'screenshot') {
      await evaluate(tab.browserId, overlayVisibleScript(false)).catch(() => {})
      try {
        // 等一帧，让隐藏生效后再截。
        await sleep(60)
        const imageBase64 = await withTimeout(
          browser.capture(tab.browserId),
          SCRIPT_TIMEOUT_MS,
          'The screenshot did not finish in time.',
        )
        const state = await evaluate(tab.browserId, loadStateScript())
        return { ...state, imageBase64 }
      } finally {
        await evaluate(tab.browserId, overlayVisibleScript(true)).catch(() => {})
      }
    }
    throw new Error(`Unsupported browser action: ${action}`)
  }

  const report = (requestId: string, payload: Record<string, unknown>) =>
    fetch(`${ENDPOINT}/${encodeURIComponent(requestId)}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    }).catch(() => {})

  // 同一会话的指令按顺序执行。
  const enqueue = (command: Command) => {
    const previous = queues.get(command.sessionId) ?? Promise.resolve()
    const next = previous.then(async () => {
      try {
        await report(command.requestId, { ok: true, result: await run(command) })
      } catch (error) {
        await report(command.requestId, {
          ok: false,
          error: error instanceof Error ? error.message : String(error),
        })
      }
    })
    queues.set(command.sessionId, next)
    void next.finally(() => {
      if (queues.get(command.sessionId) === next) queues.delete(command.sessionId)
    })
  }

  // 用户停止了这一轮：光效立刻收起，页面和页签都留着，模型下一轮还能接着用。
  const cancel = (sessionId: string) => {
    const tab = tabs.get(sessionId)
    if (!tab) return
    stopOverlayTimer(tab)
    void evaluate(tab.browserId, removeOverlayScript()).catch(() => {})
  }

  return {
    enqueue,
    cancel,
    release,
    dispose() {
      unsubscribe()
      for (const sessionId of [...tabs.keys()]) release(sessionId)
    },
  }
}

// 真的能开原生 WebView 才连上来，否则 Runtime 会放弃能用的无头浏览器：
// Linux 也有桥接但 open 不支持，远程工作区窗口没有这组命令的权限。
async function nativeBrowserUsable(browser: DesktopBrowserBridge) {
  const platform = await window.pisperDesktop?.getAppInfo().then(
    (info) => info.platform,
    () => '',
  )
  if (platform !== 'darwin' && platform !== 'win32') return false
  return browser.close('agent-browser-probe').then(
    () => true,
    () => false,
  )
}

// 返回停止函数。
export function startAgentBrowserHost(overlayLabel: () => string) {
  const browser = window.pisperDesktop?.browser
  if (!browser) return () => {}
  const host = createHost(browser, overlayLabel)
  const controller = new AbortController()
  const connect = async () => {
    if (!(await nativeBrowserUsable(browser))) return
    let delay = 1_000
    while (!controller.signal.aborted) {
      try {
        const response = await fetch(ENDPOINT, { signal: controller.signal })
        await consumeEventStream<unknown>(response, (event, data) => {
          delay = 1_000
          if (event === 'command') {
            const command = parseCommand(data)
            if (command) host.enqueue(command)
            return
          }
          const sessionId = asString((data as { sessionId?: unknown } | null)?.sessionId)
          if (event === 'cancel' && sessionId) host.cancel(sessionId)
          if (event === 'release' && sessionId) host.release(sessionId)
        })
      } catch {
        // Runtime 重启或还没就绪：退避后重连。
      }
      if (controller.signal.aborted) return
      await sleep(delay)
      delay = Math.min(30_000, delay * 2)
    }
  }
  void connect()
  return () => {
    controller.abort()
    host.dispose()
  }
}
