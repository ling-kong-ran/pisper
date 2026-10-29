// 路由表：全部页面挂在一个 App 壳（标题栏 + 内容区 + Outlet）下。
// 使用 hash 路由以兼容 Tauri WebView 与纯静态文件托管；每个页面走 lazy
// 按需加载，避免首屏一次性打包所有功能页。'/' 与未知路径一律回到看板首页。
import { Navigate, createHashRouter } from 'react-router-dom'
import App from '@/App'
import { UnsavedChangesProvider } from './UnsavedChangesProvider'
import { PageLoader } from '@/components/layout/PageLoader'
import { RouteErrorBoundary } from './RouteErrorBoundary'
import { PAGE_PATHS } from './routes'
import {
  assetsRoute,
  boardRoute,
  channelsRoute,
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

export const router = createHashRouter([
  {
    element: (
      <UnsavedChangesProvider>
        <App />
      </UnsavedChangesProvider>
    ),
    errorElement: <RouteErrorBoundary />,
    hydrateFallbackElement: <PageLoader />,
    children: [
      { index: true, element: <Navigate to={PAGE_PATHS.board} replace /> },
      { path: PAGE_PATHS.board, lazy: boardRoute },
      { path: '/board/:sessionId', lazy: sessionRoute },
      { path: PAGE_PATHS.more, lazy: moreRoute },
      // 兼容入口：旧链接 /chat、/chat?… 回到看板；打开具体会话的意图由应用壳的 navigate('chat') 垫片处理。
      { path: PAGE_PATHS.chat, element: <Navigate to={PAGE_PATHS.board} replace /> },
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
      { path: PAGE_PATHS.components, lazy: componentsRoute },
      { path: '/tools/components/:componentId', lazy: customComponentRoute },
      { path: PAGE_PATHS.workflows, lazy: workflowsRoute },
      { path: PAGE_PATHS.workflowCreate, lazy: workflowBuilderRoute },
      { path: '/workflows/:workflowId', lazy: workflowBuilderRoute },
      { path: '*', element: <Navigate to={PAGE_PATHS.board} replace /> },
    ],
  },
])
