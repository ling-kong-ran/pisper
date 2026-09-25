import { create } from 'zustand'
import { persist, type PersistStorage } from 'zustand/middleware'

type FloatingWidgetsPreferences = { prefs: Record<string, boolean> }
type FloatingWidgetsState = FloatingWidgetsPreferences & {
  storageError: boolean
  setVisible: (componentId: string, visible: boolean) => void
}

const MAX_PREFERENCES = 128
const COMPONENT_ID_PATTERN = /^[a-z0-9][a-z0-9._-]{0,63}$/

function isRecord(value: unknown): value is Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return false
  const prototype = Object.getPrototypeOf(value)
  return prototype === Object.prototype || prototype === null
}

function validComponentId(value: unknown): value is string {
  return typeof value === 'string' && COMPONENT_ID_PATTERN.test(value)
}

function restorePrefs(value: unknown): Record<string, boolean> {
  const prefs: Record<string, boolean> = {}
  if (!isRecord(value)) return prefs
  for (const key of Object.keys(value)) {
    if (Object.keys(prefs).length >= MAX_PREFERENCES) break
    const descriptor = Object.getOwnPropertyDescriptor(value, key)
    if (
      validComponentId(key) &&
      descriptor &&
      'value' in descriptor &&
      typeof descriptor.value === 'boolean'
    )
      prefs[key] = descriptor.value
  }
  return prefs
}

function restorePreferences(value: unknown): FloatingWidgetsPreferences {
  if (!isRecord(value) || Object.keys(value).some((key) => key !== 'prefs')) return { prefs: {} }
  const descriptor = Object.getOwnPropertyDescriptor(value, 'prefs')
  return { prefs: restorePrefs(descriptor && 'value' in descriptor ? descriptor.value : null) }
}

// 模板仅提供默认项；用户明确关闭的组件不会被路由切换或模板重新应用自动打开。
export function resolveFloatingWidgetIds(
  defaults: readonly string[],
  preferences: Readonly<Record<string, boolean>>,
): string[] {
  const prefs = restorePrefs(preferences)
  const ids = new Set<string>()
  for (const id of defaults) {
    if (validComponentId(id) && prefs[id] !== false) ids.add(id)
  }
  for (const [id, visible] of Object.entries(prefs)) {
    if (visible) ids.add(id)
  }
  return [...ids]
}

export const useFloatingWidgetsStore = create<FloatingWidgetsState>()((set, get, api) => {
  let storageError = false
  const reportStorage = (failed: boolean) => {
    storageError = failed
    // 避开持久化包装，报告写入失败不能再次触发相同写入。
    if (get() && get().storageError !== failed) set({ storageError: failed })
  }
  const storage: PersistStorage<FloatingWidgetsPreferences> = {
    getItem: (key) => {
      try {
        const raw = window.localStorage.getItem(key)
        if (raw === null) {
          reportStorage(false)
          return null
        }
        const envelope: unknown = JSON.parse(raw)
        reportStorage(false)
        // 不识别的版本只降级内存状态，不把默认值写回以破坏原始数据。
        return {
          state:
            isRecord(envelope) && envelope.version === 1
              ? restorePreferences(envelope.state)
              : { prefs: {} },
          version: 1,
        }
      } catch {
        reportStorage(true)
        return { state: { prefs: {} }, version: 1 }
      }
    },
    setItem: (key, value) => {
      try {
        window.localStorage.setItem(key, JSON.stringify(value))
        reportStorage(false)
      } catch {
        reportStorage(true)
      }
    },
    removeItem: (key) => {
      try {
        window.localStorage.removeItem(key)
        reportStorage(false)
      } catch {
        reportStorage(true)
      }
    },
  }
  return persist<FloatingWidgetsState, [], [], FloatingWidgetsPreferences>(
    (update, read) => ({
      prefs: {},
      storageError: false,
      setVisible: (componentId, visible) => {
        if (!validComponentId(componentId))
          throw Object.assign(new Error('Invalid component ID'), { code: 'invalid_id' })
        if (typeof visible !== 'boolean')
          throw Object.assign(new Error('Invalid visibility'), { code: 'invalid_visibility' })
        const prefs = read().prefs
        if (prefs[componentId] === visible) return
        if (!Object.hasOwn(prefs, componentId) && Object.keys(prefs).length >= MAX_PREFERENCES)
          throw Object.assign(new Error('Too many floating preferences'), {
            code: 'preferences_limit',
          })
        update({ prefs: { ...prefs, [componentId]: visible } })
      },
    }),
    {
      name: 'pisper-floating-widgets',
      version: 1,
      storage,
      partialize: ({ prefs }) => ({ prefs }),
      merge: (persisted, current) => ({
        ...current,
        ...restorePreferences(persisted),
        storageError,
      }),
    },
  )(set, get, api)
})
