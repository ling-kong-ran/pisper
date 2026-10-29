// 使用真实面板组件；接口与原生桥接由测试内存状态代替，不访问个人数据。
import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { createPortal } from 'react-dom'
import { MobileShellProvider } from '@/components/layout/MobileShellContext'
import { MobileThreePane } from '@/components/layout/MobileThreePane'
import { QueryClientProvider } from '@tanstack/react-query'
import { queryClient } from '@/lib/startup-queries'
import { SidebarProvider } from '@/components/ui/sidebar'
import { ProjectTaskList } from '@/features/chat/ProjectTaskList'
import { MobileContextScreen } from '@/app/mobile/MobileContextScreen'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { AttachmentPicker } from '@/features/chat/AttachmentPicker'
import { useClientStore } from '@/stores/client-store'
import { TerminalProvider } from '@/features/terminal/public'
window.pickerEvidence = { selected: [], attachments: [], uploads: [], errors: [] }
const resolveSessionCwd = (id) => Promise.resolve(`/workspaces/${id}`)

window.evidence = {
  closed: 0,
  created: 0,
  terminalOptions: [],
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
const terminalBridge = {
  terminalProfiles: () => Promise.resolve([{ id: 'test', label: 'Test shell', default: true }]),
  terminalCreate: (options) => {
    window.evidence.created++
    window.evidence.terminalOptions.push(options)
    return Promise.resolve(options)
  },
  terminalClose: () => {
    window.evidence.closed++
    return Promise.resolve(true)
  },
  terminalResize: () => Promise.resolve(),
  terminalWrite: () => Promise.resolve(),
}
window.pisperDesktop = terminalBridge
const labels = new Proxy({}, { get: (_, key) => String(key) })
export function Fixture() {
  const [workspaceOpen, setWorkspaceOpen] = useState(false)
  const [pane, setPane] = useState('context')
  const [tab, setTab] = useState('terminal')
  const [mode, setMode] = useState('phone')
  const [session, setSession] = useState('one')
  const [streaming, setStreaming] = useState(false)
  const [mounted, setMounted] = useState(true)
  const [sessionToolsHost, setSessionToolsHost] = useState(null)
  window.controls = {
    setPane,
    setTab,
    setMode,
    setSession,
    setStreaming,
    setMounted,
    setWorkspaceOpen,
    setClient: (client) => useClientStore.setState({ client }),
  }
  if (!mounted) return null
  return (
    <TerminalProvider bridge={terminalBridge}>
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
        <WorkspacePicker
          open={workspaceOpen}
          initialPath="/workspaces/initial"
          onOpenChange={setWorkspaceOpen}
          onSelect={(path) => {
            if (window.failWorkspaceSave) throw new Error('workspace_save_failed')
            window.pickerEvidence.selected.push(path)
          }}
        />
        <AttachmentPicker
          cwd="/workspaces/initial"
          selection={{
            attachments: [],
            addFiles: (files) => {
              window.pickerEvidence.uploads.push(...Array.from(files, (file) => file.name))
              return Promise.resolve(true)
            },
            addAttachments: (attachments) => window.pickerEvidence.attachments.push(...attachments),
            setError: (error) => window.pickerEvidence.errors.push(error.message),
          }}
        />
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
    </TerminalProvider>
  )
}
window.sidebarEvidence = { navigations: [], notices: [] }
function SidebarFixture() {
  return (
    <QueryClientProvider client={queryClient}>
      <SidebarProvider mobile={false}>
        <ProjectTaskList
          navigate={(page) => window.sidebarEvidence.navigations.push(page)}
          notify={(message, tone) => window.sidebarEvidence.notices.push({ message, tone })}
          requestText={() => Promise.resolve(null)}
          requestConfirm={() => Promise.resolve(false)}
        />
      </SidebarProvider>
    </QueryClientProvider>
  )
}
// 资产页（982c4705 起）经 React Query 读取列表；应用壳始终提供 QueryClient，夹具同样提供。
createRoot(document.getElementById('root')).render(
  location.search === '?sidebar' ? (
    <SidebarFixture />
  ) : (
    <QueryClientProvider client={queryClient}>
      <Fixture />
    </QueryClientProvider>
  ),
)
