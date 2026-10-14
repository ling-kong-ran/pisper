// 只取消当前捕获任务的等待；共享初始化和不响应取消的 SDK Promise 仍消费其结果。
/** @template T
 * @param {Promise<T>} operation
 * @param {AbortSignal | undefined} signal
 * @returns {Promise<T>}
 */
export function awaitMemoryOperation(operation, signal) {
  if (!signal) return operation
  return new Promise((resolve, reject) => {
    const onAbort = () => {
      signal.removeEventListener('abort', onAbort)
      reject(signal.reason)
    }
    signal.addEventListener('abort', onAbort, { once: true })
    operation.then(
      (value) => {
        signal.removeEventListener('abort', onAbort)
        resolve(value)
      },
      (error) => {
        signal.removeEventListener('abort', onAbort)
        reject(error)
      },
    )
    if (signal.aborted) onAbort()
  })
}
