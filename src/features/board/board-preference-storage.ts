// 看板偏好统一经 page-state-storage 落盘（键已登记在 shared/browser-preferences.mjs）。
// 隐私模式、配额或 WebView 策略可能禁用存储；偏好写不进去时仍保留本次页面里的选择。
import { createJSONStorage } from 'zustand/middleware'
import { pageStateStorage } from '@/lib/page-state-storage'

export function boardPreferenceStorage<T>() {
  return createJSONStorage<T>(() => ({
    getItem: (name) => {
      try {
        return pageStateStorage.getItem(name)
      } catch {
        return null
      }
    },
    setItem: (name, value) => {
      try {
        pageStateStorage.setItem(name, value)
      } catch {
        // 持久化失败不回滚当前选择，也不打断交互。
      }
    },
    removeItem: (name) => {
      try {
        pageStateStorage.removeItem(name)
      } catch {
        // 存储不可用时没有可清理的持久化状态。
      }
    },
  }))
}

export function persistedRecord(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {}
}

// 字符串列表：去重、限制条数与单项长度，丢弃非字符串。
export function persistedStrings(value: unknown, { limit = 200, maxLength = 4096 } = {}) {
  if (!Array.isArray(value)) return []
  const items: string[] = []
  for (const item of value) {
    if (typeof item !== 'string' || item.length > maxLength || items.includes(item)) continue
    items.push(item)
    if (items.length >= limit) break
  }
  return items
}
