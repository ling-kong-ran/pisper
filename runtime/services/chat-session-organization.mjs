// 会话导航状态只由明确的用户操作或 Agent 轮次结算改变；文件修改时间不代表未读。
// dismissAttention 是第 2 期新增的可选字段（只增不改）：看板“标记已处理”用它清掉失败注意力。
const ORGANIZATION_PATCH_KEYS = new Set(['pinned', 'archived', 'read', 'dismissAttention'])

/**
 * @typedef {{ pinned: boolean, archived: boolean, unread: boolean, failed: boolean, lastCompletedAt: string | null }} SessionOrganizationState
 * @typedef {{ pinned?: boolean, archived?: boolean, read?: boolean, dismissAttention?: true }} SessionOrganizationPatch
 */

export class SessionOrganizationInputError extends Error {
  /** @param {string} message */
  constructor(message) {
    super(message)
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
  }
}

/** @param {unknown} input @returns {SessionOrganizationPatch} */
export function parseSessionOrganizationPatch(input) {
  if (input === null || typeof input !== 'object' || Array.isArray(input)) {
    throw new SessionOrganizationInputError('会话组织状态请求必须是对象。')
  }
  const entries = Object.entries(input)
  if (!entries.length) throw new SessionOrganizationInputError('至少指定一个会话组织状态字段。')
  for (const [key, value] of entries) {
    if (!ORGANIZATION_PATCH_KEYS.has(key))
      throw new SessionOrganizationInputError('未知会话组织状态字段。')
    if (typeof value !== 'boolean')
      throw new SessionOrganizationInputError(`会话组织状态字段 ${key} 必须是布尔值。`)
    // 注意力只能由新一轮运行的结局重新设置，客户端不能把它“标回”未处理。
    if (key === 'dismissAttention' && value !== true)
      throw new SessionOrganizationInputError('dismissAttention 只能为 true。')
  }
  return /** @type {SessionOrganizationPatch} */ (Object.fromEntries(entries))
}

/** @param {unknown} meta @param {SessionOrganizationPatch} patch */
export function applySessionOrganizationPatch(meta, patch) {
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
