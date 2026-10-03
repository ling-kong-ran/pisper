import { STORAGE_KEYS } from '@/app/storage'

// 远程桌面链路失败时的自动回落：远程模式下的请求超时/不可达后，
// 把 App 切回本机模式并刷新页面，避免停留在「正在唤醒 Agent」的空等状态。
type MobileModeState = { mode?: 'local' | 'remote' | null }

type RemoteFallbackStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

const REMOTE_UNREACHABLE_STATUSES = new Set([502, 503, 504])
// 防止原生切换成功但导航没有生效时，React effect 反复触发切换。
const DEFAULT_COOLDOWN_MS = 60_000
// 原生启动可能包含冷启动，但不能让目录初始化再次永久挂起。
const DEFAULT_STATE_TIMEOUT_MS = 5_000
const DEFAULT_ENTER_LOCAL_TIMEOUT_MS = 20_000

function nativeInvoke<T>(command: string): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('移动端原生桥不可用。'))
  return invoke<T>(command)
}

function getSessionStorage(): RemoteFallbackStorage | undefined {
  try {
    return typeof window !== 'undefined' ? window.sessionStorage : undefined
  } catch {
    return undefined
  }
}

function getLocalStorage(): RemoteFallbackStorage | undefined {
  try {
    return typeof window !== 'undefined' ? window.localStorage : undefined
  } catch {
    return undefined
  }
}

function isLoopbackMobilePage() {
  return (
    typeof window !== 'undefined' &&
    window.location.protocol === 'http:' &&
    window.location.hostname === '127.0.0.1' &&
    Boolean(window.__PISPER_MOBILE_APP__)
  )
}

// 只把「远端不可达」类错误视为回落信号：代理探测失败的 HTTP 状态、
// 请求层明确标记的 timeout/network，以及原始 fetch 网络异常；业务错误和取消不回落。
function isRemoteUnreachableError(error: unknown) {
  const value = error as { status?: unknown; kind?: unknown; name?: unknown; message?: unknown }
  const status = Number(value?.status)
  if (REMOTE_UNREACHABLE_STATUSES.has(status)) return true
  if (value?.kind === 'timeout' || value?.kind === 'network') return true
  if (value?.name === 'TimeoutError') return true
  if (error instanceof TypeError) return true
  // 兼容旧版/第三方 ApiError：新版 requestJson 会提供 kind，旧错误只保留名称和文案。
  if (value?.name === 'ApiError' && value?.status == null) {
    return /tim(?:e|ed) ?out|network|请求超时|无法连接|连接桌面端失败/i.test(
      String(value.message || ''),
    )
  }
  return false
}

function readTimestamp(storage: RemoteFallbackStorage | undefined, key: string) {
  try {
    const value = storage?.getItem(key)
    const timestamp = Number(value)
    return Number.isFinite(timestamp) ? timestamp : null
  } catch {
    return null
  }
}

function writeTimestamp(
  storage: RemoteFallbackStorage | undefined,
  key: string,
  timestamp: number,
) {
  try {
    storage?.setItem(key, String(timestamp))
  } catch {
    // 存储被禁用时仍用实例内状态保护当前页面，回落本身不能因此失败。
  }
}

// 为原生调用提供上限，并给底层 Promise 安装拒绝处理，避免超时后形成未处理异常。
function settleWithin<T>(promise: Promise<T>, timeoutMs: number, message: string): Promise<T> {
  return new Promise((resolve, reject) => {
    let settled = false
    const timer = setTimeout(
      () => {
        settled = true
        reject(new Error(message))
      },
      Math.max(0, timeoutMs),
    )
    void Promise.resolve(promise).then(
      (value) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        resolve(value)
      },
      (error: unknown) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        reject(error)
      },
    )
  })
}

