// 工作流服务：有向无环图（DAG）工作流的定义、持久化、执行与审批。
// 节点类型包括 agent（调用会话）/命令/消息/条件，支持重试、人工审批节点、
// 通知（浏览器/飞书/微信）与运行记录。
import { randomUUID } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import {
  analyzeWorkflowGraph,
  createLinearWorkflowEdges,
  normalizeWorkflowEdges,
} from '../../shared/workflow-graph.mjs'

const STATE_VERSION = 2
const NODE_KINDS = new Set([
  'trigger',
  'prompt',
  'skill',
  'file',
  'mcp',
  'notification',
  'condition',
  'parallel',
  'approval',
])
const AGENT_KINDS = new Set(['prompt', 'skill', 'file', 'mcp'])
const NOTIFICATION_TARGETS = new Set(['browser', 'feishu', 'weixin'])
const FAILURE_POLICIES = new Set(['stop', 'skip'])
const OUTPUT_FORMATS = new Set(['text', 'json'])
const WORKFLOW_EXECUTION_MODES = new Set(['read-only', 'workspace-write', 'full-access'])
const CONDITION_OPERATORS = new Set([
  'exists',
  'not_exists',
  'equals',
  'not_equals',
  'contains',
  'greater_than',
  'less_than',
])

function defaultState() {
  return { version: STATE_VERSION, workflows: [], runs: [] }
}

function clone(value) {
  return JSON.parse(JSON.stringify(value))
}

function boundedString(value, max = 1200) {
  return String(value ?? '')
    .trim()
    .slice(0, max)
}

function normalizeModel(model) {
  return model?.provider && model?.model
    ? { provider: String(model.provider), model: String(model.model) }
    : null
}

function normalizeStringArray(value, max = 30) {
  return [
    ...new Set(
      (Array.isArray(value) ? value : [])
        .map((item) => boundedString(item, 120))
        .filter(Boolean)
        .slice(0, max),
    ),
  ]
}

function normalizeInputs(value) {
  return (Array.isArray(value) ? value : []).slice(0, 30).map((input, index) => ({
    id: boundedString(input?.id || `input-${index + 1}`, 80),
    name: boundedString(input?.name || `input_${index + 1}`, 80),
    label: boundedString(input?.label || input?.name || `Input ${index + 1}`, 120),
    type: ['string', 'number', 'boolean', 'text'].includes(input?.type) ? input.type : 'string',
    required: Boolean(input?.required),
    defaultValue: input?.defaultValue ?? '',
    description: boundedString(input?.description, 300),
  }))
}

function normalizeNode(node, index) {
  const kind = NODE_KINDS.has(node?.kind) ? node.kind : 'prompt'
  const condition = node?.condition || {}
  const approval = node?.approval || {}
  const notification = node?.notification || {}
  return {
    id: String(node?.id || randomUUID()),
    kind,
    label: boundedString(node?.label || `步骤 ${index + 1}`, 120),
    prompt: boundedString(node?.prompt, 100_000),
    x: Math.max(0, Math.min(4000, Number(node?.x) || 0)),
    y: Math.max(0, Math.min(4000, Number(node?.y) || 0)),
    model: normalizeModel(node?.model),
    executionMode: WORKFLOW_EXECUTION_MODES.has(node?.executionMode)
      ? node.executionMode
      : 'full-access',
    retries: Math.max(0, Math.min(3, Number(node?.retries) || 0)),
    timeoutMinutes: Math.max(1, Math.min(240, Number(node?.timeoutMinutes) || 20)),
    failurePolicy: FAILURE_POLICIES.has(node?.failurePolicy) ? node.failurePolicy : 'stop',
    enabled: node?.enabled !== false,
    outputFormat: OUTPUT_FORMATS.has(node?.outputFormat) ? node.outputFormat : 'text',
    skillName: boundedString(node?.skillName, 120),
    requestedToolNames: normalizeStringArray(node?.requestedToolNames, 20),
    condition: {
      source: boundedString(condition.source, 240),
      operator: CONDITION_OPERATORS.has(condition.operator) ? condition.operator : 'exists',
      value: condition.value ?? '',
    },
    approval: {
      message: boundedString(approval.message || node?.prompt, 1000),
      timeoutMinutes: Math.max(
        1,
        Math.min(10_080, Number(approval.timeoutMinutes) || Number(node?.timeoutMinutes) || 60),
      ),
    },
    notification: {
      title: boundedString(notification.title, 160),
      content: boundedString(notification.content, 12_000),
    },
    notificationTargets: normalizeStringArray(node?.notificationTargets).filter((target) =>
      NOTIFICATION_TARGETS.has(target),
    ),
  }
}

