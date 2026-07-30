import { existsSync, realpathSync } from 'node:fs'
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { randomUUID } from 'node:crypto'
import { TOOL_CATALOG } from '../tools/registry.mjs'

export const PERMISSION_MODES = new Set(['ask', 'auto', 'ignore'])
export const DEFAULT_PERMISSION_MODE = 'auto'
export const RESOLVED_APPROVAL_TTL_MS = 5 * 60_000
export const MAX_RESOLVED_APPROVALS = 256

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
const INTERNAL_SAFE_TOOLS = new Set(['get_goal', 'update_goal', 'get_task_list', 'update_task_list'])

function safeArgs(value, depth = 0, key = '') {
  if (depth > 3) return '[内容已省略]'
  if (/api.?key|password|passwd|secret|token/i.test(key)) return '[已隐藏敏感信息]'
  if (typeof value === 'string') {
    if (/^(?:data|image|content)$/i.test(key) && value.length > 500) return `[内容已省略，共 ${value.length} 字符]`
    return value.length > 800 ? `${value.slice(0, 800)}…` : value
  }
  if (Array.isArray(value)) return value.slice(0, 12).map((item) => safeArgs(item, depth + 1, key))
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).slice(0, 30).map(([childKey, child]) => [childKey, safeArgs(child, depth + 1, childKey)]))
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
  try { target = realpathSync.native(target) } catch {}
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
  if (['read', 'ls', 'grep', 'find', 'edit', 'write'].includes(toolName) && pathOutsideWorkspace(cwd, filePath)) {
    return {
      block: true,
      risk: 'high',
      reason: `${toolName} 不能在工作区模式下访问当前工作目录之外的文件。`,
    }
  }
  if (['read', 'ls', 'grep', 'find'].includes(toolName)) return null
  if (['edit', 'write'].includes(toolName)) {
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
  constructor({ getMode, getExecutionMode, getToolRisk, timeoutMs = 10 * 60_000 } = {}) {
    this.getMode = getMode || (() => DEFAULT_PERMISSION_MODE)
    this.getExecutionMode = getExecutionMode || (() => 'workspace')
    this.getToolRisk = getToolRisk || (() => null)
    this.timeoutMs = timeoutMs
    this.pending = new Map()
    this.resolved = new Map()
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
    while (this.resolved.size > MAX_RESOLVED_APPROVALS) this.resolved.delete(this.resolved.keys().next().value)
  }

  attachEmitter(sessionId, emit) {
    if (emit) this.emitters.set(sessionId, emit)
  }

  detachEmitter(sessionId, emit) {
    if (!emit || this.emitters.get(sessionId) === emit) this.emitters.delete(sessionId)
  }

  emit(sessionId, event, data) {
    try { this.emitters.get(sessionId)?.(event, data) } catch {}
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

  async authorize({ sessionId, cwd, toolName, toolCallId, args, signal }) {
    const mode = PERMISSION_MODES.has(this.getMode(sessionId)) ? this.getMode(sessionId) : DEFAULT_PERMISSION_MODE
    const executionMode = this.getExecutionMode(sessionId)
    const requirement = permissionRequirement({ mode, executionMode, cwd, toolName, args, toolRisk: this.getToolRisk(toolName) })
    if (!requirement) return undefined
    if (requirement.block) return { block: true, reason: requirement.reason }
    const approval = await this.requestApproval({ sessionId, toolName, toolCallId, args, mode, ...requirement, signal })
    if (approval.approved) return undefined
    return { block: true, reason: approval.reason || `用户拒绝执行工具 ${toolName}。` }
  }

  requestApproval({ sessionId, toolName, toolCallId, args, mode, risk, reason, signal }) {
    const id = randomUUID()
    const createdAt = new Date().toISOString()
    const publicApproval = { id, sessionId, toolName, toolCallId, args: safeArgs(args), mode, risk, reason, createdAt }
    return new Promise((resolveApproval) => {
      let settled = false
      const settle = (approved, resolutionReason) => {
        if (settled) return this.resolved.get(id)
        settled = true
        clearTimeout(timer)
        signal?.removeEventListener('abort', abort)
        this.pending.delete(id)
        const resolution = { id, sessionId, approved: Boolean(approved), reason: resolutionReason || '', resolvedAt: new Date().toISOString() }
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
      const resolution = approval.settle(Boolean(approved), approved ? '用户已授权执行。' : '用户拒绝执行该工具。')
      return { found: true, alreadyResolved: false, ...resolution }
    }
    this.pruneResolutions()
    const previous = this.resolved.get(approvalId)
    if (previous?.sessionId === sessionId) return { found: true, alreadyResolved: true, ...previous }
    return { found: false, alreadyResolved: false, id: approvalId, sessionId }
  }

  resolveSession(sessionId, approved, reason = '') {
    let count = 0
    for (const approval of [...this.pending.values()]) {
      if (approval.sessionId !== sessionId) continue
      approval.settle(Boolean(approved), reason || (approved ? '权限模式已允许执行。' : '会话已停止。'))
      count += 1
    }
    return count
  }

  getPending(sessionId) {
    return [...this.pending.values()].filter((approval) => approval.sessionId === sessionId).map(({ settle: _settle, ...approval }) => approval)
  }

  dispose() {
    for (const approval of [...this.pending.values()]) approval.settle(false, '应用正在关闭，工具未执行。')
    this.emitters.clear()
    this.resolved.clear()
  }
}
