import { useEffect, useState } from 'react'

// 时长每秒推进；页面隐藏时暂停，回到前台立即补一次。
export function useNow(enabled: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!enabled) return
    let timer: ReturnType<typeof setInterval> | null = null
    const start = () => {
      if (timer || document.visibilityState === 'hidden') return
      setNow(Date.now())
      timer = setInterval(() => setNow(Date.now()), 1_000)
    }
    const stop = () => {
      if (timer) clearInterval(timer)
      timer = null
    }
    const visibility = () => (document.visibilityState === 'hidden' ? stop() : start())
    start()
    document.addEventListener('visibilitychange', visibility)
    return () => {
      stop()
      document.removeEventListener('visibilitychange', visibility)
    }
  }, [enabled])
  return now
}
