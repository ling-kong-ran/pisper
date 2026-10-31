// 能力使用台账（规格 5.4、5.11，嫁接自 streams）：每个组件每项能力记“最近一次调用、目标、今天的次数、累计次数”，
// 工作室的能力清单据此显示“最近 · 今天 N 次”。网络与存储由 Runtime 在代理 / 存储路由里记，
// notify 与 sessions.* 只发生在浏览器里，由 ctx 批量上报（POST …/usage）。
//
// 只记主机、方法与能力名，不记查询参数、请求体、通知正文或会话内容（规格 5.8）。
// 落盘到 <dataDir>/custom-ui-usage.json：调用频繁，按 2 秒去抖合并写入；进程退出时未落盘的几次调用丢了也无妨。
import { CAPABILITY_TYPES } from '../../shared/custom-ui-manifest.mjs'
import { COMPONENT_ID_PATTERN, SerialJsonDocument } from './custom-ui-files.mjs'

const FLUSH_DELAY_MS = 2000
const MAX_TARGET_LENGTH = 120
const MAX_REPORT_ENTRIES = 32

/**
 * @typedef {(typeof CAPABILITY_TYPES)[number]} CapabilityType
 * @typedef {{ lastAt: string, target: string, day: string, today: number, total: number }} UsageEntry
 * @typedef {{ version: 1, components: Record<string, Partial<Record<CapabilityType, UsageEntry>>> }} UsageFile
 */

/** @param {number} time */
function localDay(time) {
  const date = new Date(time)
  const pad = (value) => String(value).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

/** @param {unknown} value */
function cleanTarget(value) {
  if (typeof value !== 'string') return ''
  // 只留可打印的单行文本：目标只是“GET api.github.com”这类摘要，不允许夹带换行或控制字符。
  return value
    .replace(/\p{Cc}+/gu, ' ')
    .trim()
    .slice(0, MAX_TARGET_LENGTH)
}

/** @param {unknown} raw @returns {UsageFile | null} */
function parseUsageFile(raw) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const source = /** @type {Record<string, unknown>} */ (raw)
  if (source.version !== 1 || !source.components || typeof source.components !== 'object')
    return null
  /** @type {UsageFile} */
  const file = { version: 1, components: {} }
  for (const [id, value] of Object.entries(source.components)) {
    if (!COMPONENT_ID_PATTERN.test(id) || !value || typeof value !== 'object') continue
    /** @type {Partial<Record<CapabilityType, UsageEntry>>} */
    const entries = {}
    for (const capability of CAPABILITY_TYPES) {
      const entry = /** @type {Record<string, unknown> | undefined} */ (
        /** @type {Record<string, unknown>} */ (value)[capability]
      )
      if (!entry || typeof entry !== 'object') continue
      if (
        typeof entry.lastAt !== 'string' ||
        Number.isNaN(Date.parse(entry.lastAt)) ||
        typeof entry.day !== 'string' ||
        !Number.isSafeInteger(entry.today) ||
        !Number.isSafeInteger(entry.total)
      )
        continue
      entries[capability] = {
        lastAt: entry.lastAt,
        target: cleanTarget(entry.target),
        day: entry.day.slice(0, 10),
        today: Math.max(0, Number(entry.today)),
        total: Math.max(0, Number(entry.total)),
      }
    }
    file.components[id] = entries
  }
  return file
}

export class ComponentUsageLedger {
  /** @param {{ path: string, now?: () => number }} options */
  constructor({ path, now = Date.now }) {
    this.now = now
    /** @type {SerialJsonDocument<UsageFile>} */
    this.document = new SerialJsonDocument({
      path,
      now,
      empty: () => ({ version: 1, components: {} }),
      parse: parseUsageFile,
    })
    /** @type {{ id: string, capability: CapabilityType, target: string, at: number, count: number }[]} */
    this.pending = []
    /** @type {ReturnType<typeof setTimeout> | null} */
    this.timer = null
  }

