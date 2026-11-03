// 某个 Bot 的任务（对话内容）：有任务在跑时 2 秒刷一次，否则 10 秒；切换 Bot 或卸载时取消请求与定时器。
import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchBotTasks, type BotTask } from './bots-api'
import { isActiveTaskStatus } from './bot-presentation'

export function useBotTasks(botId: string) {
  const [tasks, setTasks] = useState<BotTask[]>([])
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState('')
  const generation = useRef(0)

  const refresh = useCallback(async () => {
    if (!botId) return
    const current = ++generation.current
    const controller = new AbortController()
    try {
      const next = await fetchBotTasks(botId, controller.signal)
      // 乱序响应：只接受最新一次请求的结果。
      if (current !== generation.current) return
      setTasks(next)
      setError('')
    } catch (caught) {
      if (current === generation.current)
        setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      if (current === generation.current) setLoaded(true)
    }
  }, [botId])

  const active = tasks.some((task) => isActiveTaskStatus(task.status))
  useEffect(() => {
    setTasks([])
    setLoaded(false)
    setError('')
  }, [botId])
  useEffect(() => {
    void refresh()
    const timer = window.setInterval(
      () => {
        if (document.visibilityState === 'visible') void refresh()
      },
      active ? 2_000 : 10_000,
    )
    return () => {
      window.clearInterval(timer)
      generation.current += 1
    }
  }, [refresh, active])

  return { tasks, loaded, error, refresh }
}
