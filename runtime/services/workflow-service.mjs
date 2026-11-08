// 工作流服务：有向无环图（DAG）工作流的定义、持久化、执行与审批。
// 节点类型包括 agent（调用会话）/命令/消息/条件，支持重试、人工审批节点、
// 通知（浏览器/飞书/微信/QQ/Telegram）与运行记录。
import { createHash, randomUUID } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { pisperError } from '../../shared/errors.mjs'
import {
  WORKFLOW_NODE_SUMMARY,
  workflowApprovalQuestion,
  workflowNodeInstruction,
} from '../prompts/workflows.mjs'
import {
  analyzeWorkflowGraph,
  createLinearWorkflowEdges,
  normalizeWorkflowEdges,
} from '../../shared/workflow-graph.mjs'
import {
  renderWorkflowTemplate,
  validateWorkflowInputDefinitions,
  validateWorkflowInputs,
  validateWorkflowTemplate,
  WorkflowInputError,
} from '../../shared/workflow-inputs.mjs'
import {
  WORKFLOW_IMAGE_NODE_KINDS,
  isWorkflowImageNodeKind,
  normalizeWorkflowImageSettings,
  parseWorkflowImageOutput,
  workflowImageError,
} from '../../shared/workflow-image-nodes.mjs'

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
  ...WORKFLOW_IMAGE_NODE_KINDS,
])
const AGENT_KINDS = new Set(['prompt', 'skill', 'file', 'mcp'])
const NOTIFICATION_TARGETS = new Set(['browser', 'feishu', 'weixin', 'qq', 'telegram'])
const FAILURE_POLICIES = new Set(['stop', 'skip'])
const OUTPUT_FORMATS = new Set(['text', 'json'])
const WORKFLOW_EXECUTION_MODES = new Set(['workspace-write', 'full-access'])
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

