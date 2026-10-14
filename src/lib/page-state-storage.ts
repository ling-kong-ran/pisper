// 本机桌面/手机 Runtime 使用随机回环端口，localStorage 因端口变化不能单独充当持久化来源。
// 仅同步共享协议列出的界面偏好；一次性会话请求、预览 URL 和凭据不写入此文件。
import {
  BROWSER_PREFERENCE_KEYS,
  parseBrowserPreferenceSnapshot,
} from '@shared/browser-preferences.mjs'

const ENDPOINT = '/api/local/browser-preferences'
const JOURNAL_KEY = 'pisper.browserPreferences.pending'
const allowed = new Set(BROWSER_PREFERENCE_KEYS)
type PendingEntry = { value: string | null; revision: number }
const pending = new Map<string, PendingEntry>()
let activeBatch: Map<string, PendingEntry> | null = null
let writing = false
let retryTimer: number | null = null
let mirrorAvailable = false
let lastRevision = 0

function assertPageStateKey(key: string) {
  if (!allowed.has(key)) throw new Error('Page state key is not registered')
}

function readJournal(): Record<string, PendingEntry> {
  try {
    const value: unknown = JSON.parse(window.sessionStorage.getItem(JOURNAL_KEY) || '{}')
    if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
    return Object.fromEntries(
      Object.entries(value).filter(
        ([key, entry]) =>
          allowed.has(key) &&
          entry !== null &&
          typeof entry === 'object' &&
          'value' in entry &&
          (entry.value === null || typeof entry.value === 'string') &&
          'revision' in entry &&
          Number.isSafeInteger(entry.revision) &&
          entry.revision > 0,
      ),
    ) as Record<string, PendingEntry>
  } catch {
    return {}
  }
}

function writeJournal(journal: Record<string, PendingEntry>) {
  try {
    if (Object.keys(journal).length)
      window.sessionStorage.setItem(JOURNAL_KEY, JSON.stringify(journal))
    else window.sessionStorage.removeItem(JOURNAL_KEY)
  } catch {
    // sessionStorage 不可用时，本次页面仍用内存队列写入 Runtime。
  }
}

function nextRevision() {
  lastRevision = Math.max(lastRevision + 1, Date.now() * 1000)
  return lastRevision
}

function recordPending(key: string, value: string | null) {
  const entry = { value, revision: nextRevision() }
  writeJournal({ ...readJournal(), [key]: entry })
  return entry
}

function acknowledge(batch: Map<string, PendingEntry>) {
  const journal = readJournal()
  for (const [key, entry] of batch) {
    if (journal[key]?.revision === entry.revision) delete journal[key]
  }
  writeJournal(journal)
}

function batchPayload(batch: Map<string, PendingEntry>) {
  return {
    updates: Object.fromEntries([...batch].map(([key, entry]) => [key, entry.value])),
    revisions: Object.fromEntries([...batch].map(([key, entry]) => [key, entry.revision])),
  }
}

async function writeBatch(batch: Map<string, PendingEntry>) {
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), 3000)
  try {
    const response = await window.fetch(ENDPOINT, {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(batchPayload(batch)),
      signal: controller.signal,
    })
    if (!response.ok) throw new Error('Browser preferences could not be stored')
  } finally {
    clearTimeout(timeout)
  }
}

function scheduleRetry() {
  if (retryTimer !== null) return
  retryTimer = window.setTimeout(() => {
    retryTimer = null
    void flush()
  }, 2000)
}

async function flush() {
  if (writing) return
  writing = true
  while (pending.size) {
    const updates = new Map(pending)
    pending.clear()
    activeBatch = updates
    try {
      await writeBatch(updates)
      acknowledge(updates)
    } catch {
      // 新操作比失败批次更新；重试时不能用旧值覆盖用户刚做的选择。
      for (const [key, entry] of updates) {
        if (!pending.has(key)) pending.set(key, entry)
      }
      activeBatch = null
      scheduleRetry()
      break
    }
    activeBatch = null
  }
  writing = false
}

