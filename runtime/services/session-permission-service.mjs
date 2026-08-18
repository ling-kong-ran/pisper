// 会话权限审批服务：按工具风险与权限模式（ask/auto/ignore）决定工具调用是否需人工审批，
// 管理待审批队列与已决审批缓存，并为文件修改类工具生成变更预览。
import { existsSync, realpathSync } from 'node:fs'
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { createHash, randomUUID } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { PLAN_ALL_TOOL_NAMES } from '../tools/app/plan-tool-names.mjs'
import { TOOL_CATALOG } from '../tools/registry.mjs'
import { createFileChangePreview, sameFileChangeSource } from './file-change-preview.mjs'
import { guardCommand } from '../tools/command-guard.mjs'

export const PERMISSION_MODES = new Set(['ask', 'auto', 'ignore'])
export const DEFAULT_PERMISSION_MODE = 'auto'
export const RESOLVED_APPROVAL_TTL_MS = 5 * 60_000
export const MAX_RESOLVED_APPROVALS = 256

// 工具风险元数据：插件目录 + 多 Agent 内部工具（不在目录中但同样需要风险分级）。
const TOOL_RISKS = new Map([
  ...TOOL_CATALOG.map((tool) => [tool.id, tool.risk]),
  // Internal multi-agent tools are not listed in the plugins catalog, but still need risk metadata.
  ['spawn_agent', 'medium'],
  ['list_agents', 'low'],
  ['send_message', 'low'],
  ['followup_task', 'medium'],
  ['wait_agent', 'low'],
  ['interrupt_agent', 'medium'],
])
const SENSITIVE_RISKS = new Set(['medium', 'high', '中风险', '高风险'])
const INTERNAL_SAFE_TOOLS = new Set(['get_goal', 'update_goal', ...PLAN_ALL_TOOL_NAMES])

function stableValue(value) {
  if (Array.isArray(value)) return value.map((item) => stableValue(item))
  if (value && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, child]) => [key, stableValue(child)]),
    )
  return value
}

// 审批键：对 bash 只按命令文本记忆审批，相同命令换超时仍复用用户之前的决定。
function approvalKey({ cwd, toolName, args }) {
  // For bash, remember the approval on the command text only, so an identical
  // command with a different timeout still reuses the user's earlier approval.
  const normalizedArgs =
    toolName === 'bash' && args && typeof args === 'object'
      ? { command: String(args.command || '') }
      : args
  return createHash('sha256')
    .update(JSON.stringify([resolve(cwd), toolName, stableValue(normalizedArgs)]))
    .digest('hex')
}

// 文件修改类工具（edit/write）附带变更预览需求。
function isPreviewedFileChange(toolName, requirement) {
  return Boolean(requirement && ['edit', 'write'].includes(toolName))
}

function publicFileChange(change) {
  if (!change) return undefined
  return { path: change.path, diff: change.diff, truncated: change.truncated }
}

function safeArgs(value, depth = 0, key = '') {
  if (depth > 3) return '[内容已省略]'
  if (/api.?key|password|passwd|secret|token/i.test(key)) return '[已隐藏敏感信息]'
  if (typeof value === 'string') {
    if (/^(?:data|image|content)$/i.test(key) && value.length > 500)
      return `[内容已省略，共 ${value.length} 字符]`
    return value.length > 800 ? `${value.slice(0, 800)}…` : value
  }
  if (Array.isArray(value)) return value.slice(0, 12).map((item) => safeArgs(item, depth + 1, key))
  if (value && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value)
        .slice(0, 30)
        .map(([childKey, child]) => [childKey, safeArgs(child, depth + 1, childKey)]),
    )
  return value
}

function canonicalPath(input) {
  let target = resolve(input)
  const suffix = []
  while (!existsSync(target)) {
    const parent = dirname(target)
    if (parent === target) break
    suffix.unshift(target.slice(parent.length).replace(/^[/\\]+/, ''))
    target = parent
  }
  try {
    target = realpathSync.native(target)
  } catch {}
  return suffix.reduce((current, part) => join(current, part), target)
}

function pathOutsideWorkspace(cwd, input) {
  const rawPath = String(input || '').trim()
  if (!rawPath) return false
  const root = canonicalPath(cwd)
  const unresolved = isAbsolute(rawPath) ? resolve(rawPath) : resolve(root, rawPath)
  const target = canonicalPath(unresolved)
  const result = relative(root, target)
  return result === '..' || result.startsWith(`..${sep}`) || isAbsolute(result)
}