function normalizeStoredWorkflow(workflow, cwd) {
  const now = new Date().toISOString()
  const nodes = (Array.isArray(workflow?.nodes) ? workflow.nodes : [])
    .slice(0, 100)
    .map(normalizeNode)
  const sourceEdges = Array.isArray(workflow?.edges)
    ? workflow.edges
    : createLinearWorkflowEdges(nodes, () => randomUUID())
  return {
    id: String(workflow?.id || randomUUID()),
    name: boundedString(workflow?.name || '未命名工作流', 120),
    description: boundedString(workflow?.description, 600),
    status: workflow?.status === 'published' ? 'published' : 'draft',
    revision: Math.max(1, Number(workflow?.revision) || 1),
    cwd: String(workflow?.cwd || cwd),
    model: normalizeModel(workflow?.model),
    inputs: normalizeInputs(workflow?.inputs),
    tags: normalizeStringArray(workflow?.tags, 12),
    visibility: workflow?.visibility === 'shared' ? 'shared' : 'private',
    notifications: normalizeStringArray(workflow?.notifications).filter((target) =>
      NOTIFICATION_TARGETS.has(target),
    ),
    nodes,
    edges: normalizeWorkflowEdges(sourceEdges.slice(0, 300), nodes, () => randomUUID()),
    createdAt: workflow?.createdAt || now,
    updatedAt: workflow?.updatedAt || now,
    publishedAt: workflow?.publishedAt || null,
    lastRunAt: workflow?.lastRunAt || null,
    lastStatus: workflow?.lastStatus || 'idle',
    lastSummary: boundedString(workflow?.lastSummary, 1200),
    lastError: boundedString(workflow?.lastError, 1200),
  }
}

function durationLabel(durationMs) {
  const seconds = Math.max(0, Math.round(durationMs / 1000))
  if (seconds < 60) return `${seconds} 秒`
  return `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`
}

function serializeValue(value) {
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value ?? '')
  }
}

function nodeInstruction(workflow, node, previousOutputs, inputs) {
  const kindHints = {
    prompt: '完成这个 Agent 任务。',
    skill: node.skillName
      ? `必须使用 Skill「${node.skillName}」完成任务。`
      : '使用最合适的已启用 Skill 完成任务。',
    file: '使用可用的文件工具完成这个文件处理任务。',
    mcp: node.requestedToolNames.length
      ? `优先调用这些 MCP 工具：${node.requestedToolNames.join(', ')}。`
      : '优先使用已启用的 MCP 工具完成这个任务。',
  }
  const outputHint =
    node.outputFormat === 'json'
      ? '\n只输出有效 JSON，不要使用 Markdown 代码块。'
      : '\n完成后简洁总结结果，供后续节点继续使用。'
  const instruction = [
    `你正在执行工作流「${workflow.name}」的节点「${node.label}」。`,
    kindHints[node.kind] || '',
    node.prompt,
    Object.keys(inputs).length ? `\n工作流输入：\n${serializeValue(inputs)}` : '',
    previousOutputs.length
      ? `\n前序节点结果：\n${previousOutputs
          .map(
            (item) => `${item.label}：${serializeValue(item.output ?? item.summary ?? item.error)}`,
          )
          .join('\n')}`
      : '',
    outputHint,
  ]
    .filter(Boolean)
    .join('\n')
  return node.kind === 'skill' && node.skillName
    ? `/skill:${node.skillName}\n${instruction}`
    : instruction
}

function validateInputs(workflow, supplied = {}) {
  const values = {}
  for (const input of workflow.inputs) {
    const value = Object.hasOwn(supplied || {}, input.name)
      ? supplied[input.name]
      : input.defaultValue
    if (input.required && (value == null || value === ''))
      throw new Error(`工作流输入「${input.label}」不能为空。`)
    if (input.type === 'number' && value !== '' && !Number.isFinite(Number(value)))
      throw new Error(`工作流输入「${input.label}」必须是数字。`)
    values[input.name] =
      input.type === 'number' && value !== ''
        ? Number(value)
        : input.type === 'boolean'
          ? value === true || value === 'true'
          : value
  }
  for (const [key, value] of Object.entries(supplied || {})) {
    if (!Object.hasOwn(values, key)) values[key] = value
  }
  return values
}

