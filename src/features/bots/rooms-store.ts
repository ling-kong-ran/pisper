// 群列表的客户端缓存：唯一权威在 Runtime，侧栏与群页共用这一份；轮询由挂载的组件通过
// useRoomsPolling 拥有，卸载即停（和 Bot 列表一样）。
import { useEffect } from 'react'
import { create } from 'zustand'
import { fetchRooms, type Room } from './rooms-api'

type RoomsState = {
  rooms: Room[]
  loaded: boolean
  load: () => Promise<void>
  upsert: (room: Room) => void
  remove: (id: string) => void
}

let loading: Promise<void> | null = null

export const useRoomsStore = create<RoomsState>()((set, get) => ({
  rooms: [],
  loaded: false,
  load: () => {
    loading ??= fetchRooms()
      .then((rooms) => set({ rooms, loaded: true }))
      .catch(() => set({ loaded: true }))
      .finally(() => {
        loading = null
      })
    return loading
  },
  // 新建、改名后立刻反映到侧栏，不等下一次轮询；活动沿用旧的，轮询会更新。
  upsert: (room) => {
    const rooms = get().rooms
    const index = rooms.findIndex((item) => item.id === room.id)
    set({
      rooms:
        index < 0
          ? [...rooms, room]
          : rooms.map((item) =>
              item.id === room.id ? { ...room, activity: item.activity } : item,
            ),
    })
  },
  remove: (id) => set({ rooms: get().rooms.filter((item) => item.id !== id) }),
}))

// 有 Bot 在群里干活时 3 秒刷一次，否则 10 秒；页面隐藏时不刷。
export function useRoomsPolling(enabled: boolean) {
  const load = useRoomsStore((state) => state.load)
  const busy = useRoomsStore((state) => state.rooms.some((room) => room.activity.working > 0))
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
