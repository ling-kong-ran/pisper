import {
  createCustomUiView,
  releaseCustomUiView,
  renewCustomUiView,
  type CustomUiView,
} from './custom-ui-api'

// 预览凭证由单个挂载实例持有；异常断开依赖服务端 TTL，正常退出立即撤销。
export function startCustomUiView(
  componentId: string,
  onView: (view: CustomUiView) => void,
  onFailure: () => void,
): () => void {
  const controller = new AbortController()
  let activeView: CustomUiView | null = null
  let timer: ReturnType<typeof setTimeout> | undefined
  const release = (view: CustomUiView) => {
    // 写请求不自动重试，避免离线时卸载变成新的后台任务。
    void releaseCustomUiView(view.id).catch(() => undefined)
  }
  const stop = () => {
    controller.abort()
    clearTimeout(timer)
    if (activeView) {
      release(activeView)
      activeView = null
    }
  }
  const fail = () => {
    if (controller.signal.aborted) return
    onFailure()
    stop()
  }
  const renew = async () => {
    if (!activeView || controller.signal.aborted) return
    try {
      await renewCustomUiView(activeView.id, controller.signal)
      if (!controller.signal.aborted) timer = setTimeout(() => void renew(), 60_000)
    } catch {
      fail()
    }
  }
  void createCustomUiView(componentId, controller.signal).then((view) => {
    if (controller.signal.aborted) {
      release(view)
      return
    }
    activeView = view
    onView(view)
    timer = setTimeout(() => void renew(), 60_000)
  }, fail)
  return stop
}
