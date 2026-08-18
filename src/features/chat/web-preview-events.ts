// Web 预览开关事件：跨组件广播预览请求，地址先规范化再校验，
// 非法输入返回 null 表示不打开。
import { STORAGE_KEYS } from '@/app/storage'
import { normalizeWebPreviewInput } from '@/lib/web-preview'

export const WEB_PREVIEW_OPEN_EVENT = 'pisper:web-preview-open'

export type WebPreviewOpenRequest = {
  url: string
}

export function requestWebPreview(url: string) {
  const normalized = normalizeWebPreviewInput(url, window.location.href)
  if (!normalized) return null
  const request = { url: normalized }
  localStorage.setItem(STORAGE_KEYS.webPreviewRequest, JSON.stringify(request))
  window.dispatchEvent(new CustomEvent(WEB_PREVIEW_OPEN_EVENT, { detail: request }))
  return request
}

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
