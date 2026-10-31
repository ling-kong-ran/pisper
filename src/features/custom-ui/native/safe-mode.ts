// 安全模式（规格 5.6、计划 §4.3）：同步死循环无法捕获，只能在下次启动时避开。
// - URL 带 ?safe-mode=components：本页所有原生组件只显示占位卡。
// - 自动哨兵：首次挂载原生实例前同步写偏好 pisper-components-safe-mode = {loading:{at, instanceIds}}，
//   所有初始实例提交完 3 秒后清掉；这 3 秒内正常刷新 / 关窗（beforeunload、pagehide）也同步清掉——
//   真卡死时主线程被占住，跑不到这些处理器，所以“哨兵还在”仍可靠表示上次卡死（评审返修：原来 3 秒内刷新会误报）。
//   下次启动时哨兵还在，说明上次加载组件时页面可能卡死：
//   先不加载（mode 'pending'），由应用壳的轻量层询问用户（S4 接线）。
// 状态机与存储读写都可注入，便于测试；默认实例在首次使用时创建（模块导入没有副作用）。

import { SAFE_MODE_PREFERENCE_KEY } from '@/features/custom-ui/safe-mode-sentinel'

export { SAFE_MODE_PREFERENCE_KEY }
export const SAFE_MODE_SEARCH_PARAM = 'safe-mode'
export const SAFE_MODE_SEARCH_VALUE = 'components'
export const SAFE_MODE_CLEAR_DELAY_MS = 3_000
const MAX_SENTINEL_INSTANCES = 64

export type SafeMode = 'normal' | 'safe' | 'pending'
export type SafeModeSentinel = { loading: { at: string; instanceIds: string[] } }
export type SafeModeChoice = 'safe' | 'normal'

export type SafeModeStorage = {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

const INSTANCE_KEY = /^[a-zA-Z0-9][a-zA-Z0-9._:-]{0,95}$/

/** 解析存储里的哨兵；不合法（被改坏、旧格式）时返回 null，按“没有哨兵”处理，不因此进入安全模式。 */
export function parseSafeModeSentinel(raw: string | null | undefined): SafeModeSentinel | null {
  if (!raw || raw.length > 16_384) return null
  let value: unknown
  try {
    value = JSON.parse(raw)
  } catch {
    return null
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const loading = (value as Record<string, unknown>).loading
  if (!loading || typeof loading !== 'object' || Array.isArray(loading)) return null
  const { at, instanceIds } = loading as Record<string, unknown>
  if (typeof at !== 'string' || !Number.isFinite(Date.parse(at)) || !Array.isArray(instanceIds))
    return null
  const ids = instanceIds
    .filter((id): id is string => typeof id === 'string' && INSTANCE_KEY.test(id))
    .slice(0, MAX_SENTINEL_INSTANCES)
  return { loading: { at, instanceIds: ids } }
}

export function safeModeRequested(search: string): boolean {
  try {
    return new URLSearchParams(search).get(SAFE_MODE_SEARCH_PARAM) === SAFE_MODE_SEARCH_VALUE
  } catch {
    return false
  }
}

export type SafeModeController = {
  mode(): SafeMode
  // 上次启动留下的哨兵（mode 为 'pending' 时有），供询问界面显示。
  staleSentinel(): SafeModeSentinel | null
  // 宿主在首次挂载某个实例、加载模块之前调用；初始阶段结束后不再写哨兵。
  beginLoad(instanceKey: string): void
  // 实例首次提交（或首次就失败）后调用。
  endLoad(instanceKey: string): void
  resolve(choice: SafeModeChoice): void
  subscribe(listener: () => void): () => void
  dispose(): void
}

// 页面正常卸载的信号：beforeunload 先于 page-state-storage 的 pagehide 镜像补发，清哨兵的删除能随那一批发出；
// pagehide（捕获阶段）兜底移动端不触发 beforeunload 的情况。只在初始加载阶段挂着。
function listenPageUnload(handler: () => void): () => void {
  if (typeof window === 'undefined') return () => {}
  window.addEventListener('beforeunload', handler)
  window.addEventListener('pagehide', handler, { capture: true })
  return () => {
    window.removeEventListener('beforeunload', handler)
    window.removeEventListener('pagehide', handler, { capture: true })
  }
}

export function createSafeModeController({
  storage,
  search,
  now = () => Date.now(),
  schedule = (callback, ms) => setTimeout(callback, ms),
  cancel = (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
  listenUnload = listenPageUnload,
}: {
  storage: SafeModeStorage
  search: string
  now?: () => number
  schedule?: (callback: () => void, ms: number) => unknown
  cancel?: (handle: unknown) => void
  listenUnload?: (handler: () => void) => () => void
}): SafeModeController {
  const listeners = new Set<() => void>()
  const read = () => {
    try {
      return parseSafeModeSentinel(storage.getItem(SAFE_MODE_PREFERENCE_KEY))
    } catch {
      return null
    }
  }
  const write = (sentinel: SafeModeSentinel | null) => {
    try {
      if (sentinel) storage.setItem(SAFE_MODE_PREFERENCE_KEY, JSON.stringify(sentinel))
      else storage.removeItem(SAFE_MODE_PREFERENCE_KEY)
    } catch {
      // 存储不可用时退化为只有 URL 参数的安全模式；不能因此阻止组件加载。
    }
  }
  const stale = safeModeRequested(search) ? null : read()
  let mode: SafeMode = safeModeRequested(search) ? 'safe' : stale ? 'pending' : 'normal'
  let staleSentinel = stale
  // 初始阶段：从第一个实例开始加载到全部提交后 3 秒。之后挂载的实例（切页、新放置）不再写哨兵。
  let phase: 'idle' | 'loading' | 'done' = 'idle'
  const loading = new Set<string>()
  const started: string[] = []
  let clearTimer: unknown = null
  let stopUnload: (() => void) | null = null
  const finishLoading = () => {
    stopUnload?.()
    stopUnload = null
  }
  const changed = () => {
    for (const listener of [...listeners]) listener()
  }
  return {
    mode: () => mode,
    staleSentinel: () => staleSentinel,
    beginLoad(instanceKey) {
      if (mode !== 'normal' || phase === 'done') return
      if (clearTimer !== null) {
        cancel(clearTimer)
        clearTimer = null
      }
      phase = 'loading'
      // 正常卸载不是卡死：清掉哨兵，下次启动不询问。卸载后不再有写入，阶段不必改。
      stopUnload ??= listenUnload(() => {
        if (phase === 'loading') write(null)
      })
      loading.add(instanceKey)
      if (!started.includes(instanceKey) && started.length < MAX_SENTINEL_INSTANCES)
        started.push(instanceKey)
      write({ loading: { at: new Date(now()).toISOString(), instanceIds: [...started] } })
    },
    endLoad(instanceKey) {
      if (phase !== 'loading' || !loading.delete(instanceKey) || loading.size) return
      clearTimer = schedule(() => {
        clearTimer = null
        if (loading.size) return
        phase = 'done'
        finishLoading()
        write(null)
      }, SAFE_MODE_CLEAR_DELAY_MS)
    },
    resolve(choice) {
      if (mode !== 'pending') return
      staleSentinel = null
      write(null)
      mode = choice === 'safe' ? 'safe' : 'normal'
      changed()
    },
    subscribe(listener) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    dispose() {
      if (clearTimer !== null) cancel(clearTimer)
      clearTimer = null
      finishLoading()
      listeners.clear()
    },
  }
}