function validateRunnable(workflow) {
  const graph = analyzeWorkflowGraph(workflow.nodes, workflow.edges)
  const executable = graph.nodes.filter((node) => AGENT_KINDS.has(node.kind))
  if (!executable.length && !graph.nodes.some((node) => node.kind === 'condition'))
    throw new Error('工作流至少需要一个可执行节点。')
  const invalid = executable.find(
    (node) => !node.prompt && !(node.kind === 'skill' && node.skillName),
  )
  if (invalid) throw new Error(`节点「${invalid.label}」还没有填写 Prompt。`)
  if (graph.nodes.length > 1 && !graph.edges.length) throw new Error('工作流节点尚未建立连接。')
  if (graph.invalidTriggerTargets.length)
    throw new Error(`触发器「${graph.invalidTriggerTargets[0].label}」不能连接上游节点。`)
  if (graph.unconnected.length)
    throw new Error(`节点「${graph.unconnected[0].label}」尚未连接到工作流。`)
  if (graph.hasCycle) throw new Error('工作流不能包含循环连接。')
  for (const node of graph.nodes.filter((item) => item.kind === 'condition')) {
    const ports = new Set((graph.outgoing.get(node.id) || []).map((edge) => edge.sourcePort))
    if (!ports.has('true') || !ports.has('false'))
      throw new Error(`判断节点「${node.label}」需要同时连接 true 和 false 分支。`)
  }
  return graph
}

function getPathValue(source, context) {
  const path = String(source || '').trim()
  if (!path) return context.previous?.output ?? context.previous?.summary
  const segments = path.split('.').filter(Boolean)
  let value
  if (segments[0] === 'inputs') value = context.inputs
  else if (segments[0] === 'nodes') value = context.nodes[segments[1]]?.output
  else if (segments[0] === 'previous') value = context.previous?.output
  else value = context.inputs
  const offset = ['inputs', 'previous'].includes(segments[0]) ? 1 : segments[0] === 'nodes' ? 2 : 0
  for (const segment of segments.slice(offset)) value = value?.[segment]
  return value
}

function renderWorkflowTemplate(template, context) {
  return String(template || '').replace(/\{\{\s*([\w.-]+)\s*\}\}/g, (_match, path) => {
    const segments = path.split('.').filter(Boolean)
    let value
    if (segments[0] === 'inputs') value = context.inputs
    else if (segments[0] === 'previous') value = context.previous
    else if (segments[0] === 'nodes') value = context.nodes[segments[1]]
    else if (segments[0] === 'workflow') value = context.workflow
    else if (segments[0] === 'run') value = context.run
    else return `{{${path}}}`
    const offset = segments[0] === 'nodes' ? 2 : 1
    for (const segment of segments.slice(offset)) value = value?.[segment]
    return value == null ? `{{${path}}}` : serializeValue(value)
  })
}

function evaluateCondition(condition, context) {
  const actual = getPathValue(condition.source, context)
  const expected = condition.value
  if (condition.operator === 'exists')
    return actual !== undefined && actual !== null && actual !== ''
  if (condition.operator === 'not_exists')
    return actual === undefined || actual === null || actual === ''
  if (condition.operator === 'equals') return String(actual) === String(expected)
  if (condition.operator === 'not_equals') return String(actual) !== String(expected)
  if (condition.operator === 'contains')
    return Array.isArray(actual)
      ? actual.some((item) => String(item) === String(expected))
      : String(actual ?? '').includes(String(expected))
  if (condition.operator === 'greater_than') return Number(actual) > Number(expected)
  if (condition.operator === 'less_than') return Number(actual) < Number(expected)
  return Boolean(actual)
}

function parseNodeOutput(text, format) {
  const value = boundedString(text || '节点已完成。', 100_000)
  if (format !== 'json') return value
  try {
    return JSON.parse(value)
  } catch {
    throw new Error('节点声明了 JSON 输出，但模型没有返回有效 JSON。')
  }
}

