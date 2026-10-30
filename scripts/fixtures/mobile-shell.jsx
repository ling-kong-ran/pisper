// 手机外壳冒烟的组件夹具（第 4 期重写）：只挂载新外壳仍在使用的真实组件——
// 看板“新建项目…”（useProjectCommands → 系统目录对话框 / 手动填写的 WorkspacePicker）、
// 工作目录选择器（含 Android SAF 导入）、输入框的附件选择器、检查器底部表单里的“改动”
// （InspectorChanges）与“终端”（TaskTerminalView）。接口与原生桥接由内存状态代替，不访问个人数据。
// 休眠的旧三栏外壳（MobileThreePane / MobileContextScreen / ProjectTaskList）不再挂载；
// 整页的手机外壳、平板与窄桌面在 smoke-mobile-shell.mjs 里用真实生产构建验证。
import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClientProvider } from '@tanstack/react-query'
import { queryClient } from '@/lib/startup-queries'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { AttachmentPicker } from '@/features/chat/AttachmentPicker'
import { InspectorChanges } from '@/features/chat/InspectorChanges'
import { useRegisteredProjectsStore } from '@/features/chat/registered-projects-store'
import { useLaunchDefaultsStore } from '@/features/board/launch-defaults-store'
import { useProjectCommands } from '@/features/board/useProjectCommands'
import { TaskTerminalView } from '@/features/terminal/TaskTerminalView'
import { TerminalProvider } from '@/features/terminal/public'
import { useClientStore } from '@/stores/client-store'

window.pickerEvidence = { selected: [], attachments: [], uploads: [], errors: [] }
window.evidence = {
  closed: 0,
  created: 0,
  terminalOptions: [],
  reads: 0,
  approvals: 0,
  reverts: 0,
  revision: 1,
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

// 检查器底部表单的内容：表单关闭即卸载（与手机会话页一致），终端编号由外层持有，重新打开后接回同一终端。
function InspectorSheetContent({ tab, streaming, terminalId, onTerminalCreated }) {
  if (tab === 'terminal')
    return (
      <div style={{ height: 320, display: 'flex' }}>
        <TaskTerminalView
          sessionId="one"
          cwd="/workspaces/one"
          terminalId={terminalId}
          onCreated={onTerminalCreated}
        />
      </div>
    )
  return (
    <InspectorChanges
      sessionId="one"
      cwd="/workspaces/one"
      streaming={streaming}
      scope="session"
      onScopeChange={() => {}}
      vcsAvailable={false}
      view={{}}
      onViewChange={() => {}}
      requestConfirm={() =>
        new Promise((resolve) => {
          window.confirmPending = true
          window.resolveConfirm = resolve
        })
      }
    />
  )
}

export function Fixture() {
  const [workspaceOpen, setWorkspaceOpen] = useState(false)
  const [sheet, setSheet] = useState(null)
  const [streaming, setStreaming] = useState(false)
  const [mounted, setMounted] = useState(true)
  const [terminalId, setTerminalId] = useState(undefined)
  window.controls = {
    setSheet,
    setStreaming,
    setMounted,
    setWorkspaceOpen,
    setClient: (client) => useClientStore.setState({ client }),
  }
  if (!mounted) return null
  return (
    <TerminalProvider bridge={terminalBridge}>
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
      {sheet && (
        <section aria-label="inspector sheet" data-fixture-sheet={sheet}>
          <InspectorSheetContent
            tab={sheet}
            streaming={streaming}
            terminalId={terminalId}
            onTerminalCreated={setTerminalId}
          />
        </section>
      )}
    </TerminalProvider>
  )
}

window.projectEvidence = { chosen: 0, notices: [] }
window.readProjectState = () => ({
  registered: useRegisteredProjectsStore.getState().projects,
  launchCwd: useLaunchDefaultsStore.getState().cwd,
})
// 看板“新建项目…”：系统目录对话框取消不弹手动对话框；选中后登记并选为派发目标；
// 桥接报错给出错误提示；没有系统对话框时退回手动填写的 WorkspacePicker。
function ProjectFixture() {
  const projects = useProjectCommands({
    notify: (message, tone) => window.projectEvidence.notices.push({ message, tone }),
    requestConfirm: () => Promise.resolve(false),
    requestText: () => Promise.resolve(null),
    onProjectChosen: () => window.projectEvidence.chosen++,
    announceChoice: true,
  })
  return (
    <>
      <button type="button" disabled={projects.picking} onClick={() => void projects.newProject()}>
        board:project.new
      </button>
      {projects.picker.mounted && (
        <WorkspacePicker
          open={projects.picker.open}
          initialPath=""
          description={projects.picker.description}
          onOpenChange={projects.picker.onOpenChange}
          onSelect={projects.picker.onSelect}
          onCloseAutoFocus={projects.picker.onCloseAutoFocus}
        />
      )}
    </>
  )
}

createRoot(document.getElementById('root')).render(
  <QueryClientProvider client={queryClient}>
    {location.search === '?project' ? <ProjectFixture /> : <Fixture />}
  </QueryClientProvider>,
)
