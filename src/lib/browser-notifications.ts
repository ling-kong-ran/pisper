// 浏览器系统通知封装：优先走 Service Worker 的 showNotification（点击后可
// 聚焦窗口、支持 data.url），否则退化为 window.Notification。所有类型定义
// 都是“鸭子类型”以便测试注入 fake window/navigator；isSecureContext 不满足
// 时（纯 HTTP）无法注册 SW，直接返回 null 表示降级。
type NotificationPermissionState = NotificationPermission | 'unsupported'
type NotificationInstanceLike = {
  onclick: (() => void) | null
  close: () => void
}
type NotificationConstructorLike = {
  permission: NotificationPermission
  requestPermission: () => Promise<NotificationPermission>
  new (title: string, options?: NotificationOptions): NotificationInstanceLike
}
type ServiceWorkerRegistrationLike = {
  showNotification?: (title: string, options?: NotificationOptions) => Promise<void>
}
type BrowserWindowLike = {
  Notification?: NotificationConstructorLike
  isSecureContext?: boolean
  location?: { href?: string }
  focus?: () => void
}
type BrowserNavigatorLike = {
  serviceWorker?: {
    register: (
      path: string,
      options?: RegistrationOptions,
    ) => Promise<ServiceWorkerRegistrationLike>
    ready: Promise<ServiceWorkerRegistrationLike>
  }
}
type BrowserGlobalsOptions = {
  windowRef?: BrowserWindowLike
  navigatorRef?: BrowserNavigatorLike
  forceRegistration?: boolean
}

let serviceWorkerRegistrationPromise: Promise<ServiceWorkerRegistrationLike> | undefined

// 归一化 window/navigator 引用：允许测试环境注入 fake 实现，
// 非浏览器环境返回 undefined 让上层安全降级。
function browserGlobals(options: BrowserGlobalsOptions = {}) {
  return {
    windowRef:
      options.windowRef ||
      (typeof window !== 'undefined' ? (window as unknown as BrowserWindowLike) : undefined),
    navigatorRef:
      options.navigatorRef ||
      (typeof navigator !== 'undefined'
        ? (navigator as unknown as BrowserNavigatorLike)
        : undefined),
  }
}

// 读取当前通知权限；环境不支持时返回 'unsupported'。
export function getBrowserNotificationPermission(
  options: BrowserGlobalsOptions = {},
): NotificationPermissionState {
  const { windowRef } = browserGlobals(options)
  const NotificationApi = windowRef?.Notification
  if (!NotificationApi) return 'unsupported'
  return NotificationApi.permission
}

// 请求通知权限：已是 granted/denied 直接返回，default 才弹系统询问。
export async function requestBrowserNotificationPermission(
  options: BrowserGlobalsOptions = {},
): Promise<NotificationPermissionState> {
  const { windowRef } = browserGlobals(options)
  const NotificationApi = windowRef?.Notification
  if (!NotificationApi) return 'unsupported'
  if (NotificationApi.permission !== 'default') return NotificationApi.permission
  return NotificationApi.requestPermission()
}

// 准备 Service Worker（注册并等待 ready），供系统通知回退链路使用；
// 非安全上下文或没有 SW 能力时返回 null（降级为 window.Notification）。
export async function prepareBrowserNotifications(
  options: BrowserGlobalsOptions = {},
): Promise<ServiceWorkerRegistrationLike | null> {
  const { windowRef, navigatorRef } = browserGlobals(options)
  const serviceWorker = navigatorRef?.serviceWorker
  if (!windowRef?.isSecureContext || !serviceWorker) return null
  if (!serviceWorkerRegistrationPromise || options.forceRegistration) {
    serviceWorkerRegistrationPromise = serviceWorker
      .register('/notification-sw.js', { scope: '/' })
      .then(() => serviceWorker.ready)
      .catch((error) => {
        serviceWorkerRegistrationPromise = undefined
        throw error
      })
  }
  return serviceWorkerRegistrationPromise
}

// 展示系统通知：优先 SW showNotification（点击带 data.url 可聚焦窗口），
// 否则 window.Notification；未授权或环境不支持时抛错供调用方提示。
export async function showBrowserSystemNotification(
  {
    title,
    body = '',
    tag = '',
    url = '',
  }: {
    title: string
    body?: string
    tag?: string
    url?: string
  },
  options: BrowserGlobalsOptions = {},
) {
  const { windowRef } = browserGlobals(options)
  const NotificationApi = windowRef?.Notification
  if (!windowRef || !NotificationApi) throw new Error('当前浏览器不支持系统通知。')
  if (NotificationApi.permission !== 'granted')
    throw new Error('通知权限未授权，请在浏览器站点设置中允许通知。')

  const registration = await prepareBrowserNotifications(options)
  const notificationOptions = {
    body: String(body || ''),
    tag: String(tag || ''),
    data: { url: String(url || windowRef.location?.href || '/') },
  }
  if (registration?.showNotification) {
    await registration.showNotification(String(title || 'Pisper'), notificationOptions)
    return { shown: true, transport: 'service-worker' }
  }

  const item = new NotificationApi(String(title || 'Pisper'), notificationOptions)
  item.onclick = () => {
    windowRef.focus?.()
    item.close()
  }
  return { shown: true, transport: 'window' }
}

export function resetBrowserNotificationRegistrationForTests() {
  serviceWorkerRegistrationPromise = undefined
}