export type RemoteFallbackDependencies = {
  isMobileApp: () => boolean
  getMode: () => Promise<string | null | undefined>
  enterLocal: () => Promise<unknown>
  markNotice: () => void
  reload: () => void
  storage?: RemoteFallbackStorage
  now?: () => number
  cooldownMs?: number
  timeouts?: { stateMs?: number; enterLocalMs?: number }
}

export function createRemoteFallback({
  isMobileApp,
  getMode,
  enterLocal,
  markNotice,
  reload,
  storage,
  now = Date.now,
  cooldownMs = DEFAULT_COOLDOWN_MS,
  timeouts,
}: RemoteFallbackDependencies) {
  const attemptStorage = storage
  const stateTimeoutMs = timeouts?.stateMs ?? DEFAULT_STATE_TIMEOUT_MS
  const enterLocalTimeoutMs = timeouts?.enterLocalMs ?? DEFAULT_ENTER_LOCAL_TIMEOUT_MS
  let inFlight: Promise<boolean> | null = null
  let inMemoryAttemptAt: number | null = null

  const lastAttemptAt = () =>
    readTimestamp(attemptStorage, STORAGE_KEYS.mobileRemoteFallbackAttempt) ?? inMemoryAttemptAt

  const rememberAttempt = (timestamp: number) => {
    inMemoryAttemptAt = timestamp
    writeTimestamp(attemptStorage, STORAGE_KEYS.mobileRemoteFallbackAttempt, timestamp)
  }

  return {
    // 返回 true 表示已切换到本机模式并触发刷新；调用方无需再展示原错误。
    async handleFailure(error: unknown): Promise<boolean> {
      if (!isMobileApp() || !isRemoteUnreachableError(error)) return false
      if (inFlight) return inFlight

      const current = now()
      const previous = lastAttemptAt()
      if (previous !== null && current - previous < Math.max(0, cooldownMs)) return false

      const attempt = (async () => {
        try {
          // 查询也设上限，避免原生桥失去响应时把回落逻辑再次卡住。
          const mode = await settleWithin(getMode(), stateTimeoutMs, '读取移动端运行模式超时。')
          if (mode !== 'remote') return false
          // 在进入本机前记录尝试，防止切换成功但页面导航失败时重复调用原生命令。
          rememberAttempt(current)
          await settleWithin(enterLocal(), enterLocalTimeoutMs, '切换本机 Runtime 超时。')
          try {
            markNotice()
          } catch {
            // 提示标记失败不应阻止已成功的模式切换。
          }
          try {
            reload()
          } catch {
            // 本机模式已经写入；即使导航调用失败，也不要重新触发远程回落循环。
          }
          return true
        } catch {
          // 回落失败时保留原错误展示；尝试时间戳仍会在冷却窗口内阻止死循环。
          return false
        }
      })()
      inFlight = attempt.finally(() => {
        inFlight = null
      })
      return inFlight
    },
  }
}

const defaultFallback = createRemoteFallback({
  isMobileApp: isLoopbackMobilePage,
  getMode: async () => (await nativeInvoke<MobileModeState>('mobile_state'))?.mode,
  enterLocal: () => nativeInvoke('mobile_enter_local'),
  markNotice: () => {
    writeTimestamp(getLocalStorage(), STORAGE_KEYS.mobileRemoteFallbackNotice, Date.now())
  },
  reload: () => window.location.reload(),
  storage: getSessionStorage(),
})

export function fallbackRemoteToLocalAfterFailure(error: unknown) {
  return defaultFallback.handleFailure(error)
}

// 读取并清除回落通知标记（页面刷新后首次成功加载时提示一次）。
export function consumeRemoteFallbackNotice() {
  const storage = getLocalStorage()
  if (!storage) return false
  try {
    if (!storage.getItem(STORAGE_KEYS.mobileRemoteFallbackNotice)) return false
    storage.removeItem(STORAGE_KEYS.mobileRemoteFallbackNotice)
    return true
  } catch {
    return false
  }
}
