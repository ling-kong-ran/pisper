// 路由 → 页面的懒加载映射：每页一个 async 工厂，首屏只加载看板。
// 页面组件从 Outlet 上下文取公共能力并显式透传给具体页面，保持
// 页面与壳的依赖边界清晰（页面不直接读全局单例）。
import { useCallback, useRef } from 'react'
import { useNavigate, useOutletContext, useParams } from 'react-router-dom'
import {
  ensureBoardMessages,
  ensureChannelsMessages,
  ensureCustomUiMessages,
  ensureMcpMessages,
  ensureWorkflowMessages,
} from './i18n'
import { ACTIVITY_NAVIGATION_STATE } from './settings-navigation'
import type { AppRouteContext } from './route-context'
import { NEW_SESSION_ROUTE_ID, PAGE_PATHS, pagePath, sessionPath } from './routes'

// 从 Outlet 上下文取公共能力（壳层注入），各路由组件用它透传 props。
function useAppRouteContext() {
  return useOutletContext<AppRouteContext>()
}

export async function boardRoute() {
  const [{ BoardPage }] = await Promise.all([
    import('@/features/board/BoardPage'),
    ensureBoardMessages(),
  ])

  // 复制会话链接：沿用当前地址的协议、主机与路由形式（哈希路由时链接也带 #）。
  function sessionHref(id: string) {
    const { origin, pathname, search, hash } = window.location
    return hash.startsWith('#/')
      ? `${origin}${pathname}${search}#${sessionPath(id)}`
      : `${origin}${sessionPath(id)}`
  }

  function BoardRoute() {
    const context = useAppRouteContext()
    const navigate = useNavigate()
    return (
      <BoardPage
        onOpenSession={(id) => navigate(sessionPath(id))}
        onOpenHistory={() => context.navigate('chatHistory')}
        onConfigureModels={() => navigate('/config/models')}
        sessionHref={sessionHref}
        notify={context.notify}
      />
    )
  }

  return { Component: BoardRoute }
}

// 会话页：在新外壳里嵌入现有单会话视图（不挂载侧栏、活动栏和 Dockview 多标签）。
// URL 里的 sessionId 是选中依据；聊天页内部打开会话时回调这里切换路由，'new' 建好后以 replace 换成真实 id。
export async function sessionRoute() {
  const [{ ChatPage }, { SessionReturnBar }] = await Promise.all([
    import('@/features/chat/ChatPage'),
    import('@/app/SessionReturnBar'),
  ])

  function SessionRoute() {
    const context = useAppRouteContext()
    const navigate = useNavigate()
    const { sessionId = '' } = useParams()
    const currentRef = useRef(sessionId)
    currentRef.current = sessionId
    const openSession = useCallback(
      (id: string) => {
        if (!id || id === currentRef.current) return
        navigate(sessionPath(id), { replace: currentRef.current === NEW_SESSION_ROUTE_ID })
      },
      [navigate],
    )
    return (
      <div className="flex min-h-0 min-w-0 flex-1">
        <ChatPage
          notify={context.notify}
          navigate={context.navigate}
          browserNotify={context.browserNotify}
          registerPrimaryAction={context.registerPrimaryAction}
          pendingAsset={context.pendingAsset}
          onAssetConsumed={context.onAssetConsumed}
          requestText={context.requestText}
          requestConfirm={context.requestConfirm}
          routeSessionId={sessionId}
          onRouteSessionChange={openSession}
          // 返回入口放进会话顶栏最左侧，不再单独占一行（标题栏 + 会话顶栏 96px）。
          routeLeading={<SessionReturnBar onBack={() => navigate(PAGE_PATHS.board)} />}
        />
      </div>
    )
  }

  return { Component: SessionRoute }
}

export async function moreRoute() {
  const { MorePage } = await import('@/app/MorePage')

  function MoreRoute() {
    const context = useAppRouteContext()
    const navigate = useNavigate()
    return (
      <MorePage
        onOpen={(destination) => {
          // 从“更多”进入的功能页写入来源，插件、星忆等双归属页面据此点亮“更多”并显示“‹ 更多”。
          if (destination.type === 'page')
            navigate(pagePath(destination.id), { state: ACTIVITY_NAVIGATION_STATE })
          else if (destination.type === 'config') context.setConfigSection(destination.id)
          else
            navigate(`/tools/components/${encodeURIComponent(destination.id)}`, {
              state: ACTIVITY_NAVIGATION_STATE,
            })
        }}
      />
    )
  }

  return { Component: MoreRoute }
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
        activeSessionId={context.activeSessionId}
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
  const [{ ChannelsPage }] = await Promise.all([
    import('@/features/channels/ChannelsPage'),
    ensureChannelsMessages(),
  ])

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
  const [{ ConfigPage }, { ChatAppearancePage }] = await Promise.all([
    import('@/features/config/ConfigPage'),
    import('@/app/ChatAppearancePage'),
  ])

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
        renderInterfaceSettings={(appearance) => (
          <ChatAppearancePage appearance={appearance} notify={context.notify} />
        )}
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
        requestText={context.requestText}
        requestConfirm={context.requestConfirm}
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
  const [{ McpPage }] = await Promise.all([import('@/features/mcp/McpPage'), ensureMcpMessages()])

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

// “组件”标签页：第 1 期在新外壳里渲染现有的独立组件管理视图（原 设置 → 界面 → 独立组件），
// 第 5 期换成三栏组件工作室。
export async function componentsRoute() {
  const [{ CustomUiPage }] = await Promise.all([
    import('@/features/custom-ui/public'),
    ensureCustomUiMessages(),
  ])
  function ComponentsRoute() {
    const context = useAppRouteContext()
    return <CustomUiPage notify={context.notify} />
  }
  return { Component: ComponentsRoute }
}

export async function customComponentRoute() {
  const [{ CustomUiToolPage }] = await Promise.all([
    import('@/features/custom-ui/public'),
    ensureCustomUiMessages(),
  ])
  function CustomComponentRoute() {
    const context = useAppRouteContext()
    const { componentId = '' } = useParams()
    return <CustomUiToolPage componentId={componentId} notify={context.notify} />
  }
  return { Component: CustomComponentRoute }
}

export async function workflowsRoute() {
  const [{ WorkflowsPage }] = await Promise.all([
    import('@/features/workflows/WorkflowsPage'),
    ensureWorkflowMessages(),
  ])

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
  const [{ WorkflowBuilder }] = await Promise.all([
    import('@/features/workflows/WorkflowsPage'),
    ensureWorkflowMessages(),
  ])

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
