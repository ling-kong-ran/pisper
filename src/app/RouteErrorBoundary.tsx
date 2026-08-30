// 路由错误兜底页：懒加载失败 / 路径不存在时展示错误信息与重载按钮。
// 独立为页面组件以便 react-router 在错误边界内也能正常渲染。
import { isRouteErrorResponse, useRouteError } from 'react-router-dom'

import { Button } from '@/components/ui/button'

async function recoverRoute() {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (window.__PISPER_MOBILE_APP__ && invoke) {
    // 先确认本机 Runtime 和代理仍可用，再重新请求应用壳，避免只重放失败的模块请求。
    await invoke('mobile_resume_local_runtime').catch(() => undefined)
  }
  const url = new URL(window.location.href)
  if (window.__PISPER_MOBILE_APP__ && url.protocol === 'http:' && url.hostname === '127.0.0.1') {
    // 查询参数只用于绕过 WebView 对上一轮应用壳的缓存，hash 路由和当前会话仍保留。
    url.searchParams.set('_pisper_recovery', String(Date.now()))
  }
  window.location.replace(url.href)
}

export function RouteErrorBoundary() {
  const error = useRouteError()
  const message = isRouteErrorResponse(error)
    ? `${error.status} ${error.statusText}`
    : error instanceof Error
      ? error.message
      : '页面加载失败'

  return (
    <main
      className="app-startup dark:bg-[var(--bg)] dark:text-[var(--text)] flex w-full min-h-[100vh] items-center justify-center gap-[10px] bg-[var(--bg)] text-[var(--text-muted)] text-[13px]"
      role="alert"
    >
      <strong>{message}</strong>
      <Button
        variant="outline"
        size="lg"
        className="bg-surface-subtle"
        type="button"
        onClick={() => void recoverRoute()}
      >
        重新加载
      </Button>
    </main>
  )
}
