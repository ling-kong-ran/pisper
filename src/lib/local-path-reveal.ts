export const LOCAL_PATH_REVEAL_TIMEOUT_MS = 8_000

type RevealPath = (path: string) => Promise<boolean>

export class LocalPathRevealError extends Error {
  constructor(readonly reason: 'unavailable' | 'timeout' | 'failed') {
    super(`local-reveal:${reason}`)
    this.name = 'LocalPathRevealError'
  }
}

// 桌面桥接可能缺失、同步抛错或一直没有返回；这些情况都必须回到 UI 的错误反馈。
// 超时只结束等待，不取消系统操作，也不自动重试，避免迟到的请求重复打开窗口。
export async function requestLocalPathReveal(path: string, revealPath: RevealPath | undefined) {
  if (typeof revealPath !== 'function') throw new LocalPathRevealError('unavailable')
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    const result = await Promise.race([
      Promise.resolve().then(() => revealPath(path)),
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
