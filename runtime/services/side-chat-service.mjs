import { randomUUID } from 'node:crypto'

/**
 * @typedef {{ version: 1, parentSessionId: string, lastActivityAt: string, expiresAt: string }} SideChatMetadata
 * @typedef {{ cwd?: string, model?: string, thinkingLevel?: string, executionMode?: string, permissionMode?: string, runMode?: string }} InheritedSessionSettings
 * @typedef {InheritedSessionSettings & { id: string }} SideChatSessionSummary
 * @typedef {InheritedSessionSettings & { sideChat?: SideChatMetadata, manual?: boolean }} SideChatSessionMetadata
 * @typedef {{ id: string, cwd?: string, metadata: InheritedSessionSettings & { sideChat: SideChatMetadata, manual: true } }} CreateSideChatInput
 * @typedef {{ session: SideChatSessionSummary, expiresAt: string, created: boolean } | { session: null, expiresAt: null, created: false }} SideChatResponse
 * @typedef {object} SideChatDependencies
 * @property {() => Record<string, SideChatSessionMetadata>} getMetadata
 * @property {() => Promise<void>} saveMetadata
 * @property {(id: string) => Promise<SideChatSessionSummary | null>} getSummary
 * @property {(id: string) => Promise<boolean>} [hasSession]
 * @property {(input: CreateSideChatInput) => Promise<SideChatSessionSummary>} createSession
 * @property {(id: string) => Promise<unknown>} deleteSession
 * @property {(id: string) => boolean} isProtected
 * @property {() => number} [now]
 */

export const SIDE_CHAT_TTL_MS = 24 * 60 * 60 * 1000
const SIDE_CHAT_PREFIX = 'side-'

export class SideChatError extends Error {
  /** @param {string} code @param {string} message @param {number} statusCode */
  constructor(code, message, statusCode) {
    super(message)
    this.code = code
    this.statusCode = statusCode
  }
}

/** @param {unknown} id @returns {id is string} */
export function isSideChatId(id) {
  return typeof id === 'string' && id.startsWith(SIDE_CHAT_PREFIX)
}

// 临时会话的持久化边界独立于常驻运行时缓存，读取与轮询不能延长保留期。
// 元数据中的 version 为增量格式标记；普通会话没有 sideChat 字段，不需要迁移。
export class SideChatService {
  /** @param {SideChatDependencies} dependencies */
  constructor({
    getMetadata,
    saveMetadata,
    getSummary,
    hasSession = async (id) => Boolean(await getSummary(id)),
    createSession,
    deleteSession,
    isProtected,
    now = Date.now,
  }) {
    this.getMetadata = getMetadata
    this.saveMetadata = saveMetadata
    this.getSummary = getSummary
    this.hasSession = hasSession
    this.createSession = createSession
    this.deleteSession = deleteSession
    this.isProtected = isProtected
    this.now = now
    /** @type {Map<string, Promise<unknown>>} */
    this.parentOperations = new Map()
    /** @type {Map<string, number>} */
    this.reservations = new Map()
    /** @type {Set<string>} */
    this.deleting = new Set()
    /** @type {Promise<void> | null} */
    this.sweepPromise = null
    this.closed = false
  }

  /** @param {string} id */
  isSideChat(id) {
    return isSideChatId(id) || Boolean(this.getMetadata()[id]?.sideChat)
  }

  /** @param {string} id */
  protected(id) {
    return Boolean(this.reservations.get(id) || this.isProtected(id))
  }

  /** @param {string} id */
  assertAvailable(id) {
    if (!this.isSideChat(id)) return
    const metadata = this.getMetadata()[id]?.sideChat
    if (!metadata || this.deleting.has(id)) {
      throw new SideChatError('side_chat_not_found', '临时侧聊已过期或已删除，请重新打开。', 404)
    }
    if (
      (!Number.isFinite(Date.parse(metadata.expiresAt)) ||
        Date.parse(metadata.expiresAt) <= this.now()) &&
      !this.protected(id)
    ) {
      throw new SideChatError('side_chat_expired', '临时侧聊已过期，请重新打开。', 410)
    }
  }

  /** @template T @param {string} id @param {() => T | Promise<T>} operation @returns {Promise<T>} */
  async withParent(id, operation) {
    const previous = this.parentOperations.get(id) || Promise.resolve()
    const pending = previous.catch(() => {}).then(operation)
    this.parentOperations.set(id, pending)
    try {
      return await pending
    } finally {
      if (this.parentOperations.get(id) === pending) this.parentOperations.delete(id)
    }
  }

  /** @param {string} parentId @returns {string[]} */
  children(parentId) {
    return Object.entries(this.getMetadata())
      .filter(([, metadata]) => metadata?.sideChat?.parentSessionId === parentId)
      .map(([id]) => id)
  }