export function permissionRequirement({ mode, executionMode, cwd, toolName, args, toolRisk }) {
  if (executionMode === 'full-access') return null
  if (INTERNAL_SAFE_TOOLS.has(toolName)) return null
  const risk = toolRisk || TOOL_RISKS.get(toolName) || 'high'
  const filePath = args?.path || args?.file_path
  if (toolName === 'skill_create' && args?.scope === 'global') {
    return {
      block: true,
      risk: 'high',
      reason: 'skill_create 只有在完全访问模式下才能创建全局技能。',
    }
  }
  if (
    ['read', 'ls', 'grep', 'find', 'edit', 'write'].includes(toolName) &&
    pathOutsideWorkspace(cwd, filePath)
  ) {
    return {
      block: true,
      risk: 'high',
      reason: `${toolName} 不能在当前执行模式下访问当前工作目录之外的文件。`,
    }
  }
  if (['read', 'ls', 'grep', 'find'].includes(toolName)) return null
  if (toolName === 'bash') {
    const decision = guardCommand(String(args?.command || ''))
    if (decision.blocked) {
      const reason = `命令守卫拦截：${decision.reason}。`
      if (decision.severity === 'block') return { block: true, risk: 'high', reason }
      return { risk: 'high', reason: `${reason}请确认后执行。` }
    }
  }
  if (mode === 'auto') {
    if (
      toolName === 'browser_automation' &&
      ['click', 'type'].includes(String(args?.action || ''))
    ) {
      return { risk: 'high', reason: '浏览器交互可能提交表单或改变远端状态，需要确认后执行。' }
    }
    return null
  }
  if (['edit', 'write', 'skill_create'].includes(toolName)) {
    return {
      risk: 'high',
      reason: `${toolName} 将修改当前工作区中的文件，需要确认后执行。`,
    }
  }
  if (toolName === 'bash') {
    return {
      risk: 'high',
      reason: 'Shell 命令将以当前操作系统用户权限运行，批准后可访问工作区之外的文件和网络。',
    }
  }
  if (mode === 'ignore') return null
  if (mode === 'ask' && SENSITIVE_RISKS.has(risk)) {
    return { risk, reason: `${toolName} 属于${risk}工具，需要确认后执行。` }
  }
  if (mode !== 'auto') return null
  if (toolName === 'browser_automation' && ['click', 'type'].includes(String(args?.action || ''))) {
    return { risk: 'high', reason: '浏览器交互可能提交表单或改变远端状态，需要确认后执行。' }
  }
  return null
}

export class SessionPermissionService {
  constructor({
    getMode,
    getExecutionMode,
    getToolRisk,
    approvalPath,
    getFileChangePreview = createFileChangePreview,
    timeoutMs = 10 * 60_000,
  } = {}) {
    this.getMode = getMode || (() => DEFAULT_PERMISSION_MODE)
    this.getExecutionMode = getExecutionMode || (() => '')
    this.getToolRisk = getToolRisk || (() => null)
    this.getFileChangePreview = getFileChangePreview
    this.timeoutMs = timeoutMs
    this.approvalPath = approvalPath
    this.approvalWrite = Promise.resolve()
    this.pending = new Map()
    this.resolved = new Map()
    this.remembered = new Map()
    this.emitters = new Map()
    this.installedSessions = new WeakSet()
  }

  pruneResolutions() {
    const cutoff = Date.now() - RESOLVED_APPROVAL_TTL_MS
    for (const [id, value] of this.resolved) {
      if (new Date(value.resolvedAt).getTime() < cutoff) this.resolved.delete(id)
    }
  }

  rememberResolution(resolution) {
    this.pruneResolutions()
    this.resolved.set(resolution.id, resolution)
    while (this.resolved.size > MAX_RESOLVED_APPROVALS)
      this.resolved.delete(this.resolved.keys().next().value)
  }

  attachEmitter(sessionId, emit) {
    if (emit) this.emitters.set(sessionId, emit)
  }

  detachEmitter(sessionId, emit) {
    if (!emit || this.emitters.get(sessionId) === emit) this.emitters.delete(sessionId)
  }

  emit(sessionId, event, data) {
    try {
      this.emitters.get(sessionId)?.(event, data)
    } catch {}
  }

  install(session, { sessionId, cwd }) {
    if (!session?.agent || this.installedSessions.has(session)) return
    this.installedSessions.add(session)
    const upstream = session.agent.beforeToolCall
    session.agent.beforeToolCall = async (context, signal) => {
      const upstreamResult = await upstream?.(context, signal)
      if (upstreamResult?.block) return upstreamResult
      return this.authorize({
        sessionId,
        cwd,
        toolName: context.toolCall.name,
        toolCallId: context.toolCall.id,
        args: context.args,
        signal,
      })
    }
  }

  async hasRememberedApproval(key) {
    if (!this.approvalPath) return this.remembered.has(key)
    const store = await readJson(this.approvalPath, { version: 1, approvals: {} })
    return Boolean(store.approvals?.[key])
  }

  rememberApproval(key, resolution) {
    if (!this.approvalPath) {
      this.remembered.set(key, resolution)
      return Promise.resolve()
    }
    this.approvalWrite = this.approvalWrite
      .catch(() => {})
      .then(async () => {
        const store = await readJson(this.approvalPath, { version: 1, approvals: {} })
        const approvals = {
          ...(store.approvals && typeof store.approvals === 'object' ? store.approvals : {}),
          [key]: {
            approvedAt: resolution.resolvedAt,
            toolName: resolution.toolName,
            cwd: resolution.cwd,
            command: resolution.command,
            args: resolution.args,
          },
        }
        await writeJsonAtomic(this.approvalPath, { version: 1, approvals })
      })
    return this.approvalWrite
  }

