// 路由表：全部页面挂在一个 App 壳（标题栏 + 内容区 + Outlet）下。
// 使用 hash 路由以兼容 Tauri WebView 与纯静态文件托管；每个页面走 lazy
// 按需加载，避免首屏一次性打包所有功能页。主页 '/' 是会话（上次停留的会话或新任务草稿，
// 落地逻辑随会话路由懒加载）；未知路径与 /chat 兼容入口一律回到主页，看板仍在 /board。
import { Navigate, createHashRouter } from 'react-router-dom'
import App from '@/App'
import { UnsavedChangesProvider } from './UnsavedChangesProvider'
import { PageLoader } from '@/components/layout/PageLoader'
import { RouteErrorBoundary } from './RouteErrorBoundary'
import { HOME_PATH, PAGE_PATHS } from './routes'
import {
  assetsRoute,
  boardRoute,
  channelsRoute,
  homeRoute,
  chatHistoryRoute,
  componentsRoute,
  customComponentRoute,
  configRoute,
  mcpRoute,
  memoryRoute,
  moreRoute,
  pluginsRoute,
  schedulesRoute,
  sessionRoute,
  skillsRoute,
  workflowBuilderRoute,
  workflowsRoute,
} from './route-elements'

// 应用壳判断“要去哪”读路由器的实时状态：navigate() 同步更新它，React 的 location 要等 transition 提交。
// 只在事件处理时调用，那时 router 早已创建。
const routerState = () => router.state

export const router = createHashRouter([
  {
    element: (
      <UnsavedChangesProvider>
        <App routerState={routerState} />
      </UnsavedChangesProvider>
    ),
    errorElement: <RouteErrorBoundary />,
    hydrateFallbackElement: <PageLoader />,
    children: [
      { index: true, lazy: homeRoute },
      { path: PAGE_PATHS.board, lazy: boardRoute },
      { path: '/board/:sessionId', lazy: sessionRoute },
      { path: PAGE_PATHS.more, lazy: moreRoute },
      // 兼容入口：旧链接 /chat、/chat?… 回到主页；打开具体会话的意图由应用壳的 navigate('chat') 垫片处理。
      { path: PAGE_PATHS.chat, element: <Navigate to={HOME_PATH} replace /> },
      { path: PAGE_PATHS.chatHistory, lazy: chatHistoryRoute },
      { path: PAGE_PATHS.assets, lazy: assetsRoute },
      { path: PAGE_PATHS.channels, lazy: channelsRoute },
      { path: PAGE_PATHS.schedules, lazy: schedulesRoute },
      { path: PAGE_PATHS.config, lazy: configRoute },
      { path: '/config/:configSection', lazy: configRoute },
      { path: '/config', element: <Navigate to={PAGE_PATHS.config} replace /> },
      { path: PAGE_PATHS.plugins, lazy: pluginsRoute },
      { path: PAGE_PATHS.memory, lazy: memoryRoute },
      { path: PAGE_PATHS.mcp, lazy: mcpRoute },
      { path: PAGE_PATHS.skills, lazy: skillsRoute },
      // 组件列表与详情共用一条路由（可选段）：桌面在双栏里切换选中项不重挂页面，手机每级一屏。
      { path: '/components/:componentId?', lazy: componentsRoute },
      { path: '/tools/components/:componentId', lazy: customComponentRoute },
      { path: PAGE_PATHS.workflows, lazy: workflowsRoute },
      { path: PAGE_PATHS.workflowCreate, lazy: workflowBuilderRoute },
      { path: '/workflows/:workflowId', lazy: workflowBuilderRoute },
      { path: '*', element: <Navigate to={HOME_PATH} replace /> },
    ],
  },
])