  /**
   * 记一次（或 count 次）调用；先进内存队列，去抖后合并落盘。
   * @param {string} id
   * @param {CapabilityType} capability
   * @param {{ target?: string, count?: number, at?: number }} [detail]
   */
  record(id, capability, { target = '', count = 1, at } = {}) {
    if (!COMPONENT_ID_PATTERN.test(id) || !CAPABILITY_TYPES.includes(capability)) return
    const times = Number.isSafeInteger(count) ? Math.min(Math.max(count, 1), 10_000) : 1
    const time = Number.isFinite(at) ? Math.min(Number(at), this.now()) : this.now()
    this.pending.push({ id, capability, target: cleanTarget(target), at: time, count: times })
    if (!this.timer) {
      this.timer = setTimeout(() => {
        this.timer = null
        void this.flush().catch(() => {})
      }, FLUSH_DELAY_MS)
      this.timer.unref?.()
    }
  }

  /**
   * ctx 的批量上报（POST …/usage）：{ entries: [{ capability, count, target?, at? }] }，按不可信输入校验。
   * 只接受 notify 与 sessions.*：网络与存储由 Runtime 自己记，不让组件替自己“记账”。
   * @param {string} id
   * @param {unknown} input
   * @returns {boolean} 输入是否合法
   */
  report(id, input) {
    if (!input || typeof input !== 'object' || Array.isArray(input)) return false
    const entries = /** @type {Record<string, unknown>} */ (input).entries
    if (!Array.isArray(entries) || entries.length > MAX_REPORT_ENTRIES) return false
    const accepted = []
    for (const entry of entries) {
      if (!entry || typeof entry !== 'object') return false
      const { capability, count, target, at } = /** @type {Record<string, unknown>} */ (entry)
      if (
        capability !== 'notify' &&
        capability !== 'sessions.read' &&
        capability !== 'sessions.open'
      )
        return false
      if (!Number.isSafeInteger(count) || Number(count) < 1) return false
      if (target !== undefined && typeof target !== 'string') return false
      if (at !== undefined && !(typeof at === 'string' && !Number.isNaN(Date.parse(at))))
        return false
      accepted.push({ capability, count: Number(count), target, at })
    }
    for (const entry of accepted)
      this.record(id, entry.capability, {
        count: entry.count,
        target: typeof entry.target === 'string' ? entry.target : '',
        ...(typeof entry.at === 'string' ? { at: Date.parse(entry.at) } : {}),
      })
    return true
  }

  /** 把内存队列合并写盘。 */
  flush() {
    if (this.timer) {
      clearTimeout(this.timer)
      this.timer = null
    }
    const batch = this.pending.splice(0)
    if (!batch.length) return this.document.read().then(() => undefined)
    return this.document.mutate((file) => {
      const today = localDay(this.now())
      for (const item of batch) {
        const entries = (file.components[item.id] ||= {})
        const current = entries[item.capability]
        const newer = !current || Date.parse(current.lastAt) <= item.at
        // day 是 today 计数所属的日期：跨天后先归零，再只把落在今天的调用计进去。
        const base = current && current.day === today ? current.today : 0
        entries[item.capability] = {
          lastAt: newer ? new Date(item.at).toISOString() : current.lastAt,
          target: newer ? item.target : current.target,
          day: today,
          today: base + (localDay(item.at) === today ? item.count : 0),
          total: (current?.total || 0) + item.count,
        }
      }
      return { value: file, result: undefined }
    })
  }

  /**
   * GET …/usage：按能力返回；“今天”按 Runtime 本地日期算，跨天后旧的计数归零。
   * @param {string} id
   */
  async list(id) {
    await this.flush()
    const file = await this.document.read()
    const today = localDay(this.now())
    const entries = file.components[id] || {}
    /** @type {Record<string, { lastAt: string, target: string, today: number, total: number }>} */
    const usage = {}
    for (const [capability, entry] of Object.entries(entries)) {
      if (!entry) continue
      usage[capability] = {
        lastAt: entry.lastAt,
        target: entry.target,
        today: entry.day === today ? entry.today : 0,
        total: entry.total,
      }
    }
    return usage
  }

  /** 组件删除后清掉它的台账（同名新组件不继承旧记录）。 @param {string} id */
  async forget(id) {
    this.pending = this.pending.filter((item) => item.id !== id)
    await this.document.mutate((file) => {
      if (!file.components[id]) return { result: undefined }
      delete file.components[id]
      return { value: file, result: undefined }
    })
  }

  /** 停掉去抖计时器并丢弃未落盘的队列（见文件头：丢几次调用无妨）。 */
  dispose() {
    if (this.timer) clearTimeout(this.timer)
    this.timer = null
    this.pending = []
  }
}
