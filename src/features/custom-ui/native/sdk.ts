// ctx SDK v1（规格 5.6、计划 §4.2）：宿主为每个挂载的实例构建一份 ctx。
// - 能力是透明清单：调用没有声明的能力时抛 CapabilityError 并记一条日志。这不是隔离——组件与应用同源，
//   仍可以直接用 window.fetch（负责人已接受）；它让“声明了什么、用了什么”对用户可见、对代理可纠错。
// - ctx 托管的回调（定时器、订阅）都包在 try/catch 里，异常归属到实例并计入熔断（onCallbackError）。
// - 依赖全部注入（React / html / ui / theme / 请求函数 / 会话源），这个文件不导入 htm、基础组件或看板，
//   测试可以直接构建 ctx。成员清单与 shared 的 CTX_V1_KEYS 一致（custom-ui-native-host.test 校验）。
import type {
  CapabilityType,
  ComponentSurface,
  ManifestLocale,
  NativeManifest,
} from '@shared/custom-ui-manifest.mjs'
import type {
  ComponentFetchRequest,
  ComponentFetchResult,
} from '@/features/custom-ui/custom-ui-api'
import type { ComponentLogger } from './log'
import {
  ComponentRequestError,
  buildFetchRequest,
  createFetchResponse,
  linkSignals,
  type ComponentFetchResponse,
  type FetchInit,
} from './net'
import {
  filterComponentSessions,
  type ComponentSessionStatus,
  type ComponentSessionSummary,
} from './sessions'
import type { ComponentSize } from './size'
import type { ComponentThemeInfo } from './theme'
import { createManagedTimers, type TimerHost } from './timers'

export const SDK_VERSION = 1 as const
export const NOTIFY_MAX_LENGTH = 200
export const NOTIFY_PER_MINUTE = 6
// 与 Runtime 的存储键规则一致（custom-ui-storage.mjs），不合法的键在客户端就拒绝，不发请求。
const STORAGE_KEY = /^[a-zA-Z0-9][a-zA-Z0-9._:-]{0,127}$/

/** 调用了 manifest 没有声明的能力。 */
export class CapabilityError extends Error {
  capability: CapabilityType
  constructor(capability: CapabilityType) {
    super(`Capability "${capability}" is not declared in manifest.json`)
    this.name = 'CapabilityError'
    this.capability = capability
  }
}

export type ComponentPlacementInfo = {
  surface: ComponentSurface
  instanceId: string
  title: string
  preview: boolean
}

export type ComponentSessionInfo = {
  id: string
  title: string
  cwd: string
  status: ComponentSessionStatus
}

export type ComponentKit = {
  React: unknown
  html: unknown
  ui: Readonly<Record<string, unknown>>
  useTheme: () => ComponentThemeInfo
}

export type ComponentServices = {
  notify(text: string): void
  openSession(id: string): void
  // React hook：只在声明了 sessions.read 的组件渲染时调用。
  useSessions(): readonly ComponentSessionSummary[]
  subscribeSessions(listener: (sessions: readonly ComponentSessionSummary[]) => void): () => void
  fetch(request: ComponentFetchRequest, signal: AbortSignal): Promise<ComponentFetchResult>
  // 不存在时返回 undefined。
  storageGet(key: string): Promise<unknown>
  storageSet(key: string, value: unknown): Promise<void>
  storageRemove(key: string): Promise<void>
}

export type ComponentRuntimeOptions = {
  manifest: NativeManifest
  // 每次调用 ctx.t 时读当前语言：切换语言不必重建 ctx（组件状态得以保留）。
  getLanguage(): ManifestLocale
  kit: ComponentKit
  services: ComponentServices
  logger: ComponentLogger
  // 回调异常（定时器、订阅）：宿主记日志并计入熔断。
  onCallbackError(error: unknown): void
  timers?: TimerHost
  now?: () => number
}

export type ComponentRuntime = {
  readonly signal: AbortSignal
  readonly disposed: boolean
  context(view: {
    size: ComponentSize
    placement: ComponentPlacementInfo
    session?: ComponentSessionInfo
  }): Readonly<Record<string, unknown>>
  dispose(): void
}

