// 会话导航状态只由明确的用户操作或 Agent 轮次结算改变；文件修改时间不代表未读。
// dismissAttention 是第 2 期新增的可选字段（只增不改）：看板“标记已处理”用它清掉失败注意力。
// activityOrder 是“最后操作”的入栈序号：新建、发消息、打开会话时取全局最大值 + 1。
// 不用时间戳排序——开发测试时会改系统时间，时间一倒退顺序就乱了；序号只增不减。
import { errorMessage } from '../../shared/errors.mjs'

const ORGANIZATION_PATCH_KEYS = new Set([
  'pinned',
  'archived',
  'read',
  'dismissAttention',
  'opened',
])
// 只能为 true 的字段：它们是“发生了一次操作”，没有反向的取值。
const TRUE_ONLY_PATCH_KEYS = new Set(['dismissAttention', 'opened'])

/**
 * @typedef {{ pinned: boolean, archived: boolean, unread: boolean, failed: boolean, lastCompletedAt: string | null, activityOrder: number }} SessionOrganizationState
 * @typedef {{ pinned?: boolean, archived?: boolean, read?: boolean, dismissAttention?: true, opened?: true }} SessionOrganizationPatch
 */

// code 是既有接口字段，客户端按它识别；message 只给日志和兜底展示，用统一错误码的英文写法。
export class SessionOrganizationInputError extends Error {
  /** @param {string} detail */
  constructor(detail) {
    super(errorMessage('invalid', { subject: 'session', detail }))
    this.code = 'invalid_session_organization'
  }
}

/** @param {unknown} value @returns {Record<string, unknown>} */
function record(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? /** @type {Record<string, unknown>} */ (value)
    : {}
}

/** @param {unknown} meta @returns {SessionOrganizationState} */
export function normalizeSessionOrganization(meta) {
  const stored = record(record(meta).organization)
  return {
    pinned: stored.pinned === true,
    archived: stored.archived === true,
    unread: stored.unread === true,
    failed: stored.failed === true,
    lastCompletedAt:
      typeof stored.lastCompletedAt === 'string' &&
      Number.isFinite(Date.parse(stored.lastCompletedAt))
        ? stored.lastCompletedAt
        : null,
    activityOrder: activityOrderValue(stored.activityOrder),
  }
}

/** @param {unknown} value */
function activityOrderValue(value) {
  return Number.isSafeInteger(value) && /** @type {number} */ (value) > 0
    ? /** @type {number} */ (value)
    : 0
}

// 下一个入栈序号：所有会话里最大的序号 + 1；还没有任何序号时从 1 开始。
/** @param {Record<string, unknown>} sessionMeta */
export function nextSessionActivityOrder(sessionMeta) {
  let max = 0
  for (const meta of Object.values(sessionMeta || {})) {
    const order = activityOrderValue(record(record(meta).organization).activityOrder)
    if (order > max) max = order
  }
  return max + 1
}

/** @param {unknown} meta @param {number} activityOrder */
export function recordSessionActivity(meta, activityOrder) {
  return {
    ...record(meta),
    organization: { ...normalizeSessionOrganization(meta), activityOrder },
  }
}

// 注意力原因的优先级：批准 > 等你回答（ask_user）> 失败。'question' 是新枚举值，旧客户端要容忍。
/** @param {unknown} meta @param {number} [pendingApprovalCount] @param {number} [pendingQuestionCount] */
export function projectSessionOrganization(
  meta,
  pendingApprovalCount = 0,
  pendingQuestionCount = 0,
) {
  const state = normalizeSessionOrganization(meta)
  const attentionReason =
    pendingApprovalCount > 0
      ? 'approval'
      : pendingQuestionCount > 0
        ? 'question'
        : state.failed
          ? 'failure'
          : null
  return {
    pinned: state.pinned,
    archived: state.archived,
    unread: state.unread,
    needsAttention: attentionReason !== null,
    attentionReason,
    lastCompletedAt: state.lastCompletedAt,
    // 0 表示升级前没有入栈过，客户端按 modified 排在入栈过的会话后面；旧客户端忽略这个字段。
    activityOrder: state.activityOrder,
  }
}

/** @param {unknown} input @returns {SessionOrganizationPatch} */
export function parseSessionOrganizationPatch(input) {
  if (input === null || typeof input !== 'object' || Array.isArray(input)) {
    throw new SessionOrganizationInputError('body must be an object')
  }
  const entries = Object.entries(input)
  if (!entries.length) throw new SessionOrganizationInputError('no fields given')
  for (const [key, value] of entries) {
    if (!ORGANIZATION_PATCH_KEYS.has(key))
      throw new SessionOrganizationInputError(`unknown field ${key.slice(0, 60)}`)
    if (typeof value !== 'boolean')
      throw new SessionOrganizationInputError(`${key} must be a boolean`)
    // 注意力只能由新一轮运行的结局重新设置，客户端不能把它“标回”未处理；“打开过”也没有反向。
    if (TRUE_ONLY_PATCH_KEYS.has(key) && value !== true)
      throw new SessionOrganizationInputError(`${key} must be true`)
  }
  return /** @type {SessionOrganizationPatch} */ (Object.fromEntries(entries))
}

// opened 时由调用方给出这次的入栈序号（nextSessionActivityOrder），这里不读全局状态。
/** @param {unknown} meta @param {SessionOrganizationPatch} patch @param {number} [activityOrder] */
export function applySessionOrganizationPatch(meta, patch, activityOrder = 0) {
  const previous = normalizeSessionOrganization(meta)
  return {
    ...record(meta),
    organization: {
      ...previous,
      ...(Object.hasOwn(patch, 'pinned') ? { pinned: patch.pinned } : {}),
      ...(Object.hasOwn(patch, 'archived') ? { archived: patch.archived } : {}),
      ...(Object.hasOwn(patch, 'read') ? { unread: !patch.read } : {}),
      // 只清失败注意力；待批准的工具请求仍在等待，不能被“标记已处理”吞掉。
      ...(patch.dismissAttention === true ? { failed: false } : {}),
      ...(patch.opened === true && activityOrder > 0 ? { activityOrder } : {}),
    },
  }
}

/** @param {unknown} meta @param {{ failed: boolean, completedAt: string }} outcome */
export function recordSessionCompletion(meta, { failed, completedAt }) {
  const previous = normalizeSessionOrganization(meta)
  return {
    ...record(meta),
    organization: {
      ...previous,
      unread: true,
      failed: failed === true,
      lastCompletedAt: completedAt,
    },
  }
}
