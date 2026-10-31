// 组件错误环形缓冲：宿主的错误卡、ctx.log.warn/error 批量 POST 到 /api/custom-ui/components/:id/errors，
// 工作室“日志”页从 GET 读回（计划 §3.9）。
// 只在内存里（重启清空），每个组件最多 100 条；入库前脱敏密钥并把主目录缩写成 ~，
// 日志里不出现凭据和完整个人路径（AGENTS 安全红线）。每个组件每分钟最多 20 条，超出计入 dropped。
import { homedir } from 'node:os'
import { COMPONENT_SURFACES, toWellFormed } from '../../shared/custom-ui-manifest.mjs'
import { redactSecretText } from '../security/secret-redaction.mjs'
import { CustomUiError } from './custom-ui-error.mjs'

export const ERROR_LOG_CAPACITY = 100
export const ERROR_LOG_PER_MINUTE = 20
const KINDS = new Set(['load', 'render', 'callback', 'slow', 'log'])
const LEVELS = new Set(['error', 'warn'])
const INSTANCE_ID = /^[a-zA-Z0-9._:-]{1,64}$/

/** @param {unknown} value @returns {value is Record<string, unknown>} */
function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

/** @param {string} value */
function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/**
 * @typedef {{
 *   at: string,
 *   kind: string,
 *   level: string,
 *   message: string,
 *   stack?: string,
 *   instanceId?: string,
 *   surface?: string,
 * }} ComponentErrorEntry
 */

export class ComponentErrorLog {
  /** @param {{ now?: () => number, home?: string }} [options] */
  constructor({ now = Date.now, home = homedir() } = {}) {
    this.now = now
    // 主目录可能出现在 file:// 堆栈、Windows 路径（反斜杠）里，两种写法都缩写。
    const homes = home ? [home, home.replace(/\\/g, '/')] : []
    this.homePattern = homes.length
      ? new RegExp([...new Set(homes)].map(escapeRegExp).join('|'), 'g')
      : null
    /** @type {Map<string, { entries: ComponentErrorEntry[], dropped: number, windowAt: number, windowCount: number }>} */
    this.logs = new Map()
  }

  /** @param {string} text @param {number} max */
  clean(text, max) {
    let value = redactSecretText(toWellFormed(text))
    if (this.homePattern) value = value.replace(this.homePattern, '~')
    return value.slice(0, max)
  }

  /** @param {string} id */
  log(id) {
    let log = this.logs.get(id)
    if (!log) {
      log = { entries: [], dropped: 0, windowAt: this.now(), windowCount: 0 }
      this.logs.set(id, log)
    }
    return log
  }

  /**
   * @param {string} id
   * @param {unknown} input {instanceId?, surface?, kind, level, message, stack?}
   */
  record(id, input) {
    if (!isRecord(input)) throw new CustomUiError('component_request_invalid', 400)
    const allowed = ['instanceId', 'surface', 'kind', 'level', 'message', 'stack']
    if (Object.keys(input).some((key) => !allowed.includes(key)))
      throw new CustomUiError('component_request_invalid', 400)
    const { kind, level, message, stack, instanceId, surface } = input
    if (typeof kind !== 'string' || !KINDS.has(kind))
      throw new CustomUiError('component_request_invalid', 400, { field: 'kind' })
    if (typeof level !== 'string' || !LEVELS.has(level))
      throw new CustomUiError('component_request_invalid', 400, { field: 'level' })
    if (typeof message !== 'string' || !message.trim())
      throw new CustomUiError('component_request_invalid', 400, { field: 'message' })
    if (stack !== undefined && typeof stack !== 'string')
      throw new CustomUiError('component_request_invalid', 400, { field: 'stack' })
    if (
      instanceId !== undefined &&
      (typeof instanceId !== 'string' || !INSTANCE_ID.test(instanceId))
    )
      throw new CustomUiError('component_request_invalid', 400, { field: 'instanceId' })
    if (
      surface !== undefined &&
      (typeof surface !== 'string' || !COMPONENT_SURFACES.includes(/** @type {any} */ (surface)))
    )
      throw new CustomUiError('component_request_invalid', 400, { field: 'surface' })
    const log = this.log(id)
    const now = this.now()
    if (now - log.windowAt >= 60_000) {
      log.windowAt = now
      log.windowCount = 0
    }
    if (log.windowCount >= ERROR_LOG_PER_MINUTE) {
      log.dropped += 1
      throw new CustomUiError('component_errors_rate_limited', 429)
    }
    log.windowCount += 1
    /** @type {ComponentErrorEntry} */
    const entry = {
      at: new Date(now).toISOString(),
      kind,
      level,
      message: this.clean(message, 500),
      ...(stack ? { stack: this.clean(stack, 4000) } : {}),
      ...(instanceId ? { instanceId } : {}),
      ...(surface ? { surface } : {}),
    }
    log.entries.push(entry)
    if (log.entries.length > ERROR_LOG_CAPACITY)
      log.entries.splice(0, log.entries.length - ERROR_LOG_CAPACITY)
    return { ok: true }
  }

  /** @param {string} id */
  list(id) {
    const log = this.logs.get(id)
    return {
      entries: log ? log.entries.map((entry) => ({ ...entry })) : [],
      dropped: log?.dropped || 0,
    }
  }

  /** @param {string} id */
  forget(id) {
    this.logs.delete(id)
  }
}
