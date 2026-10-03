import type { MouseEvent } from 'react'

// 移动端 WebView 的 target=_blank 不能保证打开系统浏览器，交给原生 opener 处理。
// Web 与桌面端保留锚点原生行为，包括键盘与上下文菜单。
export function openMobileExternalLink(
  event: Pick<MouseEvent<HTMLAnchorElement>, 'preventDefault'>,
  url: string,
): Promise<boolean> | null {
  if (typeof window === 'undefined' || !window.__PISPER_MOBILE_APP__) return null
  event.preventDefault()
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('移动端原生桥不可用。'))
  return invoke<boolean>('mobile_open_external_url', { url })
}
