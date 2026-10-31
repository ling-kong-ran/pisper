// ctx.theme.useTheme（规格 5.6）：明暗跟随 <html class="dark">，密度 / 圆角 / 字号档来自 ui-store 写在
// <html> 上的 data-* 属性（ui-preferences.ts 的 applyUiPreferenceAttributes）。用 MutationObserver 观察，
// 不订阅 ui-store：主题“跟随系统 / 定时”的最终结果只体现在 class 上。token(name) 读 CSS 变量当前值。
import { useSyncExternalStore } from 'react'

export type ComponentThemeInfo = {
  mode: 'light' | 'dark'
  density: string
  fontSize: string
  radius: string
  token(name: string): string
}

const TOKEN_NAME = /^--[a-zA-Z0-9-]{1,64}$/

function snapshot(): string {
  if (typeof document === 'undefined') return 'light|comfortable|default|default'
  const root = document.documentElement
  return [
    root.classList.contains('dark') ? 'dark' : 'light',
    root.dataset.density || 'comfortable',
    root.dataset.fontScale || 'default',
    root.dataset.radius || 'default',
    // 数值字号写在内联样式变量里；放进快照，字号变化时组件也会重渲染。
    root.style.getPropertyValue('--ui-font-size').trim(),
  ].join('|')
}

function subscribe(listener: () => void) {
  if (typeof MutationObserver === 'undefined' || typeof document === 'undefined') return () => {}
  const observer = new MutationObserver(listener)
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['class', 'style', 'data-density', 'data-font-scale', 'data-radius'],
  })
  return () => observer.disconnect()
}

export function readThemeToken(name: unknown): string {
  if (typeof name !== 'string' || !TOKEN_NAME.test(name) || typeof document === 'undefined')
    return ''
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

const cache = new Map<string, ComponentThemeInfo>()

export function themeFromSnapshot(value: string): ComponentThemeInfo {
  const cached = cache.get(value)
  if (cached) return cached
  const [mode, density, fontScale, radius, uiFontSize] = value.split('|')
  const info = Object.freeze({
    mode: mode === 'dark' ? ('dark' as const) : ('light' as const),
    density: density || 'comfortable',
    fontSize: uiFontSize || fontScale || 'default',
    radius: radius || 'default',
    token: readThemeToken,
  })
  // 快照组合有限（几档设置的笛卡尔积），缓存只为让 useSyncExternalStore 拿到稳定引用。
  if (cache.size > 64) cache.clear()
  cache.set(value, info)
  return info
}

export function useComponentTheme(): ComponentThemeInfo {
  const value = useSyncExternalStore(subscribe, snapshot, snapshot)
  return themeFromSnapshot(value)
}