  /** @param {string} parentId @param {{ create?: boolean }} [options] @returns {Promise<SideChatResponse>} */
  async get(parentId, { create = false } = {}) {
    return this.withParent(parentId, async () => {
      if (this.closed || this.deleting.has(parentId)) {
        throw new SideChatError('session_not_found', '会话不存在。', 404)
      }
      if (this.isSideChat(parentId)) {
        throw new SideChatError('invalid_side_chat_parent', '临时侧聊不能再创建侧聊。', 400)
      }
      const parent = await this.getSummary(parentId)
      if (!parent) throw new SideChatError('session_not_found', '会话不存在。', 404)
      for (const id of this.children(parentId)) {
        // children 已筛选属于当前父会话的 sideChat；这里只缩窄该领域不变量。
        const metadata = /** @type {SideChatMetadata} */ (this.getMetadata()[id].sideChat)
        const expiresAt = Date.parse(metadata.expiresAt)
        if ((!Number.isFinite(expiresAt) || expiresAt <= this.now()) && !this.protected(id)) {
          await this.deleteSession(id)
          continue
        }
        const session = await this.getSummary(id)
        if (session) return { session, expiresAt: metadata.expiresAt, created: false }
        await this.deleteSession(id)
      }
      if (!create) return { session: null, expiresAt: null, created: false }
      const timestamp = this.now()
      /** @type {SideChatMetadata} */
      const sideChat = {
        version: 1,
        parentSessionId: parentId,
        lastActivityAt: new Date(timestamp).toISOString(),
        expiresAt: new Date(timestamp + SIDE_CHAT_TTL_MS).toISOString(),
      }
      const session = await this.createSession({
        id: `${SIDE_CHAT_PREFIX}${randomUUID()}`,
        cwd: parent.cwd,
        metadata: {
          sideChat,
          manual: true,
          model: parent.model,
          thinkingLevel: parent.thinkingLevel,
          executionMode: parent.executionMode,
          permissionMode: parent.permissionMode,
          runMode: parent.runMode,
        },
      })
      return { session, expiresAt: sideChat.expiresAt, created: true }
    })
  }

  /** @param {string} id @returns {Promise<void>} */
  async touch(id) {
    const entry = this.getMetadata()[id]
    if (!entry?.sideChat) return
    const timestamp = this.now()
    entry.sideChat = {
      ...entry.sideChat,
      lastActivityAt: new Date(timestamp).toISOString(),
      expiresAt: new Date(timestamp + SIDE_CHAT_TTL_MS).toISOString(),
    }
    await this.saveMetadata()
  }

  /** @param {string} id @returns {Promise<(() => Promise<void>) | null>} */
  async beginRun(id) {
    if (!this.isSideChat(id)) return null
    this.assertAvailable(id)
    if (this.closed) throw new SideChatError('runtime_stopping', '服务正在关闭。', 503)
    // 在任何异步装配之前预留，防止清理器与首次发送同时取得同一个会话。
    this.reservations.set(id, (this.reservations.get(id) || 0) + 1)
    const release = () => {
      const count = (this.reservations.get(id) || 1) - 1
      if (count) this.reservations.set(id, count)
      else this.reservations.delete(id)
    }
    try {
      await this.touch(id)
    } catch (error) {
      release()
      throw error
    }
    return async () => {
      try {
        await this.touch(id)
      } finally {
        release()
      }
    }
  }

  /** @template T @param {string} id @param {() => T | Promise<T>} remove @returns {Promise<T>} */
  async deleteWithChildren(id, remove) {
    return this.withParent(id, async () => {
      this.deleting.add(id)
      try {
        for (const childId of this.children(id)) await this.deleteSession(childId)
        return await remove()
      } finally {
        this.deleting.delete(id)
      }
    })
  }

  /** @returns {Promise<void>} */
  sweep() {
    if (this.closed) return Promise.resolve()
    if (this.sweepPromise) return this.sweepPromise
    const pending = this.sweepExpired()
    this.sweepPromise = pending
    return pending.finally(() => {
      if (this.sweepPromise === pending) this.sweepPromise = null
    })
  }

  /** @returns {Promise<void>} */
  async sweepExpired() {
    const parents = new Set(
      Object.values(this.getMetadata())
        .map((entry) => entry?.sideChat?.parentSessionId)
        .filter(/** @returns {parentId is string} */ (parentId) => Boolean(parentId)),
    )
    for (const parentId of parents) {
      await this.withParent(parentId, async () => {
        const parentExists = await this.hasSession(parentId)
        for (const id of this.children(parentId)) {
          if (this.protected(id)) continue
          // 与 get 相同，children 保证这些条目具有侧聊元数据。
          const metadata = /** @type {SideChatMetadata} */ (this.getMetadata()[id].sideChat)
          const expiresAt = Date.parse(metadata.expiresAt)
          if (!parentExists || !Number.isFinite(expiresAt) || expiresAt <= this.now()) {
            await this.deleteSession(id)
          }
        }
      })
    }
  }

  /** @returns {Promise<void>} */
  async dispose() {
    this.closed = true
    await this.sweepPromise?.catch(() => {})
    await Promise.allSettled([...this.parentOperations.values()])
  }
}
