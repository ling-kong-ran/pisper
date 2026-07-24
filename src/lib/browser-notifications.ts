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

export function getBrowserNotificationPermission(
  options: BrowserGlobalsOptions = {},
): NotificationPermissionState {
  const { windowRef } = browserGlobals(options)
  const NotificationApi = windowRef?.Notification
  if (!NotificationApi) return 'unsupported'
  return NotificationApi.permission
}

export async function requestBrowserNotificationPermission(
  options: BrowserGlobalsOptions = {},
): Promise<NotificationPermissionState> {
  const { windowRef } = browserGlobals(options)
  const NotificationApi = windowRef?.Notification
  if (!NotificationApi) return 'unsupported'
  if (NotificationApi.permission !== 'default') return NotificationApi.permission
  return NotificationApi.requestPermission()
}

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
    await registration.showNotification(String(title || 'Vesper'), notificationOptions)
    return { shown: true, transport: 'service-worker' }
  }

  const item = new NotificationApi(String(title || 'Vesper'), notificationOptions)
  item.onclick = () => {
    windowRef.focus?.()
    item.close()
  }
  return { shown: true, transport: 'window' }
}

export function resetBrowserNotificationRegistrationForTests() {
  serviceWorkerRegistrationPromise = undefined
}
