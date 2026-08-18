// 路由表：全部页面挂在一个 App 壳（侧边栏 + 顶部栏 + Outlet）下。
// 使用 hash 路由以兼容 Tauri WebView 与纯静态文件托管；每个页面走 lazy
// 按需加载，避免首屏一次性打包所有功能页。未知路径兜底跳回聊天页。
import { Navigate, createHashRouter } from 'react-router-dom'
import App from '@/App'
import { PageLoader } from '@/components/layout/PageLoader'
import { RouteErrorBoundary } from './RouteErrorBoundary'
import { PAGE_PATHS } from './routes'
import {
  assetsRoute,
  channelsRoute,
  chatHistoryRoute,
  chatRoute,
  configRoute,
  mcpRoute,
  memoryRoute,
  pluginsRoute,
  schedulesRoute,
  skillsRoute,
  workflowBuilderRoute,
  workflowsRoute,
} from './route-elements'

export const router = createHashRouter([
  {
    element: <App />,
    errorElement: <RouteErrorBoundary />,
    hydrateFallbackElement: <PageLoader />,
    children: [
      { index: true, element: <Navigate to={PAGE_PATHS.chat} replace /> },
      { path: PAGE_PATHS.chat, lazy: chatRoute },
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
      { path: PAGE_PATHS.workflows, lazy: workflowsRoute },
      { path: PAGE_PATHS.workflowCreate, lazy: workflowBuilderRoute },
      { path: '/workflows/:workflowId', lazy: workflowBuilderRoute },
      { path: '*', element: <Navigate to={PAGE_PATHS.chat} replace /> },
    ],
  },
])
