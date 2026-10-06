export function abortReason(signal: AbortSignal): unknown {
  return signal.reason === undefined
    ? new DOMException('The operation was aborted.', 'AbortError')
    : signal.reason
}

export function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted) throw abortReason(signal)
}

// 取消只结束当前调用者的等待，不能终止其他请求共用的恢复工作。
export function waitWithAbort<T>(work: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const aborted = () => reject(abortReason(signal))
    if (signal.aborted) aborted()
    else signal.addEventListener('abort', aborted, { once: true })
    void work.then(
      (value) => {
        signal.removeEventListener('abort', aborted)
        resolve(value)
      },
      (error: unknown) => {
        signal.removeEventListener('abort', aborted)
        reject(error)
      },
    )
  })
}

// 较旧的系统 WebView 缺少 AbortSignal.any/timeout；显式清理可避免长会话积累监听器。
export function createAbortScope(parent?: AbortSignal, timeoutMs?: number) {
  const controller = new AbortController()
  let timer: ReturnType<typeof setTimeout> | undefined
  const forward = () => controller.abort(parent && abortReason(parent))
  const dispose = () => {
    parent?.removeEventListener('abort', forward)
    controller.signal.removeEventListener('abort', dispose)
    clearTimeout(timer)
  }
  controller.signal.addEventListener('abort', dispose, { once: true })
  if (parent?.aborted) forward()
  else {
    parent?.addEventListener('abort', forward, { once: true })
    if (timeoutMs !== undefined)
      timer = setTimeout(
        () => controller.abort(new DOMException('The operation timed out.', 'TimeoutError')),
        timeoutMs,
      )
  }
  return {
    signal: controller.signal,
    abort: (reason?: unknown) => controller.abort(reason),
    dispose,
  }
}
