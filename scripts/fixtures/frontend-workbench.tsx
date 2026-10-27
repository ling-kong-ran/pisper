import { requestWebPreview } from '@/features/chat/web-preview-events'
import { BackNavigation } from '@/components/layout/BackNavigation'
// 工作台验收使用真实 ChatPage 和 Dockview；数据只在 HTTP 边界提供。
import { createRoot } from 'react-dom/client'
import { useState } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { QueryClientProvider } from '@tanstack/react-query'
import { queryClient } from '@/lib/startup-queries'
import { TooltipProvider } from '@/components/ui/tooltip'
import { ChatPage } from '@/features/chat/ChatPage'
import { useClientStore } from '@/stores/client-store'
import { useContextPagesStore } from '@/features/chat/session-context-pages-store'
import { requestSessionSelection } from '@/features/chat/events'
import type { SessionOpenDisposition } from '@/features/chat/dock-layout'
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
Object.assign(window, {
  workbenchFixture: {
    preview: requestWebPreview,
    revealFiles: (id: string) =>
      useContextPagesStore.getState().update(id, 'files', { type: 'open', kind: 'files' }),
    state: () => useContextPagesStore.getState().sessions,
    open: (id: string, disposition: SessionOpenDisposition = 'open') =>
      requestSessionSelection(id, disposition),
  },
})
export function Fixture() {
  const [notice, setNotice] = useState('')
  return (
    <MemoryRouter>
      <BackNavigation />
      <QueryClientProvider client={queryClient}>
        <TooltipProvider>
          <TerminalProvider bridge={terminalBridge}>
            <div style={{ height: '100vh', display: 'flex', flexDirection: 'column' }}>
              <div role="status" style={{ flexShrink: 0 }}>
                {notice}
              </div>
              <ChatPage
                notify={setNotice}
                navigate={() => {}}
                registerPrimaryAction={() => () => {}}
                reportActiveSessionRun={() => {}}
                pendingAsset={null}
                onAssetConsumed={() => {}}
                requestText={() => Promise.resolve(null)}
                requestConfirm={() => Promise.resolve(true)}
              />
            </div>
          </TerminalProvider>
        </TooltipProvider>
      </QueryClientProvider>
    </MemoryRouter>
  )
}
createRoot(document.getElementById('root')!).render(<Fixture />)