function normalizeStoredRun(run) {
  return {
    ...run,
    sourceSessionId: String(run?.sourceSessionId || ''),
    inputs: run?.inputs && typeof run.inputs === 'object' ? run.inputs : {},
    workflowRevision: Math.max(1, Number(run?.workflowRevision) || 1),
    retryOf: String(run?.retryOf || ''),
    nodes: (Array.isArray(run?.nodes) ? run.nodes : []).map((node) => ({
      ...node,
      output: node?.output ?? node?.summary ?? '',
      startedAt: node?.startedAt || null,
      finishedAt: node?.finishedAt || null,
      durationMs: Number(node?.durationMs) || 0,
      selectedPort: node?.selectedPort || '',
      approval: node?.approval || null,
      skipReason: node?.skipReason || '',
    })),
  }
}

export class WorkflowService {
  constructor({ path, cwd, agent, notifications, maxConcurrent = 4 }) {
    this.path = path
    this.cwd = cwd
    this.agent = agent
    this.notifications = notifications
    this.maxConcurrent = maxConcurrent
    this.state = defaultState()
    this.writeQueue = Promise.resolve()
    this.active = new Map()
  }

  async init() {
    // 加载工作流与运行记录；恢复运行中的工作流执行。
    const stored = await readJson(this.path, defaultState())
    this.state = {
      version: STATE_VERSION,
      workflows: (Array.isArray(stored.workflows) ? stored.workflows : []).map((workflow) => {
        const normalized = normalizeStoredWorkflow(workflow, this.cwd)
        if (normalized.lastStatus === 'running' || normalized.lastStatus === 'waiting_approval')
          normalized.lastStatus = 'interrupted'
        return normalized
      }),
      runs: (Array.isArray(stored.runs) ? stored.runs : []).slice(-200).map((storedRun) => {
        const run = normalizeStoredRun(storedRun)
        return ['running', 'waiting_approval'].includes(run.status)
          ? {
              ...run,
              status: 'interrupted',
              finishedAt: new Date().toISOString(),
              error: '应用重启，工作流运行已中断。',
            }
          : run
      }),
    }
    await this.save()
  }

  save() {
    const snapshot = clone(this.state)
    this.writeQueue = this.writeQueue
      .catch(() => {})
      .then(() => writeJsonAtomic(this.path, snapshot))
    return this.writeQueue
  }

  getState({ sessionId = '' } = {}) {
    const state = clone(this.state)
    if (sessionId) state.runs = state.runs.filter((run) => run.sourceSessionId === sessionId)
    return {
      ...state,
      limits: { maxConcurrent: this.maxConcurrent, running: this.active.size },
    }
  }

  getRun(id) {
    const run = this.state.runs.find((item) => item.id === id)
    return run ? clone(run) : null
  }

  // 规范化工作流输入：校验字段/节点/边，生成唯一 ID。
  async normalizeInput(input, current = {}) {
    const merged = { ...current, ...input }
    const name = String(merged.name || '').trim()
    if (!name) throw new Error('工作流名称不能为空。')
    if (Object.hasOwn(input || {}, 'cwd'))
      merged.cwd = await this.agent.validateDirectory(input.cwd)
    const workflow = normalizeStoredWorkflow(
      { ...merged, name, updatedAt: new Date().toISOString() },
      this.cwd,
    )
    if (workflow.status === 'published') {
      validateRunnable(workflow)
      workflow.publishedAt ||= new Date().toISOString()
    }
    return workflow
  }

  async create(input) {
    const workflow = await this.normalizeInput({
      ...input,
      id: randomUUID(),
      revision: 1,
      createdAt: new Date().toISOString(),
    })
    this.state.workflows.unshift(workflow)
    await this.save()
    return clone(workflow)
  }

  async update(id, input) {
    const index = this.state.workflows.findIndex((workflow) => workflow.id === id)
    if (index < 0) return null
    if ([...this.active.values()].some((record) => record.workflowId === id))
      throw new Error('工作流正在运行，暂时不能修改。')
    const current = this.state.workflows[index]
    const workflow = await this.normalizeInput(input, current)
    workflow.id = current.id
    workflow.createdAt = current.createdAt
    workflow.revision = current.revision + 1
    workflow.lastRunAt = current.lastRunAt
    workflow.lastStatus = current.lastStatus
    workflow.lastSummary = current.lastSummary
    workflow.lastError = current.lastError
    this.state.workflows[index] = workflow
    await this.save()
    return clone(workflow)
  }

