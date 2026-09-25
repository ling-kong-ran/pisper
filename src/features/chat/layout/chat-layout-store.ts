import { create } from 'zustand'
import { persist, type PersistStorage } from 'zustand/middleware'
import { useSessionContextStore } from '@/features/chat/session-context-store'
import {
  CHAT_LAYOUT_SAVED_LIMIT,
  ChatLayoutValidationError,
  DEFAULT_CHAT_LAYOUT,
  equalChatLayoutContent,
  parseChatLayout,
  type ChatLayoutTemplate,
} from '@/features/chat/layout/chat-layout'

export type SavedChatLayout = { id: string; template: ChatLayoutTemplate }

type ChatLayoutPreferences = {
  active: ChatLayoutTemplate
  saved: SavedChatLayout[]
}

type ChatLayoutState = ChatLayoutPreferences & {
  revision: number
  storageError: boolean
  apply: (template: ChatLayoutTemplate) => void
  save: (template: ChatLayoutTemplate) => string
  importTemplate: (template: ChatLayoutTemplate) => string
  rename: (id: string, name: string) => void
  remove: (id: string) => void
  reset: () => void
}

function defaults(): ChatLayoutPreferences {
  return { active: parseChatLayout(DEFAULT_CHAT_LAYOUT), saved: [] }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function templateOrDefault(value: unknown) {
  try {
    return parseChatLayout(value)
  } catch {
    return parseChatLayout(DEFAULT_CHAT_LAYOUT)
  }
}

function nameKey(template: ChatLayoutTemplate) {
  return template.name.toLowerCase()
}

function importedName(template: ChatLayoutTemplate, saved: SavedChatLayout[]): string {
  const names = new Set(saved.map((entry) => nameKey(entry.template)))
  if (!names.has(nameKey(template))) return template.name
  for (let copy = 2; ; copy += 1) {
    const suffix = ` (${copy})`
    const base = [...template.name]
      .slice(0, 80 - suffix.length)
      .join('')
      .trimEnd()
    const name = `${base}${suffix}`
    if (!names.has(name.toLowerCase())) return name
  }
}

function restorePreferences(value: unknown): ChatLayoutPreferences {
  if (!isRecord(value) || Object.keys(value).some((key) => key !== 'active' && key !== 'saved'))
    return defaults()
  const saved: SavedChatLayout[] = []
  if (Array.isArray(value.saved)) {
    for (const entry of value.saved) {
      if (saved.length >= CHAT_LAYOUT_SAVED_LIMIT) break
      if (
        !isRecord(entry) ||
        Object.keys(entry).some((key) => key !== 'id' && key !== 'template') ||
        typeof entry.id !== 'string' ||
        !/^[a-z0-9-]{1,80}$/.test(entry.id) ||
        saved.some((item) => item.id === entry.id)
      )
        continue
      try {
        const template = parseChatLayout(entry.template)
        if (!saved.some((item) => nameKey(item.template) === nameKey(template)))
          saved.push({ id: entry.id, template })
      } catch {
        // 一个旧模板或损坏条目不应让其他可用模板一起消失。
      }
    }
  }
  return { active: templateOrDefault(value.active), saved }
}

let nextId = 0

function createId(saved: SavedChatLayout[]) {
  let id: string
  do {
    nextId += 1
    id = globalThis.crypto?.randomUUID?.() ?? `layout-${Date.now().toString(36)}-${nextId}`
  } while (saved.some((item) => item.id === id))
  return id
}

export const useChatLayoutStore = create<ChatLayoutState>()((set, get, api) => {
  let storageError = false
  const reportStorage = (failed: boolean) => {
    storageError = failed
    // 直接使用持久化中间件之外的 set，避免报告写入失败时递归尝试写入。
    if (get() && get().storageError !== failed) set({ storageError: failed })
  }
  const storage: PersistStorage<ChatLayoutPreferences> = {
    getItem: (key) => {
      try {
        const raw = window.localStorage.getItem(key)
        if (raw === null) {
          reportStorage(false)
          return null
        }
        const envelope: unknown = JSON.parse(raw)
        reportStorage(false)
        // 未知版本不猜测迁移，也不在读取时覆盖原数据；用户主动应用后才写入当前格式。
        return {
          state:
            isRecord(envelope) && envelope.version === 1
              ? restorePreferences(envelope.state)
              : defaults(),
          version: 1,
        }
      } catch {
        reportStorage(true)
        return { state: defaults(), version: 1 }
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
  return persist<ChatLayoutState, [], [], ChatLayoutPreferences>(
    (update, read) => ({
      ...defaults(),
      revision: 0,
      storageError: false,
      apply: (value) => {
        const active = parseChatLayout(value)
        // 模板只在应用时设置起始宽度，后续拖动仍由上下文宽度 Store 独立管理。
        useSessionContextStore.getState().setWidth(active.desktop.contextWidth)
        update((state) => ({ active, revision: state.revision + 1 }))
      },
      save: (value) => {
        const template = parseChatLayout(value)
        const saved = read().saved
        const existing = saved.find((item) => nameKey(item.template) === nameKey(template))
        if (!existing && saved.length >= CHAT_LAYOUT_SAVED_LIMIT)
          throw new ChatLayoutValidationError('saved_limit', 'saved')
        const id = existing?.id ?? createId(saved)
        update({
          saved: existing
            ? saved.map((item) => (item.id === id ? { id, template } : item))
            : [...saved, { id, template }],
        })
        return id
      },
      importTemplate: (value) => {
        const parsed = parseChatLayout(value)
        const saved = read().saved
        if (saved.length >= CHAT_LAYOUT_SAVED_LIMIT)
          throw new ChatLayoutValidationError('saved_limit', 'saved')
        const template = parseChatLayout({ ...parsed, name: importedName(parsed, saved) })
        const id = createId(saved)
        update({ saved: [...saved, { id, template }] })
        return id
      },
      rename: (id, name) => {
        const { saved, active } = read()
        const entry = saved.find((item) => item.id === id)
        if (!entry) throw new ChatLayoutValidationError('not_found', 'saved')
        const template = parseChatLayout({ ...entry.template, name })
        if (saved.some((item) => item.id !== id && nameKey(item.template) === nameKey(template)))
          throw new ChatLayoutValidationError('duplicate_name', 'name')
        if (entry.template.name === template.name) return
        // 当前模板必须同时匹配旧名称与内容，不能误改同内容、另一个名称的独立模板。
        const renameActive =
          active.name === entry.template.name && equalChatLayoutContent(active, entry.template)
        update({
          saved: saved.map((item) => (item.id === id ? { id, template } : item)),
          ...(renameActive ? { active: { ...active, name: template.name } } : {}),
        })
      },
      remove: (id) => {
        if (read().saved.some((item) => item.id === id))
          update((state) => ({ saved: state.saved.filter((item) => item.id !== id) }))
      },
      reset: () => read().apply(DEFAULT_CHAT_LAYOUT),
    }),
    {
      name: 'pisper-chat-layout',
      version: 1,
      storage,
      partialize: ({ active, saved }) => ({ active, saved }),
      merge: (persisted, current) => ({
        ...current,
        ...restorePreferences(persisted),
        storageError,
      }),
    },
  )(set, get, api)
})