function nodeConfigurationHash(node) {
  const { x: _x, y: _y, label: _label, ...configuration } = node
  return createHash('sha256').update(JSON.stringify(configuration)).digest('hex')
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
  // 存量定义仍按原格式加载；新写入和每次运行在共享边界执行严格校验。
  return (Array.isArray(value) ? value : []).slice(0, 30).map((input, index) => ({
    id: boundedString(input?.id || `input-${index + 1}`, 80),
    name: boundedString(input?.name || `input_${index + 1}`, 80),
    label: boundedString(input?.label || input?.name || `Input ${index + 1}`, 120),
    type: ['string', 'number', 'boolean', 'text', 'image', 'video'].includes(input?.type)
      ? input.type
      : 'string',
    required: Boolean(input?.required),
    defaultValue: input?.defaultValue ?? (['image', 'video'].includes(input?.type) ? null : ''),
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
    label: boundedString(node?.label || `Step ${index + 1}`, 120),
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
    ...(isWorkflowImageNodeKind(kind)
      ? { image: normalizeWorkflowImageSettings(node?.image) }
      : {}),
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
    name: boundedString(workflow?.name || 'Untitled workflow', 120),
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

// 通知模板里的耗时：用不分语言的 45s / 3m 12s，聊天软件里的中文模板与英文都看得懂。
function durationLabel(durationMs) {
  const seconds = Math.max(0, Math.round(durationMs / 1000))
  if (seconds < 60) return `${seconds}s`
  return `${Math.floor(seconds / 60)}m ${seconds % 60}s`
}

function serializeValue(value) {
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value ?? '')
  }
}

function workflowTemplateContext(workflow, run, previousOutputs) {
  return {
    inputs: run.inputs,
    previous: previousOutputs.at(-1) || null,
    nodes: Object.fromEntries(
      run.nodes.filter((item) => item.status === 'completed').map((item) => [item.id, item]),
    ),
    workflow: { id: workflow.id, name: workflow.name, description: workflow.description },
    run: { id: run.id, startedAt: run.startedAt },
  }
}

function nodeInstruction(workflow, node, previousOutputs, run, mediaContext = '') {
  const inputs = run.inputs
  const context = workflowTemplateContext(workflow, run, previousOutputs)
  const instruction = workflowNodeInstruction({
    workflowName: workflow.name,
    node,
    prompt: renderWorkflowTemplate(node.prompt, context),
    inputs: Object.keys(inputs).length ? serializeValue(inputs) : '',
    mediaContext,
    previous: previousOutputs.map((item) => ({
      label: item.label,
      text: serializeValue(item.output ?? item.summary ?? item.error),
    })),
  })
  return node.kind === 'skill' && node.skillName
    ? `/skill:${node.skillName}\n${instruction}`
    : instruction
}

function validateRunnable(workflow) {
  const graph = analyzeWorkflowGraph(workflow.nodes, workflow.edges)
  const executable = graph.nodes.filter((node) => AGENT_KINDS.has(node.kind))
  if (
    !executable.length &&
    !graph.nodes.some((node) => node.kind === 'condition' || isWorkflowImageNodeKind(node.kind))
  )
    throw pisperError('invalid', 'workflow', 'add at least one executable node')
  const invalid = executable.find(
    (node) => !node.prompt && !(node.kind === 'skill' && node.skillName),
  )
  if (invalid) throw pisperError('invalid', 'workflow', `node "${invalid.label}" has no prompt`)
  if (graph.nodes.length > 1 && !graph.edges.length)
    throw pisperError('invalid', 'workflow', 'nodes are not connected')
  if (graph.invalidTriggerTargets.length)
    throw pisperError(
      'invalid',
      'workflow',
      `trigger "${graph.invalidTriggerTargets[0].label}" cannot have upstream nodes`,
    )
  if (graph.unconnected.length)
    throw pisperError(
      'invalid',
      'workflow',
      `node "${graph.unconnected[0].label}" is not connected`,
    )
  if (graph.hasCycle) throw pisperError('invalid', 'workflow', 'connections form a cycle')
  for (const node of graph.nodes.filter((item) => item.kind === 'condition')) {
    const ports = new Set((graph.outgoing.get(node.id) || []).map((edge) => edge.sourcePort))
    if (!ports.has('true') || !ports.has('false'))
      throw pisperError(
        'invalid',
        'workflow',
        `condition "${node.label}" needs both true and false branches`,
      )
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
  for (const segment of segments.slice(offset)) {
    if (
      ['__proto__', 'constructor', 'prototype'].includes(segment) ||
      !value ||
      typeof value !== 'object' ||
      !Object.hasOwn(value, segment)
    )
      return undefined
    value = value[segment]
  }
  return value
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
  const value = boundedString(text || WORKFLOW_NODE_SUMMARY.completed, 100_000)
  if (format !== 'json') return value
  try {
    return JSON.parse(value)
  } catch {
    throw pisperError('invalid', 'workflow', 'node expects JSON output but got invalid JSON')
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
  constructor({
    path,
    cwd,
    agent,
    notifications,
    maxConcurrent = 4,
    resolveMediaInputs,
    executeImageNode,
  }) {
    this.path = path
    this.cwd = cwd
    this.agent = agent
    this.notifications = notifications
    this.maxConcurrent = maxConcurrent
    this.executeImageNode = executeImageNode
    this.resolveMediaInputs =
      resolveMediaInputs ||
      (async (inputs) => {
        if (Object.values(inputs).some((value) => value && typeof value === 'object'))
          throw new WorkflowInputError('workflow_media_missing')
        return { attachments: [], context: '' }
      })
    this.state = defaultState()
    this.writeQueue = Promise.resolve()
    this.active = new Map()
    this.starting = new Map()
    this.creating = new Set()
    this.closed = false
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
              error: WORKFLOW_NODE_SUMMARY.interruptedByRestart,
            }
          : run
      }),
    }
    await this.save()
  }

  save(createdWorkflowId = '') {
    this.writeQueue = this.writeQueue
      .catch(() => {})
      // 等前一笔提交及其回滚完成后再取权威状态，避免排队快照重新写入失败的新实体。
      .then(() => {
        const snapshot = clone(this.state)
        // 本次提交只带上自己的新实体，不能替并发创建提前落盘。
        snapshot.workflows = snapshot.workflows.filter(
          ({ id }) => !this.creating.has(id) || id === createdWorkflowId,
        )
        return writeJsonAtomic(this.path, snapshot)
      })
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
    merged.inputs = validateWorkflowInputDefinitions(merged.inputs)
    const name = String(merged.name || '').trim()
    if (!name) throw pisperError('invalid', 'workflow', 'name is required')
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
    this.creating.add(workflow.id)
    this.state.workflows.unshift(workflow)
    try {
      await this.save(workflow.id)
    } catch (error) {
      // 只撤销本次新建，保留提交期间其他操作产生的状态。
      this.state.workflows = this.state.workflows.filter((item) => item.id !== workflow.id)
      throw error
    } finally {
      this.creating.delete(workflow.id)
    }
    return clone(workflow)
  }

  async update(id, input) {
    const index = this.state.workflows.findIndex((workflow) => workflow.id === id)
    if (index < 0) return null
    if (
      this.starting.has(id) ||
      [...this.active.values()].some((record) => record.workflowId === id)
    )
      throw pisperError('busy', 'workflow')
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
      name: boundedString(input.name || `${current.name} copy`, 120),
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
      throw pisperError('invalid', 'file', 'not a Pisper workflow file')
    return this.create({ ...input.workflow, status: 'draft', visibility: 'private' })
  }

  async remove(id) {
    if (
      this.starting.has(id) ||
      [...this.active.values()].some((record) => record.workflowId === id)
    )
      throw pisperError('busy', 'workflow')
    const before = this.state.workflows.length
    this.state.workflows = this.state.workflows.filter((workflow) => workflow.id !== id)
    this.state.runs = this.state.runs.filter((run) => run.workflowId !== id)
    if (this.state.workflows.length === before) return false
    await this.save()
    return true
  }

  // 立即运行工作流：校验发布状态、构建图并排队执行。
  runNow(id, options = {}) {
    if (this.closed) return Promise.reject(cancelledError())
    if (this.starting.has(id)) return Promise.reject(pisperError('busy', 'workflow'))
    if (this.active.size + this.starting.size >= this.maxConcurrent)
      return Promise.reject(
        pisperError('limit', 'workflow_run', `max ${this.maxConcurrent} at once`),
      )
    const pending = this.startRun(id, options)
    this.starting.set(id, pending)
    return pending.finally(() => this.starting.delete(id))
  }

  async startRun(id, options = {}) {
    const workflow = this.state.workflows.find((item) => item.id === id)
    if (!workflow) return null
    if ([...this.active.values()].some((record) => record.workflowId === id))
      throw pisperError('busy', 'workflow')
    if (this.active.size >= this.maxConcurrent)
      throw pisperError('limit', 'workflow_run', `max ${this.maxConcurrent} at once`)
    const graph = validateRunnable(workflow)
    const onlyNode = options.nodeId ? graph.nodes.find((node) => node.id === options.nodeId) : null
    const sourceRun = options.nodeId
      ? this.state.runs.find((run) => run.id === options.sourceRunId && run.workflowId === id)
      : null
    let reusedNodes = []
    let imagePredecessors
    let resumeOutput
    if (options.nodeId) {
      if (
        !onlyNode ||
        !isWorkflowImageNodeKind(onlyNode.kind) ||
        !sourceRun ||
        ['running', 'waiting_approval'].includes(sourceRun.status)
      )
        throw workflowImageError('workflow_image_source_stale')
      const ancestors = new Set()
      const collect = (nodeId) => {
        for (const edge of graph.incoming.get(nodeId) || []) {
          if (!ancestors.has(edge.source)) {
            ancestors.add(edge.source)
            collect(edge.source)
          }
        }
      }
      collect(onlyNode.id)
      for (const ancestorId of ancestors) {
        const current = graph.nodes.find((node) => node.id === ancestorId)
        const cached = sourceRun.nodes.find((node) => node.id === ancestorId)
        if (
          !current ||
          cached?.status !== 'completed' ||
          cached.configurationHash !== nodeConfigurationHash(current)
        )
          throw workflowImageError('workflow_image_source_stale')
      }
      const descendants = new Set()
      const invalidate = (nodeId) => {
        for (const edge of graph.outgoing.get(nodeId) || []) {
          if (!descendants.has(edge.target)) {
            descendants.add(edge.target)
            invalidate(edge.target)
          }
        }
      }
      invalidate(onlyNode.id)
      const previousNode = sourceRun.nodes.find((node) => node.id === onlyNode.id)
      if (
        onlyNode.kind === 'media-generate' &&
        previousNode?.status === 'failed' &&
        previousNode.configurationHash === nodeConfigurationHash(onlyNode) &&
        previousNode.output &&
        typeof previousNode.output === 'object'
      )
        resumeOutput = parseWorkflowImageOutput(previousNode.output)
      reusedNodes = sourceRun.nodes
        .filter(
          (node) =>
            node.id !== onlyNode.id &&
            !descendants.has(node.id) &&
            node.status === 'completed' &&
            graph.nodes.some((current) => current.id === node.id),
        )
        .map((node) => ({ ...clone(node), reused: true }))
      imagePredecessors = {
        [onlyNode.id]: (graph.incoming.get(onlyNode.id) || []).map((edge) =>
          reusedNodes.find((node) => node.id === edge.source),
        ),
      }
    }
    const inputs = validateWorkflowInputs(workflow.inputs, options.inputs ?? sourceRun?.inputs)
    const inputNames = workflow.inputs.length
      ? workflow.inputs.map((input) => input.name)
      : ['task']
    for (const node of graph.nodes) {
      for (const template of [
        node.prompt,
        node.approval.message,
        node.notification.title,
        node.notification.content,
      ])
        validateWorkflowTemplate(
          template,
          inputNames,
          graph.nodes.map((item) => item.id),
        )
    }
    await this.resolveMediaInputs(inputs)
    if (this.closed) throw cancelledError()
    const run = {
      id: randomUUID(),
      workflowId: workflow.id,
      workflowName: workflow.name,
      workflowRevision: workflow.revision,
      trigger: onlyNode ? 'node' : options.trigger || 'manual',
      ...(onlyNode ? { sourceRunId: sourceRun.id, nodeId: onlyNode.id } : {}),
      sourceSessionId: String(options.sourceSessionId || ''),
      sourceMessage: boundedString(options.sourceMessage, 12_000),
      retryOf: String(options.retryOf || ''),
      inputs,
      status: 'running',
      startedAt: new Date().toISOString(),
      finishedAt: null,
      durationMs: 0,
      completedNodes: reusedNodes.length,
      totalNodes: onlyNode ? reusedNodes.length + 1 : graph.nodes.length,
      currentNodeId: '',
      currentNodeLabel: '',
      summary: '',
      error: '',
      sessionId: '',
      assets: [],
      nodes: [
        ...reusedNodes,
        ...(onlyNode ? [onlyNode] : graph.order).map((node) => ({
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
          configurationHash: nodeConfigurationHash(node),
        })),
      ],
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
      controller: new AbortController(),
      done: null,
      imagePredecessors,
      resumeOutput,
    }
    this.active.set(run.id, record)
    await this.save()
    const executionGraph = onlyNode
      ? {
          ...graph,
          nodes: [onlyNode],
          order: [onlyNode],
          incoming: new Map([[onlyNode.id, []]]),
          outgoing: new Map([[onlyNode.id, []]]),
        }
      : graph
    record.done = this.execute(workflow, run, executionGraph, record)
    void record.done.catch(() => {})
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
    record.controller.abort(cancelledError())
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
          skipped.summary = WORKFLOW_NODE_SUMMARY.branchNotSelected
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
          .join('\n') || WORKFLOW_NODE_SUMMARY.workflowCompleted
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
        nodeRun.summary = cancelled
          ? WORKFLOW_NODE_SUMMARY.workflowStopped
          : WORKFLOW_NODE_SUMMARY.upstreamFailed
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
    const predecessors =
      record.imagePredecessors?.[node.id] ??
      (graph.incoming.get(node.id) || [])
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
          : WORKFLOW_NODE_SUMMARY.triggered
      } else if (node.kind === 'condition') {
        const nodes = Object.fromEntries([...nodeRuns].map(([id, value]) => [id, value]))
        const matched = evaluateCondition(node.condition, {
          inputs: run.inputs,
          nodes,
          previous: predecessors[0],
        })
        nodeRun.output = matched
        nodeRun.selectedPort = matched ? 'true' : 'false'
        nodeRun.summary = matched
          ? WORKFLOW_NODE_SUMMARY.conditionTrue
          : WORKFLOW_NODE_SUMMARY.conditionFalse
      } else if (node.kind === 'parallel') {
        nodeRun.output = predecessors.map((item) => item.output)
        nodeRun.summary = predecessors.length
          ? WORKFLOW_NODE_SUMMARY.parallelJoined
          : WORKFLOW_NODE_SUMMARY.parallelStarted
      } else if (node.kind === 'approval') {
        const decision = await this.waitForApproval(
          run,
          {
            ...node,
            approval: {
              ...node.approval,
              message: renderWorkflowTemplate(
                node.approval.message,
                workflowTemplateContext(workflow, run, predecessors),
              ),
            },
          },
          nodeRun,
          record,
        )
        if (!decision.approved)
          throw decision.comment
            ? new Error(decision.comment)
            : pisperError('forbidden', 'workflow', `approval "${node.label}" was rejected`)
        nodeRun.output = { approved: true, comment: decision.comment }
        nodeRun.summary = decision.comment || WORKFLOW_NODE_SUMMARY.approved
      } else if (node.kind === 'notification') {
        nodeRun.output = predecessors.map((item) => item.output)
        const fallbackContent =
          predecessors
            .map((item) => item.summary)
            .filter(Boolean)
            .join('\n') || WORKFLOW_NODE_SUMMARY.notificationSent
        const templateContext = workflowTemplateContext(workflow, run, predecessors)
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
      } else if (isWorkflowImageNodeKind(node.kind)) {
        if (!this.executeImageNode) throw workflowImageError('workflow_image_unavailable')
        const controller = new AbortController()
        const abort = () => controller.abort(record.controller.signal.reason)
        record.controller.signal.addEventListener('abort', abort, { once: true })
        if (record.cancelled) abort()
        const timer = setTimeout(
          () =>
            controller.abort(
              Object.assign(new Error('workflow_image_timeout'), { code: 'WORKFLOW_TIMEOUT' }),
            ),
          node.timeoutMinutes * 60000,
        )
        timer.unref?.()
        try {
          nodeRun.attempts = 1
          const result = await this.executeImageNode({
            node: {
              ...node,
              prompt: renderWorkflowTemplate(
                node.prompt,
                workflowTemplateContext(workflow, run, predecessors),
              ),
            },
            inputs: run.inputs,
            predecessors,
            workflowId: workflow.id,
            runId: run.id,
            cwd: workflow.cwd,
            signal: controller.signal,
            ...(record.resumeOutput ? { resumeOutput: record.resumeOutput } : {}),
          })
          controller.signal.throwIfAborted()
          nodeRun.output = parseWorkflowImageOutput(result.output)
          nodeRun.summary = boundedString(result.summary, 1200)
          run.assets.push(
            ...(result.assets || []).filter(
              (asset) => !run.assets.some((item) => item.id === asset.id),
            ),
          )
        } catch (failure) {
          if (failure?.partialOutput)
            nodeRun.output = parseWorkflowImageOutput(failure.partialOutput)
          if (controller.signal.aborted) throw controller.signal.reason
          throw failure
        } finally {
          clearTimeout(timer)
          record.controller.signal.removeEventListener('abort', abort)
        }
      } else if (AGENT_KINDS.has(node.kind)) {
        await this.executeAgentNode(workflow, run, record, node, nodeRun, predecessors)
      } else {
        nodeRun.output = predecessors.map((item) => item.output)
        nodeRun.summary = WORKFLOW_NODE_SUMMARY.controlPassed
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
        nodeRun.summary = WORKFLOW_NODE_SUMMARY.skipped(message)
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
        const media = await this.resolveMediaInputs(run.inputs)
        if (record.cancelled) throw cancelledError()
        const prompt = this.agent.prompt({
          sessionId: inheritedSessionId,
          message: nodeInstruction(workflow, node, predecessors, run, media.context),
          attachments: media.attachments,
          cwd: workflow.cwd,
          title: `Workflow · ${workflow.name}`,
          // 工作流节点没有人在场，不提供 ask_user（规格 8.4）。
          unattended: true,
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
                new Error(`Node "${node.label}" timed out after ${node.timeoutMinutes} minutes.`),
                { code: 'WORKFLOW_TIMEOUT' },
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
      message: node.approval.message || workflowApprovalQuestion(node.label),
      requestedAt: new Date().toISOString(),
      expiresAt: new Date(Date.now() + node.approval.timeoutMinutes * 60_000).toISOString(),
    }
    void this.save()
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        record.approvals.delete(node.id)
        reject(pisperError('timeout', 'workflow', `approval "${node.label}"`))
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
              node: failedNode?.label || 'Unknown node',
              error: run.error,
              runId: run.id,
            },
          }
    return this.notifications.notify(event, data, { platforms: workflow.notifications })
  }

  async dispose() {
    this.closed = true
    await Promise.allSettled([...this.starting.values()])
    for (const record of this.active.values()) {
      record.cancelled = true
      record.controller.abort(cancelledError())
      for (const pending of record.approvals.values()) pending.reject(cancelledError())
      await Promise.all(
        [...record.sessionIds].map((sessionId) => this.agent.abort(sessionId).catch(() => {})),
      )
    }
    await Promise.allSettled([...this.active.values()].map((record) => record.done).filter(Boolean))
    await this.writeQueue.catch(() => {})
  }
}

function cancelledError() {
  return Object.assign(new Error('Workflow stopped.'), { code: 'WORKFLOW_CANCELLED' })
}
