import { isRouteErrorResponse, useRouteError } from 'react-router-dom'

import { Button } from '@/components/ui/button'

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
        onClick={() => window.location.reload()}
      >
        重新加载
      </Button>
    </main>
  )
}
