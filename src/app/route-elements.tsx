// 路由 → 页面的懒加载映射：每页一个 async 工厂，首屏只加载看板。
// 页面组件从 Outlet 上下文取公共能力并显式透传给具体页面，保持
// 页面与壳的依赖边界清晰（页面不直接读全局单例）。
import { useLocation, useNavigate, useOutletContext, useParams } from 'react-router-dom'
import {
  ensureBoardMessages,
  ensureChannelsMessages,
  ensureCustomUiMessages,
  ensureMcpMessages,
  ensureWorkflowMessages,
} from './i18n'
import type { AppRouteContext } from './route-context'
import { componentPath, pagePath, sessionPath } from './routes'

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
        requestConfirm={context.requestConfirm}
        requestText={context.requestText}
        onNewTask={context.startNewTask}
      />
    )
  }

  return { Component: BoardRoute }
}

// 会话页（规格 4.1）：泳道条 + 会话页头 + 对话 + 检查器，布局编排在按需加载的 SessionShell 里，
// 不进入入口包；聊天页仍是独立的懒加载入口。
export async function sessionRoute() {
  const [{ ChatPage }, { SessionShell }] = await Promise.all([
    import('@/features/chat/ChatPage'),
    import('@/app/SessionShell'),
    // 泳道条、会话页头、检查器与确认层的文案都在看板命名空间里。
    ensureBoardMessages(),
  ])

  function SessionRoute() {
    return <SessionShell context={useAppRouteContext()} ChatPage={ChatPage} />
  }

  return { Component: SessionRoute }
}

// 会话主页（'/'）：上次停留的会话或“新任务”草稿。落地逻辑与草稿随这里按需加载，不进入入口包；
// 多数落地会马上切到上次的会话，顺手预取会话路由（聊天页仍只从 sessionRoute 加载；不等待，草稿不被它拖慢）。
export async function homeRoute() {
  void sessionRoute().catch(() => undefined)
  const [{ SessionHome }] = await Promise.all([
    import('@/app/SessionHome'),
    // 看板入口、泳道条与草稿的文案都在看板命名空间里。
    ensureBoardMessages(),
  ])

  function HomeRoute() {
    return <SessionHome context={useAppRouteContext()} />
  }

  return { Component: HomeRoute }
}

export async function moreRoute() {
  const { MorePage } = await import('@/app/MorePage')

  function MoreRoute() {
    const context = useAppRouteContext()
    const navigate = useNavigate()
    return (
      <MorePage
        onOpen={(destination) => {
          // 功能页只有一个家（规格 2.6，双归属退役）：不再往历史条目里写进入来源。
          // 远程访问 / 手机服务器 / 桌面宠物仍在 /config/:section，作为功能页打开（点亮“功能”、页头“‹ 功能”）。
          if (destination.type === 'page') navigate(pagePath(destination.id))
          else if (destination.type === 'config') context.setConfigSection(destination.id)
          else navigate(`/tools/components/${encodeURIComponent(destination.id)}`)
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
    // “设置 → 快捷键”里会话页与看板相关的新动作文案在看板命名空间里（不占入口包）。
    ensureBoardMessages(),
    // “界面”里“组件已移到 组件 页 →”的文案在组件命名空间里。
    ensureCustomUiMessages(),
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
        renderInterfaceSettings={(appearance) => <ChatAppearancePage appearance={appearance} />}
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
// 从手机组件列表 push 进详情的历史条目带这个标记（见 ComponentsRoute）。
const COMPONENT_LIST_ENTRY = Object.freeze({ componentFromList: true })
function isComponentListEntry(state: unknown) {
  return (
    typeof state === 'object' &&
    state !== null &&
    (state as { componentFromList?: unknown }).componentFromList === true
  )
}

export async function componentsRoute() {
  const [{ CustomUiPage }] = await Promise.all([
    import('@/features/custom-ui/public'),
    ensureCustomUiMessages(),
  ])
  function ComponentsRoute() {
    const context = useAppRouteContext()
    const navigate = useNavigate()
    const location = useLocation()
    const { componentId = '' } = useParams()
    // 选中项写进路由（/components/:componentId）：手机上列表 → 详情各占一屏，返回键回到列表；
    // 桌面在双栏里切换选中项用 replace，不在历史里堆积。
    // 手机详情的“‹ 全部组件”：详情是从列表 push 进来的就退回那一条，否则再 push 一条列表会让
    // 浏览器后退又回到详情；直接打开的详情（深链）用 replace 换成列表。
    const select = (id: string, replace: boolean) => {
      if (!id && !replace && isComponentListEntry(location.state)) navigate(-1)
      else
        navigate(componentPath(id), {
          replace: replace || !id,
          state: id && !replace ? COMPONENT_LIST_ENTRY : undefined,
        })
    }
    return <CustomUiPage notify={context.notify} selectedId={componentId} onSelect={select} />
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
