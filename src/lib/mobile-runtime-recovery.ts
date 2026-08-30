type MobileRuntimeRecoveryDependencies = {
  isMobile: () => boolean
  resume: () => Promise<void>
  probe: () => Promise<boolean>
  reload: () => Promise<never>
}

export function createMobileRuntimeRecoveryCoordinator(
  dependencies: MobileRuntimeRecoveryDependencies,
) {
  let recoveryRequired = false
  let activeRecovery: Promise<void> | null = null

  const startRecovery = () => {
    if (!dependencies.isMobile()) {
      recoveryRequired = false
      return Promise.resolve()
    }
    if (!recoveryRequired) return activeRecovery || Promise.resolve()
    if (activeRecovery) return activeRecovery

    const attempt = (async () => {
      await dependencies.resume()
      if (!(await dependencies.probe())) await dependencies.reload()
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
  try {
    const response = await fetch(url, { cache: 'no-store' })
    if (!response.ok) return false
    const contentType = response.headers.get('Content-Type') || ''
    const body = await response.text()
    return contentType.includes('text/html') && body.includes('id="root"')
  } catch {
    return false
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
