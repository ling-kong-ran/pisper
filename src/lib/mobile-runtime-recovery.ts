type MobileRuntimeRecoveryDependencies = {
  isMobile: () => boolean
  resume: () => Promise<void>
  probe: () => Promise<boolean>
  reload: () => Promise<void>
  // 各步骤的兜底时限；测试可注入更小值。
  timeouts?: { resumeMs?: number; probeMs?: number; reloadMs?: number }
}

// 恢复步骤的默认兜底时限：任何一步无限挂起（iOS 冻结 socket、原生启动卡死、
// 导航未生效）都会让全部 API 请求永远等待，因此每一步都必须有时限。
const LOOPBACK_PROBE_TIMEOUT_MS = 8_000
const DEFAULT_RECOVERY_TIMEOUTS = {
  resumeMs: 12_000,
  probeMs: LOOPBACK_PROBE_TIMEOUT_MS,
  reloadMs: 8_000,
}

// 超时只是解除调用方等待；底层工作（原生调用/fetch/导航）可能仍在进行，不做取消。
function settleWithin<T>(promise: Promise<T>, ms: number, onTimeout: () => T): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => resolve(onTimeout()), ms)
    void Promise.resolve(promise).then(
      (value) => {
        clearTimeout(timer)
        resolve(value)
      },
      (error: unknown) => {
        clearTimeout(timer)
        reject(error)
      },
    )
  })
}

export function createMobileRuntimeRecoveryCoordinator(
  dependencies: MobileRuntimeRecoveryDependencies,
) {
  let recoveryRequired = false
  let activeRecovery: Promise<void> | null = null
  const timeouts = { ...DEFAULT_RECOVERY_TIMEOUTS, ...dependencies.timeouts }

  const startRecovery = () => {
    if (!dependencies.isMobile()) {
      recoveryRequired = false
      return Promise.resolve()
    }
    if (!recoveryRequired) return activeRecovery || Promise.resolve()
    if (activeRecovery) return activeRecovery

    const attempt = (async () => {
      // resume 超时不直接判失败：本机运行时可能本来就在运行（如远程模式），
      // 继续用探测结果判断可用性。
      await settleWithin(dependencies.resume(), timeouts.resumeMs, () => undefined)
      const healthy = await settleWithin(dependencies.probe(), timeouts.probeMs, () => false)
      if (!healthy) {
        // 页面重载也可能挂起（导航未生效）；超时后放行请求，避免永远卡在恢复闸门。
        await settleWithin(dependencies.reload(), timeouts.reloadMs, () => undefined)
      }
      recoveryRequired = false
    })()
    activeRecovery = attempt.finally(() => {
      activeRecovery = null
    })
    return activeRecovery
  }

  return {
    markBackgrounded() {
      if (dependencies.isMobile()) recoveryRequired = true
    },
    recoverAfterForeground() {
      return startRecovery()
    },
    waitUntilReady() {
      return recoveryRequired ? startRecovery() : activeRecovery || Promise.resolve()
    },
  }
}

function nativeInvoke<T>(command: string): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('移动端原生桥不可用。'))
  return invoke<T>(command)
}

function isLoopbackPage() {
  return (
    window.location.protocol === 'http:' &&
    window.location.hostname === '127.0.0.1' &&
    Boolean(window.__PISPER_MOBILE_APP__)
  )
}

async function probeLoopbackPage() {
  if (!isLoopbackPage()) return true
  const url = new URL(window.location.href)
  url.pathname = '/'
  url.search = ''
  url.searchParams.set('_pisper_resume_probe', String(Date.now()))
  url.hash = ''
  const controller = new AbortController()
  const timeoutId = window.setTimeout(() => controller.abort(), LOOPBACK_PROBE_TIMEOUT_MS)
  try {
    // 探测 fetch 自身也要有时限：被 iOS 冻结的连接可能接受请求却永不应答，
    // 没有超时会拖着整个恢复闸门永远等待；显式 AbortController 兼容旧 WebView。
    const response = await fetch(url, { cache: 'no-store', signal: controller.signal })
    if (!response.ok) return false
    const contentType = response.headers.get('Content-Type') || ''
    const body = await response.text()
    return contentType.includes('text/html') && body.includes('id="root"')
  } catch {
    return false
  } finally {
    window.clearTimeout(timeoutId)
  }
}

async function reloadLoopbackPage(): Promise<never> {
  const url = new URL(window.location.href)
  url.searchParams.set('_pisper_recovery', String(Date.now()))
  window.location.replace(url.href)
  return new Promise<never>(() => undefined)
}

const mobileRuntimeRecovery = createMobileRuntimeRecoveryCoordinator({
  isMobile: () => typeof window !== 'undefined' && isLoopbackPage(),
  resume: () => nativeInvoke<void>('mobile_resume_local_runtime'),
  probe: probeLoopbackPage,
  reload: reloadLoopbackPage,
})

let installed = false

export function installMobileRuntimeForegroundRecovery() {
  if (installed || typeof document === 'undefined' || typeof window === 'undefined') return
  installed = true
  if (document.visibilityState === 'hidden') mobileRuntimeRecovery.markBackgrounded()

  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') {
      mobileRuntimeRecovery.markBackgrounded()
      return
    }
    void mobileRuntimeRecovery.recoverAfterForeground().catch(() => undefined)
  })
  window.addEventListener('pagehide', () => mobileRuntimeRecovery.markBackgrounded())
  window.addEventListener('pageshow', (event) => {
    if (!event.persisted) return
    void mobileRuntimeRecovery.recoverAfterForeground().catch(() => undefined)
  })
}

export function waitForMobileRuntimeReady() {
  return mobileRuntimeRecovery.waitUntilReady()
}