function queue(key: string, entry: PendingEntry) {
  if (!allowed.has(key)) return
  if (!mirrorAvailable) return
  pending.set(key, entry)
  void flush()
}

function flushOnPageHide() {
  if (!mirrorAvailable) return
  const updates = new Map(activeBatch || [])
  for (const [key, entry] of pending) {
    if ((updates.get(key)?.revision || 0) < entry.revision) updates.set(key, entry)
  }
  if (!updates.size) return
  const body = new Blob([JSON.stringify(batchPayload(updates))], { type: 'application/json' })
  if (!navigator.sendBeacon?.(ENDPOINT, body)) {
    void window
      .fetch(ENDPOINT, { method: 'POST', credentials: 'same-origin', body, keepalive: true })
      .catch(() => {})
  }
}

// 通用页面状态接口：Zustand persist 可直接作为 storage 使用，普通页面也可按键读写。
// 白名单在 shared/browser-preferences.mjs 中维护，避免把临时请求或凭据落盘。
export const pageStateStorage = {
  getItem: (key: string) => {
    assertPageStateKey(key)
    return window.localStorage.getItem(key)
  },
  setItem: (key: string, value: string) => {
    assertPageStateKey(key)
    window.localStorage.setItem(key, value)
    queue(key, recordPending(key, value))
  },
  removeItem: (key: string) => {
    assertPageStateKey(key)
    window.localStorage.removeItem(key)
    queue(key, recordPending(key, null))
  },
}

// 必须在导入任何 Zustand/i18n Store 之前恢复，避免默认值先写入并覆盖上次的选择。
export async function restorePageState() {
  let storage: Storage
  try {
    storage = window.localStorage
  } catch {
    return
  }
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), 3000)
  try {
    const response = await window.fetch(ENDPOINT, {
      credentials: 'same-origin',
      cache: 'no-store',
      signal: controller.signal,
    })
    if (response.ok) {
      const snapshot = parseBrowserPreferenceSnapshot(await response.json())
      mirrorAvailable = true
      const journal = readJournal()
      const missing = new Map<string, PendingEntry>()
      const resend = new Map<string, PendingEntry>()
      for (const key of BROWSER_PREFERENCE_KEYS) {
        const serverRevision = snapshot.revisions[key] || 0
        lastRevision = Math.max(lastRevision, serverRevision)
        const unsaved = journal[key]
        if (unsaved && unsaved.revision > serverRevision) {
          lastRevision = Math.max(lastRevision, unsaved.revision)
          if (unsaved.value === null) storage.removeItem(key)
          else storage.setItem(key, unsaved.value)
          resend.set(key, unsaved)
          continue
        }
        if (unsaved) delete journal[key]
        if (Object.hasOwn(snapshot.values, key)) {
          const value = snapshot.values[key]
          if (value === null) storage.removeItem(key)
          else storage.setItem(key, value)
        } else {
          const value = storage.getItem(key)
          if (value !== null) {
            const entry = { value, revision: nextRevision() }
            journal[key] = entry
            missing.set(key, entry)
          }
        }
      }
      // 旧布局已退役，避免新端口首轮迁移继续携带其大型 JSON。
      storage.removeItem('pisper-chat-layout')
      writeJournal(journal)
      for (const [key, entry] of resend) queue(key, entry)
      // 升级首轮保留当前端口已存在的设置，并让后续随机端口都可恢复。
      // 恢复已完成；补写走后台队列，不能让未响应的 PUT 阻塞应用挂载。
      for (const [key, entry] of missing) pending.set(key, entry)
      if (missing.size) void flush()
    }
  } catch {
    // 后端暂时不可用时，仍使用当前端口可读取的旧偏好启动界面。
  } finally {
    clearTimeout(timeout)
  }
  window.addEventListener('pagehide', flushOnPageHide)
  window.addEventListener('online', () => void flush())
}