  async duplicate(id, input = {}) {
    const current = this.state.workflows.find((workflow) => workflow.id === id)
    if (!current) return null
    const nodeIds = new Map(current.nodes.map((node) => [node.id, randomUUID()]))
    return this.create({
      ...clone(current),
      ...input,
      id: undefined,
      name: boundedString(input.name || `${current.name} 副本`, 120),
      status: 'draft',
      revision: 1,
      publishedAt: null,
      lastRunAt: null,
      lastStatus: 'idle',
      lastSummary: '',
      lastError: '',
      nodes: current.nodes.map((node) => ({ ...node, id: nodeIds.get(node.id) })),
      edges: current.edges.map((edge) => ({
        ...edge,
        id: randomUUID(),
        source: nodeIds.get(edge.source),
        target: nodeIds.get(edge.target),
      })),
    })
  }

  exportWorkflow(id) {
    const workflow = this.state.workflows.find((item) => item.id === id)
    if (!workflow) return null
    const exported = clone(workflow)
    delete exported.id
    delete exported.createdAt
    delete exported.updatedAt
    delete exported.publishedAt
    delete exported.lastRunAt
    delete exported.lastStatus
    delete exported.lastSummary
    delete exported.lastError
    return { format: 'pisper-workflow', version: 1, workflow: exported }
  }

  async importWorkflow(input) {
    if (input?.format !== 'pisper-workflow' || !input?.workflow)
      throw new Error('不是有效的 Pisper 工作流文件。')
    return this.create({ ...input.workflow, status: 'draft', visibility: 'private' })
  }

  async remove(id) {
    if ([...this.active.values()].some((record) => record.workflowId === id))
      throw new Error('工作流正在运行，暂时不能删除。')
    const before = this.state.workflows.length
    this.state.workflows = this.state.workflows.filter((workflow) => workflow.id !== id)
    this.state.runs = this.state.runs.filter((run) => run.workflowId !== id)
    if (this.state.workflows.length === before) return false
    await this.save()
    return true
  }

  // 立即运行工作流：校验发布状态、构建图并排队执行。
  async runNow(id, options = {}) {
    const workflow = this.state.workflows.find((item) => item.id === id)
    if (!workflow) return null
    if ([...this.active.values()].some((record) => record.workflowId === id))
      throw new Error('工作流已经在运行。')
    if (this.active.size >= this.maxConcurrent)
      throw new Error(`工作流并发已达到上限（${this.maxConcurrent}）。`)
    const graph = validateRunnable(workflow)
    const inputs = validateInputs(workflow, options.inputs)
    const run = {
      id: randomUUID(),
      workflowId: workflow.id,
      workflowName: workflow.name,
      workflowRevision: workflow.revision,
      trigger: options.trigger || 'manual',
      sourceSessionId: String(options.sourceSessionId || ''),
      sourceMessage: boundedString(options.sourceMessage, 12_000),
      retryOf: String(options.retryOf || ''),
      inputs,
      status: 'running',
      startedAt: new Date().toISOString(),
      finishedAt: null,
      durationMs: 0,
      completedNodes: 0,
      totalNodes: graph.nodes.length,
      currentNodeId: '',
      currentNodeLabel: '',
      summary: '',
      error: '',
      sessionId: '',
      assets: [],
      nodes: graph.order.map((node) => ({
        id: node.id,
        label: node.label,
        kind: node.kind,
        status: 'pending',
        attempts: 0,
        summary: '',
        output: '',
        error: '',
        sessionId: '',
        startedAt: null,
        finishedAt: null,
        durationMs: 0,
        selectedPort: '',
        approval: null,
        skipReason: '',
      })),
    }
    this.state.runs.push(run)
    this.state.runs = this.state.runs.slice(-200)
    workflow.lastRunAt = run.startedAt
    workflow.lastStatus = 'running'
    workflow.lastError = ''
    const record = {
      runId: run.id,
      workflowId: workflow.id,
      cancelled: false,
      sessionIds: new Set(),
      approvals: new Map(),
    }
    this.active.set(run.id, record)
    await this.save()
    void this.execute(workflow, run, graph, record)
    return clone(run)
  }

  async retryRun(runId) {
    const run = this.state.runs.find((item) => item.id === runId)
    if (!run || !['failed', 'cancelled', 'interrupted'].includes(run.status)) return null
    return this.runNow(run.workflowId, {
      trigger: 'retry',
      inputs: run.inputs,
      sourceSessionId: run.sourceSessionId,
      sourceMessage: run.sourceMessage,
      retryOf: run.id,
    })
  }

  async resolveApproval(runId, nodeId, approved, comment = '') {
    const record = this.active.get(runId)
    const pending = record?.approvals.get(nodeId)
    if (!pending) return null
    record.approvals.delete(nodeId)
    pending.resolve({ approved: Boolean(approved), comment: boundedString(comment, 1000) })
    return this.getRun(runId)
  }

