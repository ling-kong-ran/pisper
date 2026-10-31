// 组件宿主的页面级单例与真实服务接线：模块加载器、样式注册表、熔断、安全模式、日志缓冲与上报器，
// 以及 ctx 背后的请求函数。全部惰性创建（首次调用时），模块导入没有副作用。
import {
  componentErrorCode,
  componentFetch,
  componentStorageGet,
  componentStorageRemove,
  componentStorageSet,
  reportComponentError,
} from '@/features/custom-ui/custom-ui-api'
import { createCircuitBreaker, type CircuitBreaker } from './circuit-breaker'
import {
  createErrorReporter,
  createLogBuffer,
  type ComponentLogBuffer,
  type ErrorReporter,
} from './log'
import { createModuleLoader, createStyleRegistry, type ModuleLoader } from './module-loader'
import { toComponentRequestError, type ComponentServices } from './sdk'
import { componentSessionFeed, useComponentSessionList } from './sessions-source'

let loader: ModuleLoader | null = null
let styles: ReturnType<typeof createStyleRegistry> | null = null
let breaker: CircuitBreaker | null = null
let logBuffer: ComponentLogBuffer | null = null
let reporter: ErrorReporter | null = null

export function componentModuleLoader(): ModuleLoader {
  // 模块 URL 由 componentModuleUrl 拼出（同源、带 rev），@vite-ignore：它不是构建期可解析的模块。
  loader ??= createModuleLoader({ importer: (url) => import(/* @vite-ignore */ url) })
  return loader
}

export function componentStyleRegistry() {
  styles ??= createStyleRegistry(document)
  return styles
}

export function componentCircuitBreaker(): CircuitBreaker {
  breaker ??= createCircuitBreaker()
  return breaker
}

// 安全模式单例在 safe-mode-instance.ts（询问组件也用它，不经过这里的重依赖）。
export { componentSafeMode } from './safe-mode-instance'

export function componentLogBuffer(): ComponentLogBuffer {
  logBuffer ??= createLogBuffer()
  return logBuffer
}

export function componentErrorReporter(): ErrorReporter {
  reporter ??= createErrorReporter({ send: reportComponentError })
  return reporter
}

/** ctx 背后的真实请求；notify 与打开会话由放置宿主传入（它们知道提示条与路由）。 */
export function createComponentServices(
  componentId: string,
  host: { notify(text: string): void; openSession(id: string): void },
): ComponentServices {
  return {
    notify: host.notify,
    openSession: host.openSession,
    useSessions: useComponentSessionList,
    subscribeSessions: (listener) => componentSessionFeed.subscribe(listener),
    fetch: (request, signal) => componentFetch(componentId, request, signal),
    storageGet: (key) =>
      componentStorageGet(componentId, key).then(
        (result) => result.value,
        (error: unknown) => {
          // 键不存在是正常结果（ctx.storage.get 返回 undefined），不是错误。
          if (componentErrorCode(error) === 'component_storage_missing') return undefined
          throw toComponentRequestError(error)
        },
      ),
    storageSet: (key, value) =>
      componentStorageSet(componentId, key, value).catch((error: unknown) => {
        throw toComponentRequestError(error)
      }),
    storageRemove: (key) =>
      componentStorageRemove(componentId, key).catch((error: unknown) => {
        throw toComponentRequestError(error)
      }),
  }
}