  async authorize({ sessionId, cwd, toolName, toolCallId, args, signal }) {
    const mode = PERMISSION_MODES.has(this.getMode(sessionId))
      ? this.getMode(sessionId)
      : DEFAULT_PERMISSION_MODE
    const executionMode = this.getExecutionMode(sessionId)
    const requirement = permissionRequirement({
      mode,
      executionMode,
      cwd,
      toolName,
      args,
      toolRisk: this.getToolRisk(toolName),
    })
    if (!requirement) return undefined
    if (requirement.block) return { block: true, reason: requirement.reason }
    const previewedFileChange = isPreviewedFileChange(toolName, requirement)
    let fileChange
    if (previewedFileChange) {
      try {
        fileChange = await this.getFileChangePreview({ cwd, toolName, args })
      } catch (error) {
        return {
          block: true,
          reason: error instanceof Error ? error.message : String(error),
        }
      }
    }
    const rememberedKey = approvalKey({ sessionId, cwd, toolName, args })
    if (!previewedFileChange && (await this.hasRememberedApproval(rememberedKey))) return undefined
    const approval = await this.requestApproval({
      sessionId,
      toolName,
      toolCallId,
      args,
      mode,
      fileChange,
      ...requirement,
      signal,
    })
    if (approval.approved) {
      if (fileChange) {
        try {
          const currentFileChange = await this.getFileChangePreview({ cwd, toolName, args })
          if (!sameFileChangeSource(fileChange, currentFileChange)) {
            return {
              block: true,
              reason: '目标文件在审核期间发生了变化，请重新请求修改并查看最新 Diff。',
            }
          }
        } catch (error) {
          return {
            block: true,
            reason: error instanceof Error ? error.message : String(error),
          }
        }
      } else {
        await this.rememberApproval(rememberedKey, {
          resolvedAt: new Date().toISOString(),
          toolName,
          cwd: resolve(cwd),
          command: toolName === 'bash' ? String(args?.command || '') : '',
          args: toolName === 'bash' ? undefined : safeArgs(args),
        })
      }
      return undefined
    }
    return { block: true, reason: approval.reason || `用户拒绝执行工具 ${toolName}。` }
  }

  requestApproval({
    sessionId,
    toolName,
    toolCallId,
    args,
    mode,
    risk,
    reason,
    fileChange,
    signal,
  }) {
    const id = randomUUID()
    const createdAt = new Date().toISOString()
    const publicApproval = {
      id,
      sessionId,
      toolName,
      toolCallId,
      args: safeArgs(args),
      mode,
      risk,
      reason,
      createdAt,
      ...(fileChange ? { fileChange: publicFileChange(fileChange) } : {}),
    }
    return new Promise((resolveApproval) => {
      let settled = false
      const settle = (approved, resolutionReason) => {
        if (settled) return this.resolved.get(id)
        settled = true
        clearTimeout(timer)
        signal?.removeEventListener('abort', abort)
        this.pending.delete(id)
        const resolution = {
          id,
          sessionId,
          approved: Boolean(approved),
          reason: resolutionReason || '',
          resolvedAt: new Date().toISOString(),
        }
        this.rememberResolution(resolution)
        this.emit(sessionId, 'permission_resolved', resolution)
        resolveApproval({ approved: resolution.approved, reason: resolution.reason })
        return resolution
      }
      const abort = () => settle(false, '操作已停止，工具未执行。')
      const timer = setTimeout(() => settle(false, '等待授权超时，工具未执行。'), this.timeoutMs)
      timer.unref?.()
      if (signal?.aborted) {
        abort()
        return
      }
      signal?.addEventListener('abort', abort, { once: true })
      this.pending.set(id, { ...publicApproval, settle })
      this.emit(sessionId, 'permission_request', publicApproval)
    })
  }

  resolve(sessionId, approvalId, approved) {
    const approval = this.pending.get(approvalId)
    if (approval?.sessionId === sessionId) {
      const resolution = approval.settle(
        Boolean(approved),
        approved ? '用户已授权执行。' : '用户拒绝执行该工具。',
      )
      return { found: true, alreadyResolved: false, ...resolution }
    }
    this.pruneResolutions()
    const previous = this.resolved.get(approvalId)
    if (previous?.sessionId === sessionId)
      return { found: true, alreadyResolved: true, ...previous }
    return { found: false, alreadyResolved: false, id: approvalId, sessionId }
  }

  resolveSession(sessionId, approved, reason = '') {
    let count = 0
    for (const approval of [...this.pending.values()]) {
      if (approval.sessionId !== sessionId) continue
      approval.settle(
        Boolean(approved),
        reason || (approved ? '权限模式已允许执行。' : '会话已停止。'),
      )
      count += 1
    }
    return count
  }

  getPending(sessionId) {
    return [...this.pending.values()]
      .filter((approval) => approval.sessionId === sessionId)
      .map(({ settle: _settle, ...approval }) => approval)
  }

  dispose() {
    for (const approval of [...this.pending.values()])
      approval.settle(false, '应用正在关闭，工具未执行。')
    this.emitters.clear()
    this.resolved.clear()
    this.remembered.clear()
  }
}
