// 路由 → 页面的懒加载映射：每页一个 async 工厂，首屏只加载聊天页。
// 页面组件从 Outlet 上下文取公共能力并显式透传给具体页面，保持
// 页面与壳的依赖边界清晰（页面不直接读全局单例）。
import { useOutletContext } from 'react-router-dom'
import type { AppRouteContext } from './route-context'

// 从 Outlet 上下文取公共能力（壳层注入），各路由组件用它透传 props。
function useAppRouteContext() {
  return useOutletContext<AppRouteContext>()
}

export async function chatRoute() {
  const { ChatPage } = await import('@/features/chat/ChatPage')

  function ChatRoute() {
    const context = useAppRouteContext()
    return (
      <ChatPage
        notify={context.notify}
        browserNotify={context.browserNotify}
        registerPrimaryAction={context.registerPrimaryAction}
        pendingAsset={context.pendingAsset}
        onAssetConsumed={context.onAssetConsumed}
        requestText={context.requestText}
        requestConfirm={context.requestConfirm}
      />
    )
  }

  return { Component: ChatRoute }
}

export async function chatHistoryRoute() {
  const { ChatHistoryPage } = await import('@/features/chat/ChatHistoryPage')

  function ChatHistoryRoute() {
    const context = useAppRouteContext()
    return (
      <ChatHistoryPage
        query={context.query}
        navigate={context.navigate}
        notify={context.notify}
        requestConfirm={context.requestConfirm}
        requestText={context.requestText}
      />
    )
  }

  return { Component: ChatHistoryRoute }
}

export async function assetsRoute() {
  const { AssetsPage } = await import('@/features/assets/AssetsPage')

  function AssetsRoute() {
    const context = useAppRouteContext()
    return (
      <AssetsPage
        query={context.query}
        notify={context.notify}
        registerPrimaryAction={context.registerPrimaryAction}
        requestConfirm={context.requestConfirm}
        onUse={context.onUseAsset}
      />
    )
  }

  return { Component: AssetsRoute }
}

export async function channelsRoute() {
  const { ChannelsPage } = await import('@/features/channels/ChannelsPage')

  function ChannelsRoute() {
    const context = useAppRouteContext()
    return (
      <ChannelsPage
        notify={context.notify}
        registerPrimaryAction={context.registerPrimaryAction}
        requestConfirm={context.requestConfirm}
      />
    )
  }

  return { Component: ChannelsRoute }
}

export async function schedulesRoute() {
  const { SchedulesPage } = await import('@/features/schedules/SchedulesPage')

  function SchedulesRoute() {
    const context = useAppRouteContext()
    return (
      <SchedulesPage
        notify={context.notify}
        registerPrimaryAction={context.registerPrimaryAction}
        requestConfirm={context.requestConfirm}
        openNotificationSettings={context.openNotificationSettings}
      />
    )
  }

  return { Component: SchedulesRoute }
}

export async function configRoute() {
  const { ConfigPage } = await import('@/features/config/ConfigPage')

  function ConfigRoute() {
    const context = useAppRouteContext()
    return (
      <ConfigPage
        notify={context.notify}
        registerPrimaryAction={context.registerPrimaryAction}
        section={context.configSection}
        onBrowserNotificationChange={context.setNotificationSettings}
        requestConfirm={context.requestConfirm}
        update={context.appUpdate}
      />
    )
  }

  return { Component: ConfigRoute }
}

export async function pluginsRoute() {
  const { PluginsPage } = await import('@/features/plugins/PluginsPage')

  function PluginsRoute() {
    const context = useAppRouteContext()
    return (
      <PluginsPage
        query={context.query}
        notify={context.notify}
        registerPrimaryAction={context.registerPrimaryAction}
        onStatusChange={context.setPluginStats}
      />
    )
  }

  return { Component: PluginsRoute }
}

export async function memoryRoute() {
  const { MemoryPage } = await import('@/features/memory/MemoryPage')

  function MemoryRoute() {
    const context = useAppRouteContext()
    return (
      <MemoryPage
        query={context.query}
        notify={context.notify}
        registerPrimaryAction={context.registerPrimaryAction}
        requestConfirm={context.requestConfirm}
      />
    )
  }

  return { Component: MemoryRoute }
}

export async function mcpRoute() {
  const { McpPage } = await import('@/features/workflows/PreviewPages')

  function McpRoute() {
    const context = useAppRouteContext()
    return (
      <McpPage
        query={context.query}
        notify={context.notify}
        registerPrimaryAction={context.registerPrimaryAction}
        requestText={context.requestText}
        requestConfirm={context.requestConfirm}
      />
    )
  }

  return { Component: McpRoute }
}

export async function skillsRoute() {
  const { SkillsPage } = await import('@/features/skills/SkillsPage')

  function SkillsRoute() {
    const context = useAppRouteContext()
    return (
      <SkillsPage
        query={context.query}
        activeSessionId={context.activeSessionId}
        notify={context.notify}
        registerPrimaryAction={context.registerPrimaryAction}
        requestText={context.requestText}
        requestConfirm={context.requestConfirm}
      />
    )
  }

  return { Component: SkillsRoute }
}

export async function workflowsRoute() {
  const { WorkflowsPage } = await import('@/features/workflows/WorkflowsPage')

  function WorkflowsRoute() {
    const context = useAppRouteContext()
    return (
      <WorkflowsPage
        query={context.query}
        notify={context.notify}
        requestConfirm={context.requestConfirm}
      />
    )
  }

  return { Component: WorkflowsRoute }
}

export async function workflowBuilderRoute() {
  const { WorkflowBuilder } = await import('@/features/workflows/WorkflowsPage')

  function WorkflowBuilderRoute() {
    const context = useAppRouteContext()
    return (
      <WorkflowBuilder
        notify={context.notify}
        registerPrimaryAction={context.registerPrimaryAction}
        registerWorkflowActions={context.registerWorkflowActions}
      />
    )
  }

  return { Component: WorkflowBuilderRoute }
}
