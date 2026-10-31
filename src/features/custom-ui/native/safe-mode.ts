// 安全模式（规格 5.6、计划 §4.3）：同步死循环无法捕获，只能在下次启动时避开。
// - URL 带 ?safe-mode=components：本页所有原生组件只显示占位卡。
// - 自动哨兵：首次挂载原生实例前同步写偏好 pisper-components-safe-mode = {loading:{at, instanceIds}}，
//   所有初始实例提交完 3 秒后清掉。下次启动时哨兵还在，说明上次加载组件时页面可能卡死：
//   先不加载（mode 'pending'），由应用壳的轻量层询问用户（S4 接线）。
// 状态机与存储读写都可注入，便于测试；默认实例在首次使用时创建（模块导入没有副作用）。

export const SAFE_MODE_PREFERENCE_KEY = 'pisper-components-safe-mode'
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

export function createSafeModeController({
  storage,
  search,
  now = () => Date.now(),
  schedule = (callback, ms) => setTimeout(callback, ms),
  cancel = (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
}: {
  storage: SafeModeStorage
  search: string
  now?: () => number
  schedule?: (callback: () => void, ms: number) => unknown
  cancel?: (handle: unknown) => void
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
      listeners.clear()
    },
  }
}
