// 应用入口：挂载 React 根节点并启动 hash 路由。
// 桌面端前端同时服务 Web 与 Tauri WebView，因此使用 createHashRouter
// 保证文件协议 / 静态托管下刷新不丢失路径；启动前把旧版 hash 页路径
// 迁移为新的 hash 路由格式，并同步页面可见性状态供流式调度器使用。
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { RouterProvider } from 'react-router-dom'
import './index.css'
import { AppProviders } from '@/app/providers'
import { router } from '@/app/router'
import { legacyHashPath } from '@/app/routes'

const syncPageVisibility = () => {
  document.documentElement.dataset.pageVisibility = document.visibilityState
}
syncPageVisibility()
document.addEventListener('visibilitychange', syncPageVisibility)

const legacyPath = legacyHashPath(window.location.hash)
if (legacyPath) {
  window.history.replaceState(
    null,
    '',
    `${window.location.pathname}${window.location.search}#${legacyPath}`,
  )
}

const rootElement = document.getElementById('root')
if (!rootElement) throw new Error('Missing #root application mount point')

createRoot(rootElement).render(
  <StrictMode>
    <AppProviders>
      <RouterProvider router={router} />
    </AppProviders>
  </StrictMode>,
)
