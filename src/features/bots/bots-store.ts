// Bot 列表的客户端缓存：唯一权威在 Runtime，侧栏与 Bot 页共用这一份；
// 轮询由挂载的组件通过 useBotsPolling 拥有，卸载即停。
import { useEffect } from 'react'
import { create } from 'zustand'
import { fetchBots, type Bot } from './bots-api'

type BotsState = {
  bots: Bot[]
  loaded: boolean
  error: string
  load: () => Promise<void>
  upsert: (bot: Bot) => void
}

let loading: Promise<void> | null = null

export const useBotsStore = create<BotsState>()((set, get) => ({
  bots: [],
  loaded: false,
  error: '',
  load: () => {
    loading ??= fetchBots()
      .then((bots) => set({ bots, loaded: true, error: '' }))
      .catch((error: unknown) =>
        set({ loaded: true, error: error instanceof Error ? error.message : String(error) }),
      )
      .finally(() => {
        loading = null
      })
    return loading
  },
  // 新建或修改后立刻反映到列表，不等下一次轮询。
  upsert: (bot) => {
    const bots = get().bots
    const index = bots.findIndex((item) => item.id === bot.id)
    set({
      bots:
        index < 0
          ? [...bots, bot]
          : bots.map((item) => (item.id === bot.id ? { ...bot, activity: item.activity } : item)),
    })
  },
}))

// 有 Bot 在说话时 3 秒刷一次，否则 10 秒；页面隐藏时不刷。
export function useBotsPolling(enabled: boolean) {
  const load = useBotsStore((state) => state.load)
  const busy = useBotsStore((state) => state.bots.some((bot) => bot.activity?.status === 'running'))
  useEffect(() => {
    if (!enabled) return
    void load()
    const timer = window.setInterval(
      () => {
        if (document.visibilityState === 'visible') void load()
      },
      busy ? 3_000 : 10_000,
    )
    return () => window.clearInterval(timer)
  }, [enabled, busy, load])
}
