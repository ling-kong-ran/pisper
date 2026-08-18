// Web 预览开关事件：跨组件广播预览请求，地址先规范化再校验，
// 非法输入返回 null 表示不打开。
import { STORAGE_KEYS } from '@/app/storage'
import { normalizeWebPreviewInput } from '@/lib/web-preview'

export const WEB_PREVIEW_OPEN_EVENT = 'pisper:web-preview-open'

export type WebPreviewOpenRequest = {
  url: string
}

// 发起 Web 预览：规范化 URL（非法返回 null 不触发），
// 持久化请求到 localStorage 并广播事件给预览面板。
export function requestWebPreview(url: string) {
  const normalized = normalizeWebPreviewInput(url, window.location.href)
  if (!normalized) return null
  const request = { url: normalized }
  localStorage.setItem(STORAGE_KEYS.webPreviewRequest, JSON.stringify(request))
  window.dispatchEvent(new CustomEvent(WEB_PREVIEW_OPEN_EVENT, { detail: request }))
  return request
}

// 消费 Web 预览请求：取出即删（跨重启只处理一次），无请求返回 null。
export function consumeWebPreviewRequest(): WebPreviewOpenRequest | null {
  const raw = localStorage.getItem(STORAGE_KEYS.webPreviewRequest)
  localStorage.removeItem(STORAGE_KEYS.webPreviewRequest)
  if (!raw) return null
  try {
    const value = JSON.parse(raw) as { url?: unknown }
    const url =
      typeof value?.url === 'string'
        ? normalizeWebPreviewInput(value.url, window.location.href)
        : null
    return url ? { url } : null
  } catch {
    return null
  }
}
