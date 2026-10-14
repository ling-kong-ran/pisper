// 会话导航状态只由明确的用户操作或 Agent 轮次结算改变；文件修改时间不代表未读。
const ORGANIZATION_PATCH_KEYS = new Set(['pinned', 'archived', 'read'])

/**
 * @typedef {{ pinned: boolean, archived: boolean, unread: boolean, failed: boolean, lastCompletedAt: string | null }} SessionOrganizationState
 * @typedef {{ pinned?: boolean, archived?: boolean, read?: boolean }} SessionOrganizationPatch
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

/** @param {unknown} meta @param {number} [pendingApprovalCount] */
export function projectSessionOrganization(meta, pendingApprovalCount = 0) {
  const state = normalizeSessionOrganization(meta)
  const attentionReason = pendingApprovalCount > 0 ? 'approval' : state.failed ? 'failure' : null
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
