import { isRouteErrorResponse, useRouteError } from 'react-router-dom'

export function RouteErrorBoundary() {
  const error = useRouteError()
  const message = isRouteErrorResponse(error)
    ? `${error.status} ${error.statusText}`
    : error instanceof Error
      ? error.message
      : '页面加载失败'

  return (
    <main className="app-startup" role="alert">
      <strong>{message}</strong>
      <button className="button secondary" type="button" onClick={() => window.location.reload()}>
        重新加载
      </button>
    </main>
  )
}
