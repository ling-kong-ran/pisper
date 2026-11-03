// Bot 的电脑列表（B6）：在线状态会变，Bot 页打开时 10 秒刷一次；卸载时取消请求与定时器。
import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchEnvironments, type BotEnvironment, type BotRemoteInfo } from './bots-api'

const EMPTY_REMOTE: BotRemoteInfo = { enabled: false, endpoints: [], fingerprint: null }

export function useBotEnvironments() {
  const [environments, setEnvironments] = useState<BotEnvironment[]>([])
  const [remote, setRemote] = useState<BotRemoteInfo>(EMPTY_REMOTE)
  const [loaded, setLoaded] = useState(false)
  const generation = useRef(0)

  const refresh = useCallback(async () => {
    const current = ++generation.current
    try {
      const next = await fetchEnvironments()
      if (current !== generation.current) return
      setEnvironments(next.environments)
      setRemote(next.remote)
    } catch {
      // 拿不到列表时保留上一次的结果；本机始终可用。
    } finally {
      if (current === generation.current) setLoaded(true)
    }
  }, [])

  useEffect(() => {
    void refresh()
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') void refresh()
    }, 10_000)
    return () => {
      window.clearInterval(timer)
      generation.current += 1
    }
  }, [refresh])

  return { environments, remote, loaded, refresh }
}
