// 使用真实面板组件；接口与原生桥接由测试内存状态代替，不访问个人数据。
import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { createPortal } from 'react-dom'
import { MobileShellProvider } from '@/components/layout/MobileShellContext'
import { MobileThreePane } from '@/components/layout/MobileThreePane'
import { MobileContextScreen } from '@/app/mobile/MobileContextScreen'
import {
  normalizeWorkspaceEntries,
  joinWorkspacePath,
  parentWorkspacePath,
} from '@/features/chat/api/workspace-entries'
window.workspaceEntries = { normalizeWorkspaceEntries, joinWorkspacePath, parentWorkspacePath }
window.fileEvidence = { requests: [] }
const resolveSessionCwd = (id) =>
  id === 'slow'
    ? new Promise((resolve) => {
        window.resolveSlowCwd = resolve
      })
    : Promise.resolve(`/workspaces/${id}`)

window.evidence = {
  closeAll: 0,
  created: 0,
  reads: 0,
  approvals: 0,
  reverts: 0,
  revision: 1,
  pluginWrites: 0,
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
const labels = new Proxy({}, { get: (_, key) => String(key) })
export function Fixture() {
  const [pane, setPane] = useState('context')
  const [tab, setTab] = useState('terminal')
  const [mode, setMode] = useState('phone')
  const [session, setSession] = useState('one')
  const [streaming, setStreaming] = useState(false)
  const [mounted, setMounted] = useState(true)
  const [sessionToolsHost, setSessionToolsHost] = useState(null)
  window.controls = { setPane, setTab, setMode, setSession, setStreaming, setMounted }
  if (!mounted) return null
  return (
    <MobileShellProvider
      value={{
        active: true,
        mode,
        pane,
        tab,
        setPane,
        openContext: setTab,
        sessionToolsHost,
        setSessionToolsHost,
      }}
    >
      {sessionToolsHost &&
        createPortal(
          <textarea aria-label="session tool draft" defaultValue="" />,
          sessionToolsHost,
        )}
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
              notify={() => {}}
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
              resolveSessionCwd={resolveSessionCwd}
            />
          }
        />
      </div>
    </MobileShellProvider>
  )
}
createRoot(document.getElementById('root')).render(<Fixture />)