  async stop(runId) {
    const record = this.active.get(runId)
    if (!record) return null
    record.cancelled = true
    for (const pending of record.approvals.values()) pending.reject(cancelledError())
    record.approvals.clear()
    await Promise.all(
      [...record.sessionIds].map((sessionId) => this.agent.abort(sessionId).catch(() => {})),
    )
    return this.getRun(runId)
  }

  // 执行工作流：按拓扑序遍历图节点，处理前置依赖与并发。
  async execute(workflow, run, graph, record) {
    const started = Date.now()
    const nodeRuns = new Map(run.nodes.map((item) => [item.id, item]))
    const executions = new Map()
    let failedNode = null

    const edgeActive = (edge) => {
      const source = nodeRuns.get(edge.source)
      if (!source || ['failed', 'cancelled'].includes(source.status)) return false
      if (source.status === 'skipped' && source.skipReason === 'branch_not_selected') return false
      if (source.kind === 'condition') return source.selectedPort === edge.sourcePort
      return true
    }

    const executeNode = (node) => {
      if (executions.has(node.id)) return executions.get(node.id)
      const promise = (async () => {
        const incoming = graph.incoming.get(node.id) || []
        await Promise.all(
          incoming.map((edge) => executeNode(graph.nodes.find((item) => item.id === edge.source))),
        )
        if (incoming.length && !incoming.some(edgeActive)) {
          const skipped = nodeRuns.get(node.id)
          skipped.status = 'skipped'
          skipped.skipReason = 'branch_not_selected'
          skipped.summary = '上游条件分支未命中。'
          skipped.output = skipped.summary
          skipped.finishedAt = new Date().toISOString()
          run.completedNodes += 1
          await this.save()
          return skipped
        }
        return this.executeNode(workflow, run, graph, record, node, nodeRuns)
      })()
      executions.set(node.id, promise)
      return promise
    }

    try {
      const results = await Promise.allSettled(graph.order.map(executeNode))
      const rejected = results.find((result) => result.status === 'rejected')
      if (rejected) throw rejected.reason
      run.status = 'completed'
      const terminalNodes = graph.order.filter(
        (node) => !(graph.outgoing.get(node.id) || []).length,
      )
      run.summary =
        terminalNodes
          .map((node) => nodeRuns.get(node.id))
          .filter((node) => node && node.status !== 'skipped')
          .map((node) => node.summary)
          .filter(Boolean)
          .join('\n') || '工作流已完成。'
      workflow.lastStatus = 'completed'
      workflow.lastSummary = run.summary
      workflow.lastError = ''
    } catch (error) {
      const cancelled = record.cancelled || error?.code === 'WORKFLOW_CANCELLED'
      const message = error instanceof Error ? error.message : String(error)
      run.status = cancelled ? 'cancelled' : 'failed'
      run.error = message
      failedNode = run.nodes.find((node) => node.status === 'failed') || null
      for (const nodeRun of run.nodes.filter((item) => item.status === 'pending')) {
        nodeRun.status = 'skipped'
        nodeRun.summary = cancelled ? '工作流已停止。' : '上游节点失败，未执行。'
        nodeRun.output = nodeRun.summary
        nodeRun.finishedAt = new Date().toISOString()
      }
      workflow.lastStatus = run.status
      workflow.lastError = message
    } finally {
      run.currentNodeId = ''
      run.currentNodeLabel = ''
      run.finishedAt = new Date().toISOString()
      run.durationMs = Date.now() - started
      workflow.updatedAt = new Date().toISOString()
      this.active.delete(run.id)
      await this.save()
    }

    if (workflow.notifications.length && run.status === 'completed') {
      await this.notifyRun('workflow.completed', workflow, run).catch(() => {})
    } else if (workflow.notifications.length && run.status === 'failed') {
      await this.notifyRun('workflow.failed', workflow, run, failedNode).catch(() => {})
    }
  }

