import { requestWebPreview } from '@/features/chat/web-preview-events'
import { BackNavigation } from '@/components/layout/BackNavigation'
// 工作台验收使用新外壳的真实路由页面：看板（/board）与会话页（/board/:id 嵌入单会话 ChatPage，
// 不挂载 Dockview）。路由工厂与应用壳同源（route-elements），数据只在 HTTP 边界提供。
import { createRoot } from 'react-dom/client'
import { useState } from 'react'
import { Outlet, RouterProvider, createMemoryRouter } from 'react-router-dom'
import { QueryClientProvider } from '@tanstack/react-query'
import { queryClient } from '@/lib/startup-queries'
import { TooltipProvider } from '@/components/ui/tooltip'
import { boardRoute, sessionRoute } from '@/app/route-elements'
import type { AppRouteContext } from '@/app/route-context'
import { PAGE_PATHS, sessionPath } from '@/app/routes'
import { useClientStore } from '@/stores/client-store'
import { useContextPagesStore } from '@/features/chat/session-context-pages-store'
import { requestSessionSelection } from '@/features/chat/events'
import { TerminalProvider } from '@/features/terminal/public'
import type { DesktopTerminalCreateOptions, DesktopTerminalEvent } from '@/types/update'

const terminalCallbacks = new Map<string, (event: DesktopTerminalEvent) => void>()
const terminalCalls = { created: [] as DesktopTerminalCreateOptions[], closed: [] as string[] }
const terminalBridge = {
  terminalProfiles: () => Promise.resolve([{ id: 'shell', label: 'Fixture shell', default: true }]),
  terminalCreate: (
    options: DesktopTerminalCreateOptions,
    onEvent: (event: DesktopTerminalEvent) => void,
  ) => {
    terminalCalls.created.push(options)
    terminalCallbacks.set(options.terminalId, onEvent)
    return Promise.resolve({
      terminalId: options.terminalId,
      cwd: options.cwd,
      profileId: options.profileId,
    })
  },
  terminalWrite: () => Promise.resolve(),
  terminalResize: () => Promise.resolve(),
  terminalClose: (id: string) => {
    terminalCalls.closed.push(id)
    return Promise.resolve(true)
  },
}
Object.assign(window, {
  terminalFixture: {
    ...terminalCalls,
    emit: (id: string, text: string) =>
      terminalCallbacks.get(id)?.({
        type: 'output',
        terminalId: id,
        data: [...new TextEncoder().encode(text)],
      }),
  },
})

await useClientStore.getState().load()
const [{ Component: BoardRoute }, { Component: SessionRoute }] = await Promise.all([
  boardRoute(),
  sessionRoute(),
])
const pageRequests: string[] = []

// 应用壳通过 Outlet 上下文注入公共能力；这里只提供看板与会话页实际用到的那一部分，
// 跨页跳转（历史会话、配置）记录下来供冒烟断言，不渲染别的页面。
function Shell() {
  const [notice, setNotice] = useState('')
  const context: Partial<AppRouteContext> = {
    query: '',
    activeSessionId: '',
    navigate: (page) => pageRequests.push(page),
    notify: setNotice,
    browserNotify: () => {},
    registerPrimaryAction: () => () => {},
    pendingAsset: null,
    onAssetConsumed: () => {},
    requestText: () => Promise.resolve(null),
    requestConfirm: () => Promise.resolve(true),
  }
  return (
    <div style={{ height: '100vh', display: 'flex', flexDirection: 'column' }}>
      <div role="status" style={{ flexShrink: 0 }}>
        {notice}
      </div>
      <div style={{ display: 'flex', flex: '1 1 0', minHeight: 0, minWidth: 0 }}>
        <Outlet context={context} />
      </div>
    </div>
  )
}

const initialSession = new URL(location.href).searchParams.get('session')
const router = createMemoryRouter(
  [
    {
      element: <Shell />,
      children: [
        { path: PAGE_PATHS.board, element: <BoardRoute /> },
        { path: '/board/:sessionId', element: <SessionRoute /> },
      ],
    },
  ],
  { initialEntries: [initialSession ? sessionPath(initialSession) : PAGE_PATHS.board] },
)

Object.assign(window, {
  workbenchFixture: {
    preview: requestWebPreview,
    revealFiles: (id: string) =>
      useContextPagesStore.getState().update(id, 'files', { type: 'open', kind: 'files' }),
    state: () => useContextPagesStore.getState().sessions,
    // 看板卡片与会话页的“打开会话”都落到路由；这里直接走同一条路由。
    open: (id: string) => router.navigate(sessionPath(id)),
    board: () => router.navigate(PAGE_PATHS.board),
    back: () => router.navigate(-1),
    path: () => router.state.location.pathname,
    // 会话页内部的“打开会话”事件（命令面板、子会话链接等）经 openRoutedSession 交给路由。
    select: (id: string) => requestSessionSelection(id),
    pageRequests: () => [...pageRequests],
  },
})

export function Fixture() {
  return (
    <>
      <BackNavigation />
      <QueryClientProvider client={queryClient}>
        <TooltipProvider>
          <TerminalProvider bridge={terminalBridge}>
            <RouterProvider router={router} />
          </TerminalProvider>
        </TooltipProvider>
      </QueryClientProvider>
    </>
  )
}
createRoot(document.getElementById('root')!).render(<Fixture />)