function browserTimers(): TimerHost {
  return {
    setTimeout: (handler, ms) => window.setTimeout(handler, ms),
    setInterval: (handler, ms) => window.setInterval(handler, ms),
    clearTimeout: (id) => window.clearTimeout(id),
    clearInterval: (id) => window.clearInterval(id),
  }
}

/** ctx.t：读 manifest.i18n.<locale>.strings，缺失时退到另一种语言，再退到键名；{name} 用 vars 替换。 */
export function translateComponentString(
  manifest: Pick<NativeManifest, 'i18n'>,
  language: ManifestLocale,
  key: unknown,
  vars?: unknown,
): string {
  if (typeof key !== 'string') return ''
  const other: ManifestLocale = language === 'zh-CN' ? 'en-US' : 'zh-CN'
  const template =
    manifest.i18n[language]?.strings?.[key] ?? manifest.i18n[other]?.strings?.[key] ?? key
  if (!vars || typeof vars !== 'object') return template
  const values = vars as Record<string, unknown>
  return template.replace(/\{([a-zA-Z0-9_]{1,64})\}/g, (match, name: string) => {
    const value = Object.hasOwn(values, name) ? values[name] : undefined
    return typeof value === 'string' || (typeof value === 'number' && Number.isFinite(value))
      ? String(value)
      : match
  })
}

/** manifest 里组件在当前语言下的显示名（工作室、功能索引、卡片头共用）。 */
export function componentDisplayName(
  manifest: Pick<NativeManifest, 'i18n' | 'name'>,
  language: ManifestLocale,
): string {
  return manifest.i18n[language]?.name || manifest.name
}