  // 执行单个节点：agent 节点调会话，命令节点调 shell，消息节点转发通知。
  async executeNode(workflow, run, graph, record, node, nodeRuns) {
    if (record.cancelled) throw cancelledError()
    const nodeRun = nodeRuns.get(node.id)
    const predecessors = (graph.incoming.get(node.id) || [])
      .map((edge) => nodeRuns.get(edge.source))
      .filter((item) => item && !['skipped', 'failed', 'cancelled'].includes(item.status))
    nodeRun.status = 'running'
    nodeRun.startedAt = new Date().toISOString()
    run.currentNodeId = node.id
    run.currentNodeLabel = node.label
    await this.save()

    try {
      if (node.kind === 'trigger') {
        nodeRun.output = run.inputs
        nodeRun.summary = Object.keys(run.inputs).length
          ? serializeValue(run.inputs)
          : '工作流已触发。'
      } else if (node.kind === 'condition') {
        const nodes = Object.fromEntries([...nodeRuns].map(([id, value]) => [id, value]))
        const matched = evaluateCondition(node.condition, {
          inputs: run.inputs,
          nodes,
          previous: predecessors[0],
        })
        nodeRun.output = matched
        nodeRun.selectedPort = matched ? 'true' : 'false'
        nodeRun.summary = matched ? '条件成立，进入 true 分支。' : '条件不成立，进入 false 分支。'
      } else if (node.kind === 'parallel') {
        nodeRun.output = predecessors.map((item) => item.output)
        nodeRun.summary = predecessors.length ? '并行分支已汇合。' : '并行分支已启动。'
      } else if (node.kind === 'approval') {
        const decision = await this.waitForApproval(run, node, nodeRun, record)
        if (!decision.approved)
          throw new Error(decision.comment || `审批节点「${node.label}」已拒绝。`)
        nodeRun.output = { approved: true, comment: decision.comment }
        nodeRun.summary = decision.comment || '审批已通过。'
      } else if (node.kind === 'notification') {
        nodeRun.output = predecessors.map((item) => item.output)
        const fallbackContent =
          predecessors
            .map((item) => item.summary)
            .filter(Boolean)
            .join('\n') || '通知已发送。'
        const templateContext = {
          inputs: run.inputs,
          previous: predecessors.at(-1) || null,
          nodes: Object.fromEntries([...nodeRuns]),
          workflow: { id: workflow.id, name: workflow.name, description: workflow.description },
          run: { id: run.id, startedAt: run.startedAt },
        }
        const content =
          renderWorkflowTemplate(node.notification.content, templateContext).trim() ||
          fallbackContent
        const title = renderWorkflowTemplate(node.notification.title, templateContext).trim()
        nodeRun.summary = boundedString(content, 1200)
        if (node.notificationTargets.length) {
          await this.notifications.notify(
            'workflow.completed',
            {
              workflow: {
                name: workflow.name,
                summary: content,
                duration: durationLabel(Date.now() - Date.parse(run.startedAt)),
                runId: run.id,
              },
            },
            { platforms: node.notificationTargets, title: title || undefined, content },
          )
        }
      } else if (AGENT_KINDS.has(node.kind)) {
        await this.executeAgentNode(workflow, run, record, node, nodeRun, predecessors)
      } else {
        nodeRun.output = predecessors.map((item) => item.output)
        nodeRun.summary = '控制节点已通过。'
      }
      nodeRun.status = 'completed'
    } catch (error) {
      if (record.cancelled || error?.code === 'WORKFLOW_CANCELLED') {
        nodeRun.status = 'cancelled'
        nodeRun.error = error.message
        throw error
      }
      const message = error instanceof Error ? error.message : String(error)
      nodeRun.error = message
      if (node.failurePolicy === 'skip') {
        nodeRun.status = 'skipped'
        nodeRun.skipReason = 'failure_policy'
        nodeRun.summary = `已跳过：${message}`
        nodeRun.output = nodeRun.summary
      } else {
        nodeRun.status = 'failed'
        throw error
      }
    } finally {
      nodeRun.finishedAt = new Date().toISOString()
      nodeRun.durationMs = Date.parse(nodeRun.finishedAt) - Date.parse(nodeRun.startedAt)
      run.completedNodes += 1
      await this.save()
    }
    return nodeRun
  }

