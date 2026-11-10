// 群列表的客户端缓存：唯一权威在 Runtime，侧栏与群页共用这一份；刷新由挂载的组件通过
// useRoomsPolling 拥有，卸载即停（和 Bot 列表一样）。Runtime 推 room 帧时按帧刷新、只留慢速兜底。
import { useEffect } from 'react'
import { create } from 'zustand'
import { roomFeedAlive, subscribeRoomChanges } from '@/features/board/room-changes'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import {
  ROOM_LIST_BUSY_POLL_MS,
  roomChangeAffectsList,
  roomListPollDelay,
} from './room-live-policy'
import { fetchRooms, type Room } from './rooms-api'

type RoomsState = {
  rooms: Room[]
  loaded: boolean
  load: () => Promise<void>
  upsert: (room: Room) => void
  remove: (id: string) => void
}

let loading: Promise<void> | null = null
// 正在读的时候又有变化：读完再读一次，不漏掉读的过程中发生的变化。
let again = false
let loadedAt = 0
// 一阵帧（Bot 干活时每秒几帧）合成一次读。
const FRAME_COALESCE_MS = 500

export const useRoomsStore = create<RoomsState>()((set, get) => ({
  rooms: [],
  loaded: false,
  load: () => {
    if (loading) {
      again = true
      return loading
    }
    loadedAt = Date.now()
    loading = fetchRooms()
      .then((rooms) => set({ rooms, loaded: true }))
      .catch(() => set({ loaded: true }))
      .finally(() => {
        loading = null
        if (again) {
          again = false
          void get().load()
        }
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

// 推送在线时按帧刷新、30 秒兜底；没有推送（旧 Runtime、事件流断了）时有 Bot 在群里干活 3 秒刷一次，
// 否则 10 秒。页面隐藏时不刷。
export function useRoomsPolling(enabled: boolean) {
  const load = useRoomsStore((state) => state.load)
  const busy = useRoomsStore((state) => state.rooms.some((room) => room.activity.working > 0))
  const push = useRuntimeCapabilitiesStore(
    (state) => state.capabilities.features['rooms.events'] === true,
  )
  useEffect(() => {
    if (!enabled) return
    void load()
    const visible = () => document.visibilityState === 'visible'
    const timer = window.setInterval(() => {
      if (!visible()) return
      const delay = roomListPollDelay({ push, alive: roomFeedAlive(), busy })
      if (Date.now() - loadedAt >= delay) void load()
    }, ROOM_LIST_BUSY_POLL_MS)
    let soon = 0
    const unsubscribe = push
      ? subscribeRoomChanges((change) => {
          if (!roomChangeAffectsList(change) || soon) return
          soon = window.setTimeout(() => {
            soon = 0
            if (visible()) void load()
          }, FRAME_COALESCE_MS)
        })
      : () => {}
    return () => {
      window.clearInterval(timer)
      window.clearTimeout(soon)
      unsubscribe()
    }
  }, [enabled, busy, load, push])
}
