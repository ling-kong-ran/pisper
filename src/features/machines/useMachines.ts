// 远程机器列表：对方连过来的机器在线状态会变，打开时 10 秒刷一次；页面隐藏时不刷，卸载时放弃未完成的结果。
import { useCallback, useEffect, useRef, useState } from 'react'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { fetchMachines, type Machine, type MachineRemoteInfo } from './machines-api'

const EMPTY_REMOTE: MachineRemoteInfo = { enabled: false, endpoints: [], fingerprint: null }
const REFRESH_MS = 10_000

export function useMachines() {
  const available = useRuntimeCapabilitiesStore(
    (state) => state.capabilities.features.remoteMachines,
  )
  const [machines, setMachines] = useState<Machine[]>([])
  const [remote, setRemote] = useState<MachineRemoteInfo>(EMPTY_REMOTE)
  const [loaded, setLoaded] = useState(false)
  const generation = useRef(0)

  const refresh = useCallback(async () => {
    const current = ++generation.current
    try {
      const next = await fetchMachines()
      if (current !== generation.current) return
      setMachines(next.machines)
      setRemote(next.remote)
    } catch {
      // 拿不到列表时保留上一次的结果。
    } finally {
      if (current === generation.current) setLoaded(true)
    }
  }, [])

  useEffect(() => {
    if (!available) return
    void refresh()
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') void refresh()
    }, REFRESH_MS)
    return () => {
      window.clearInterval(timer)
      generation.current += 1
    }
  }, [available, refresh])

  return { available, machines, remote, loaded, refresh }
}
