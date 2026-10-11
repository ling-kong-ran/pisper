// 使用真实面板组件；接口与原生桥接由测试内存状态代替，不访问个人数据。
import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { SessionWorkflowRuns } from '@/features/chat/components/session/SessionWorkflowRuns'
import { MobileThreePane } from '@/components/layout/MobileThreePane'
import { MobileContextScreen } from '@/app/mobile/MobileContextScreen'

window.evidence = {
  closeAll: 0,
  created: 0,
  reads: 0,
  approvals: 0,
  reverts: 0,
  revision: 1,
  pluginWrites: 0,
  reveals: [],
  notices: [],
  cwd: '/fixture',
}
window.pluginData = {
  plugins: [
    {
      id: 'fixture',
      name: 'Fixture',
      source: 'builtin',
      builtIn: true,
      enabled: true,
      capabilities: [
        {
          name: 'fixture_tool',
          label: 'Fixture tool',
          enabled: true,
          risk: 'low',
          category: 'test',
          description: 'Fixture',
        },
      ],
    },
  ],
  enabledTools: ['fixture_tool'],
  webSearch: { provider: 'bing', language: 'auto', safeSearch: 1, maxResults: 8 },
  piExtensions: {},
  computerUseEnabled: false,
  changes: [],
  presets: {},
}
window.pisperDesktop = {
  terminalProfiles: () => Promise.resolve([{ id: 'test', label: 'Test shell', default: true }]),
  terminalCreate: (options) => {
    window.evidence.created++
    return Promise.resolve(options)
  },
  terminalCloseAll: () => {
    window.evidence.closeAll++
    return Promise.resolve()
  },
  terminalResize: () => Promise.resolve(),
  terminalWrite: () => Promise.resolve(),
  onTerminalEvent: () => () => {},
}
const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
const labels = new Proxy({}, { get: (_, key) => String(key) })
export function Fixture() {
  const [pane, setPane] = useState('context')
  const [tab, setTab] = useState('terminal')
  const [mode, setMode] = useState('phone')
  const [session, setSession] = useState('one')
  const [streaming, setStreaming] = useState(false)
  const [mounted, setMounted] = useState(true)
  window.controls = { setPane, setTab, setMode, setSession, setStreaming, setMounted }
  if (!mounted) return null
  if (mode === 'workflow')
    return (
      <QueryClientProvider client={queryClient}>
        <SessionWorkflowRuns sessionId={session} />
      </QueryClientProvider>
    )
  return (
    <div style={{ height: '100dvh', display: 'flex' }}>
      <MobileThreePane
        enabled
        mode={mode}
        pane={pane}
        onPaneChange={setPane}
        sessions={<div>sessions</div>}
        chat={
          <div>
            chat
            <textarea aria-label="composer" />
            <div
              aria-label="horizontal-content"
              style={{ width: 240, overflowX: 'auto', whiteSpace: 'nowrap' }}
            >
              <div style={{ width: 1000 }}>Horizontal content</div>
            </div>
            <div
              aria-label="vertical-content"
              style={{ width: 280, height: 160, overflowY: 'auto' }}
            >
              <div style={{ height: 700 }}>Vertical content</div>
            </div>
            <div data-swipe-ignore aria-label="embedded-control" style={{ width: 280, height: 40 }}>
              Embedded control
            </div>
            <div
              data-fixed-probe
              style={{ position: 'fixed', right: 0, bottom: 0, width: 10, height: 10 }}
            />
          </div>
        }
        context={
          <MobileContextScreen
            tab={tab}
            onTabChange={setTab}
            visible={mode === 'pad' || pane === 'context'}
            sessionStreaming={streaming}
            onCloseTerminal={() => setPane('chat')}
            onOpenChat={() => setStreaming(false)}
            activeSessionId={session}
            query=""
            notify={(message, tone) => window.evidence.notices.push({ message, tone })}
            requestConfirm={() =>
              new Promise((resolve) => {
                window.confirmPending = true
                window.resolveConfirm = resolve
              })
            }
            requestText={() => Promise.resolve(null)}
            onUseAsset={() => {}}
            terminalSupported
            terminalLabels={labels}
            resolveSessionCwd={() => {
              if (window.evidence.holdCwd) {
                window.evidence.cwdRequests = (window.evidence.cwdRequests || 0) + 1
                return new Promise((resolve, reject) => {
                  window.releaseCwd = resolve
                  window.rejectCwd = reject
                })
              }
              return Promise.resolve(window.evidence.cwd)
            }}
          />
        }
      />
    </div>
  )
}
createRoot(document.getElementById('root')).render(<Fixture />)
