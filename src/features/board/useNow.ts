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

// 分钟级时钟：对齐到下一个整分钟才推进（返回当前分钟起点的毫秒数），页面隐藏时暂停、回前台立即补一次。
// 侧栏分组只为“今天”的边界需要它；用每秒时钟会让侧栏、窄轨计数与手机标签栏每秒整体重渲染。
export function useMinute() {
  const [minute, setMinute] = useState(() => Math.floor(Date.now() / 60_000) * 60_000)
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null
    const tick = () => {
      const now = Date.now()
      setMinute(Math.floor(now / 60_000) * 60_000)
      timer = setTimeout(tick, 60_000 - (now % 60_000) + 50)
    }
    const stop = () => {
      if (timer) clearTimeout(timer)
      timer = null
    }
    const visibility = () => {
      stop()
      if (document.visibilityState !== 'hidden') tick()
    }
    visibility()
    document.addEventListener('visibilitychange', visibility)
    return () => {
      stop()
      document.removeEventListener('visibilitychange', visibility)
    }
  }, [])
  return minute
}
