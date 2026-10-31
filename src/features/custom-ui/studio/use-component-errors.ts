import { useEffect } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { listComponentErrors } from '@/features/custom-ui/custom-ui-api'
import { componentLogBuffer } from '@/features/custom-ui/native/host-runtime'
import { customUiQueryKeys } from '@/features/custom-ui/query-keys'

/** Runtime 记录的错误与网络统计：工作室打开期间轮询，本页记下新的 warn / error 时提前刷新。 */
export function useComponentErrors(componentId: string, enabled: boolean) {
  const client = useQueryClient()
  const query = useQuery({
    queryKey: customUiQueryKeys.errors(componentId),
    queryFn: ({ signal }) => listComponentErrors(componentId, signal),
    enabled,
    retry: false,
    refetchInterval: 15_000,
    refetchOnWindowFocus: false,
  })
  useEffect(() => {
    if (!enabled) return
    let timer: number | null = null
    const unsubscribe = componentLogBuffer().subscribe((id) => {
      if (id !== componentId || timer !== null) return
      // 上报是异步的：稍等一下再读，一阵连续错误只刷新一次。
      timer = window.setTimeout(() => {
        timer = null
        void client.invalidateQueries({ queryKey: customUiQueryKeys.errors(componentId) })
      }, 800)
    })
    return () => {
      unsubscribe()
      if (timer !== null) window.clearTimeout(timer)
    }
  }, [client, componentId, enabled])
  return query
}