  async executeAgentNode(workflow, run, record, node, nodeRun, predecessors) {
    let lastError = null
    for (let attempt = 0; attempt <= node.retries; attempt += 1) {
      nodeRun.attempts = attempt + 1
      try {
        let timeoutTimer
        const predecessor = predecessors.length === 1 ? predecessors[0] : null
        const predecessorBranches = predecessor
          ? (run.nodes.filter((item) => item.id === predecessor.id).length,
            workflow.edges.filter((edge) => edge.source === predecessor.id).length)
          : 0
        const inheritedSessionId =
          predecessor && predecessorBranches === 1 ? predecessor.sessionId : ''
        let activeSessionId = inheritedSessionId
        const prompt = this.agent.prompt({
          sessionId: inheritedSessionId,
          message: nodeInstruction(workflow, node, predecessors, run.inputs),
          cwd: workflow.cwd,
          title: `工作流 · ${workflow.name}`,
          model: node.model || workflow.model,
          executionMode: node.executionMode,
          isolatedContext: true,
          requestedToolNames: node.requestedToolNames,
          onSession: (sessionId) => {
            activeSessionId = sessionId
            record.sessionIds.add(sessionId)
            nodeRun.sessionId = sessionId
            run.sessionId = sessionId
          },
        })
        const timeout = new Promise((_resolve, reject) => {
          timeoutTimer = setTimeout(async () => {
            if (activeSessionId) await this.agent.abort(activeSessionId).catch(() => {})
            reject(
              Object.assign(
                new Error(`节点「${node.label}」执行超过 ${node.timeoutMinutes} 分钟。`),
                {
                  code: 'WORKFLOW_TIMEOUT',
                },
              ),
            )
          }, node.timeoutMinutes * 60_000)
          timeoutTimer.unref?.()
        })
        let result
        try {
          result = await Promise.race([prompt, timeout])
        } finally {
          clearTimeout(timeoutTimer)
        }
        if (record.cancelled) throw cancelledError()
        run.sessionId = result.sessionId || run.sessionId
        nodeRun.sessionId = result.sessionId || nodeRun.sessionId
        if (nodeRun.sessionId) record.sessionIds.add(nodeRun.sessionId)
        nodeRun.output = parseNodeOutput(result.text, node.outputFormat)
        nodeRun.summary = boundedString(
          typeof nodeRun.output === 'string' ? nodeRun.output : serializeValue(nodeRun.output),
          1200,
        )
        run.assets.push(
          ...(result.assets || []).filter(
            (asset) => !run.assets.some((item) => item.id === asset.id),
          ),
        )
        lastError = null
        break
      } catch (error) {
        lastError = error
        if (record.cancelled || error?.code === 'WORKFLOW_CANCELLED') throw error
      }
    }
    if (lastError) throw lastError
  }

  // 人工审批节点：挂起直到用户在 API 层批准/驳回。
  waitForApproval(run, node, nodeRun, record) {
    run.status = 'waiting_approval'
    nodeRun.status = 'waiting_approval'
    nodeRun.approval = {
      message: node.approval.message || `是否允许工作流继续执行「${node.label}」？`,
      requestedAt: new Date().toISOString(),
      expiresAt: new Date(Date.now() + node.approval.timeoutMinutes * 60_000).toISOString(),
    }
    void this.save()
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        record.approvals.delete(node.id)
        reject(new Error(`审批节点「${node.label}」等待超时。`))
      }, node.approval.timeoutMinutes * 60_000)
      timer.unref?.()
      record.approvals.set(node.id, {
        resolve: (decision) => {
          clearTimeout(timer)
          run.status = 'running'
          nodeRun.approval = {
            ...nodeRun.approval,
            ...decision,
            resolvedAt: new Date().toISOString(),
          }
          resolve(decision)
        },
        reject: (error) => {
          clearTimeout(timer)
          reject(error)
        },
      })
    })
  }

  // 运行事件通知：向已启用的通知渠道广播。
  notifyRun(event, workflow, run, failedNode = null) {
    const data =
      event === 'workflow.completed'
        ? {
            workflow: {
              name: workflow.name,
              summary: run.summary,
              duration: durationLabel(run.durationMs),
              runId: run.id,
            },
          }
        : {
            workflow: {
              name: workflow.name,
              node: failedNode?.label || '未知节点',
              error: run.error,
              runId: run.id,
            },
          }
    return this.notifications.notify(event, data, { platforms: workflow.notifications })
  }

  async dispose() {
    for (const record of this.active.values()) {
      record.cancelled = true
      for (const pending of record.approvals.values()) pending.reject(cancelledError())
      await Promise.all(
        [...record.sessionIds].map((sessionId) => this.agent.abort(sessionId).catch(() => {})),
      )
    }
    await this.writeQueue.catch(() => {})
  }
}

function cancelledError() {
  return Object.assign(new Error('工作流已停止。'), { code: 'WORKFLOW_CANCELLED' })
}
