// 会话检查器的组件标签内容（session-panel 放置，规格 4.4、5.5）：宽度跟随检查器（456 停靠 / 420 覆盖层），
// 高度铺满标签内容区。ctx.session 是当前会话的只读摘要（标题、项目、状态），取自与 ctx.sessions 同一份投影；
// 会话还不在目录里时（刚建、目录未刷新）退回 id + 项目，不阻塞组件渲染。
import { useMemo } from 'react'
import type { ComponentInstance, ComponentRow } from '@/features/custom-ui/custom-ui-api'
import { ComponentHost } from '@/features/custom-ui/native/ComponentHost'
import type { ComponentSessionInfo } from '@/features/custom-ui/native/sdk'
import { useComponentSessionList } from '@/features/custom-ui/native/sessions-source'

export type SessionPanelComponentProps = {
  placed: { instance: ComponentInstance; component: ComponentRow }
  sessionId: string
  cwd: string
  notify(message: string): void
  onOpenSession?(id: string): void
}

export function SessionPanelComponent({
  placed,
  sessionId,
  cwd,
  notify,
  onOpenSession,
}: SessionPanelComponentProps) {
  const sessions = useComponentSessionList()
  const summary = sessions.find((entry) => entry.id === sessionId)
  const title = summary?.title ?? ''
  const status = summary?.status ?? 'idle'
  const session = useMemo<ComponentSessionInfo>(
    () => ({ id: sessionId, title, cwd: summary?.cwd || cwd, status }),
    [cwd, sessionId, status, summary?.cwd, title],
  )
  const { instance, component } = placed
  return (
    <div
      data-session-panel-component={instance.instanceId}
      className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
    >
      <ComponentHost
        // 换会话时整块重挂：组件拿到的 ctx.session 与内部状态都属于当前会话。
        key={sessionId}
        component={component}
        instance={{
          instanceId: instance.instanceId,
          surface: 'session-panel',
          title: instance.title,
          props: instance.props,
        }}
        session={session}
        notify={notify}
        onOpenSession={onOpenSession}
        className="h-full"
      />
    </div>
  )
}