export function createComponentRuntime(options: ComponentRuntimeOptions): ComponentRuntime {
  const { manifest, getLanguage, kit, services, logger, onCallbackError } = options
  const now = options.now ?? (() => Date.now())
  const controller = new AbortController()
  const subscriptions = new Set<() => void>()
  let disposed = false
  const capabilities = manifest.capabilities

  const callbackError = (error: unknown) => {
    if (disposed) return
    logger.record('callback', 'error', error)
    onCallbackError(error)
  }
  const timers = createManagedTimers({
    host: options.timers ?? browserTimers(),
    onError: callbackError,
  })

  const require = (capability: CapabilityType) => {
    if (capabilities[capability]) return
    const error = new CapabilityError(capability)
    logger.record('log', 'warn', error)
    throw error
  }
  // 异步能力：未声明时返回被拒绝的 Promise（await 处照样抛出 CapabilityError）。
  const requireAsync = (capability: CapabilityType) => {
    try {
      require(capability)
      return null
    } catch (error) {
      return Promise.reject(error)
    }
  }

  const notifyTimes: number[] = []
  const notify = (text: unknown) => {
    require('notify')
    if (disposed) return
    const at = now()
    while (notifyTimes.length && at - notifyTimes[0] >= 60_000) notifyTimes.shift()
    if (notifyTimes.length >= NOTIFY_PER_MINUTE) {
      logger.warn('notify rate limited (6 per minute)')
      return
    }
    const message = (typeof text === 'string' ? text : String(text)).trim()
    if (!message) return
    notifyTimes.push(at)
    services.notify(Array.from(message).slice(0, NOTIFY_MAX_LENGTH).join(''))
  }

  const sessions = Object.freeze({
    useList(filter?: unknown) {
      require('sessions.read')
      return filterComponentSessions(services.useSessions(), filter)
    },
    subscribe(listener: unknown) {
      require('sessions.read')
      if (typeof listener !== 'function' || disposed) return () => {}
      const unsubscribe = services.subscribeSessions((list) => {
        if (disposed) return
        try {
          ;(listener as (sessions: readonly ComponentSessionSummary[]) => void)(list)
        } catch (error) {
          callbackError(error)
        }
      })
      let active = true
      const release = () => {
        if (!active) return
        active = false
        subscriptions.delete(release)
        unsubscribe()
      }
      subscriptions.add(release)
      return release
    },
    open(id: unknown) {
      require('sessions.open')
      if (disposed || typeof id !== 'string' || !id) return
      services.openSession(id)
    },
  })

  const net = Object.freeze({
    fetch(url: unknown, init?: FetchInit): Promise<ComponentFetchResponse> {
      const denied = requireAsync('network')
      if (denied) return denied
      let request: ComponentFetchRequest
      try {
        request = buildFetchRequest(url, init ?? {})
      } catch (error) {
        return Promise.reject(error)
      }
      const linked = linkSignals([controller.signal, init?.signal])
      return services
        .fetch(request, linked.signal)
        .then(createFetchResponse, (error: unknown) => {
          throw error instanceof ComponentRequestError ? error : toComponentRequestError(error)
        })
        .finally(linked.release)
    },
  })

  const storageKey = (key: unknown) => {
    if (typeof key !== 'string' || !STORAGE_KEY.test(key))
      throw new ComponentRequestError('component_storage_key_invalid', 'invalid storage key')
    return key
  }
  const storage = Object.freeze({
    get(key: unknown) {
      const denied = requireAsync('storage')
      if (denied) return denied
      try {
        return services.storageGet(storageKey(key))
      } catch (error) {
        return Promise.reject(error)
      }
    },
    set(key: unknown, value: unknown) {
      const denied = requireAsync('storage')
      if (denied) return denied
      try {
        return services.storageSet(storageKey(key), value)
      } catch (error) {
        return Promise.reject(error)
      }
    },
    remove(key: unknown) {
      const denied = requireAsync('storage')
      if (denied) return denied
      try {
        return services.storageRemove(storageKey(key))
      } catch (error) {
        return Promise.reject(error)
      }
    },
  })

  const log = Object.freeze({
    info: (...args: unknown[]) => logger.info(...args),
    warn: (...args: unknown[]) => logger.warn(...args),
    error: (...args: unknown[]) => logger.error(...args),
  })
  const theme = Object.freeze({ useTheme: kit.useTheme })
  const t = (key: unknown, vars?: unknown) =>
    translateComponentString(manifest, getLanguage(), key, vars)

  return {
    get signal() {
      return controller.signal
    },
    get disposed() {
      return disposed
    },
    context({ size, placement, session }) {
      // 成员顺序与 CTX_V1_KEYS 一致；session 只在会话面板放置时有值，其余放置为 undefined。
      return Object.freeze({
        sdk: SDK_VERSION,
        React: kit.React,
        html: kit.html,
        ui: kit.ui,
        theme,
        sessions,
        notify,
        net,
        storage,
        signal: controller.signal,
        setTimeout: timers.setTimeout,
        setInterval: timers.setInterval,
        clearTimeout: timers.clearTimeout,
        clearInterval: timers.clearInterval,
        size: Object.freeze({ ...size }),
        placement: Object.freeze({ ...placement }),
        session:
          placement.surface === 'session-panel' && session
            ? Object.freeze({ ...session })
            : undefined,
        t,
        log,
        CapabilityError,
      })
    },
    dispose() {
      if (disposed) return
      disposed = true
      timers.dispose()
      for (const release of [...subscriptions]) release()
      controller.abort(new DOMException('Component unmounted', 'AbortError'))
    },
  }
}

/** 把请求层的 ApiError 转成带 Runtime 错误码的 ComponentRequestError（宿主的存储服务也用它）。 */
export function toComponentRequestError(error: unknown): ComponentRequestError {
  if (error && typeof error === 'object') {
    const data = (error as { data?: { code?: unknown; error?: unknown } }).data
    const code = data?.code ?? data?.error
    if (typeof code === 'string' && /^[a-z0-9_]{1,64}$/.test(code))
      return new ComponentRequestError(code)
    const kind = (error as { kind?: unknown }).kind
    if (kind === 'cancelled' || (error as { name?: unknown }).name === 'AbortError')
      return new ComponentRequestError('component_request_aborted')
    if (kind === 'timeout') return new ComponentRequestError('component_request_timeout')
  }
  return new ComponentRequestError('component_request_failed')
}
