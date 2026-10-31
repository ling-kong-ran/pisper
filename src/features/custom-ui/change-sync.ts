// 组件变化 → 作废组件查询（计划 §3.10、§4.1）。所有放置 hook 共用一条订阅（按引用计数），
// 最后一个使用者卸载时退订；同一帧事件只作废一次。
import type { ComponentChange } from '@/features/board/component-changes'
import { customUiQueryKeys } from './query-keys'

export type InvalidatingClient = {
  invalidateQueries(filters: { queryKey: readonly unknown[] }): Promise<unknown> | unknown
}

/** 一条变化需要作废哪些查询键：'*' 作废全部组件查询；实例变化只作废实例；其余作废目录、详情与实例。 */
export function componentChangeQueryKeys(change: ComponentChange): (readonly unknown[])[] {
  if (change.id === '*') return [customUiQueryKeys.all]
  if (change.reason === 'instances') return [customUiQueryKeys.instances()]
  return [
    customUiQueryKeys.catalog(),
    customUiQueryKeys.describe(change.id),
    customUiQueryKeys.errors(change.id),
    // 删除组件会连带删除它的实例；启用状态变化会影响宿主是否渲染。
    customUiQueryKeys.instances(),
  ]
}

export function createComponentChangeSync(
  subscribe: (listener: (change: ComponentChange) => void) => () => void,
) {
  let users = 0
  let unsubscribe: (() => void) | null = null
  let client: InvalidatingClient | null = null
  return {
    retain(nextClient: InvalidatingClient): () => void {
      client = nextClient
      users += 1
      unsubscribe ??= subscribe((change) => {
        for (const queryKey of componentChangeQueryKeys(change))
          void Promise.resolve(client?.invalidateQueries({ queryKey })).catch(() => undefined)
      })
      let released = false
      return () => {
        if (released) return
        released = true
        users -= 1
        if (users > 0) return
        unsubscribe?.()
        unsubscribe = null
        client = null
      }
    },
    users: () => users,
  }
}
