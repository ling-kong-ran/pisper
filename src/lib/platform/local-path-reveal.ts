export const LOCAL_PATH_REVEAL_TIMEOUT_MS = 8_000

type RevealPath = (path: string) => Promise<boolean>

export async function revealPathThroughRuntime(path: string): Promise<boolean> {
  const response = await fetch('/api/desktop/reveal-path', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ path }),
  })
  let payload: { revealed?: boolean; error?: string } = {}
  try {
    payload = (await response.json()) as typeof payload
  } catch {}
  if (!response.ok) throw new Error(payload.error || `HTTP ${response.status}`)
  return payload.revealed === true
}

export class LocalPathRevealError extends Error {
  constructor(readonly reason: 'unavailable' | 'timeout' | 'failed') {
    super(`local-reveal:${reason}`)
    this.name = 'LocalPathRevealError'
  }
}

// 桌面桥接可能缺失、同步抛错或一直没有返回；这些情况都必须回到 UI 的错误反馈。
// 超时只结束等待，不取消系统操作，也不自动重试，避免迟到的请求重复打开窗口。
export async function requestLocalPathReveal(
  path: string,
  runtimeRevealPath: RevealPath | undefined,
) {
  if (typeof runtimeRevealPath !== 'function') throw new LocalPathRevealError('unavailable')
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    const result = await Promise.race([
      Promise.resolve().then(() => runtimeRevealPath(path)),
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(
          () => reject(new LocalPathRevealError('timeout')),
          LOCAL_PATH_REVEAL_TIMEOUT_MS,
        )
      }),
    ])
    if (result !== true) throw new LocalPathRevealError('failed')
  } finally {
    clearTimeout(timer)
  }
}
