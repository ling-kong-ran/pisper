import { randomUUID } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import {
  analyzeWorkflowGraph,
  createLinearWorkflowEdges,
  normalizeWorkflowEdges,
} from '../../shared/workflow-graph.mjs'

const NODE_KINDS = new Set([
  'trigger',
  'prompt',
  'file',
  'mcp',
  'notification',
  'condition',
  'parallel',
  'approval',
])
const EXECUTABLE_KINDS = new Set(['prompt', 'file', 'mcp', 'condition'])
const NOTIFICATION_TARGETS = new Set(['browser', 'feishu', 'weixin'])
const FAILURE_POLICIES = new Set(['stop', 'skip'])

function defaultState() {
  return { version: 1, workflows: [], runs: [] }
}

function clone(value) {
  return JSON.parse(JSON.stringify(value))
}

function normalizeModel(model) {
  return model?.provider && model?.model
    ? { provider: String(model.provider), model: String(model.model) }
    : null
}

function normalizeNode(node, index) {
  const kind = NODE_KINDS.has(node?.kind) ? node.kind : 'prompt'
  return {
    id: String(node?.id || randomUUID()),
    kind,
    label: String(node?.label || `步骤 ${index + 1}`)
      .trim()
      .slice(0, 120),
    prompt: String(node?.prompt || '')
      .trim()
      .slice(0, 100_000),
    x: Math.max(0, Math.min(4000, Number(node?.x) || 0)),
    y: Math.max(0, Math.min(4000, Number(node?.y) || 0)),
    model: normalizeModel(node?.model),
    retries: Math.max(0, Math.min(3, Number(node?.retries) || 0)),
    timeoutMinutes: Math.max(1, Math.min(240, Number(node?.timeoutMinutes) || 20)),
    failurePolicy: FAILURE_POLICIES.has(node?.failurePolicy) ? node.failurePolicy : 'stop',
    enabled: node?.enabled !== false,
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
    name: String(workflow?.name || '未命名工作流')
      .trim()
      .slice(0, 120),
    description: String(workflow?.description || '')
      .trim()
      .slice(0, 600),
    status: workflow?.status === 'published' ? 'published' : 'draft',
    cwd: String(workflow?.cwd || cwd),
    model: normalizeModel(workflow?.model),
    notifications: [
      ...new Set(
        (Array.isArray(workflow?.notifications) ? workflow.notifications : []).filter((target) =>
          NOTIFICATION_TARGETS.has(target),
        ),
      ),
    ],
    nodes,
    edges: normalizeWorkflowEdges(sourceEdges.slice(0, 300), nodes, () => randomUUID()),
    createdAt: workflow?.createdAt || now,
    updatedAt: workflow?.updatedAt || now,
    publishedAt: workflow?.publishedAt || null,
    lastRunAt: workflow?.lastRunAt || null,
    lastStatus: workflow?.lastStatus || 'idle',
    lastSummary: String(workflow?.lastSummary || '').slice(0, 1200),
    lastError: String(workflow?.lastError || '').slice(0, 1200),
  }
}

function durationLabel(durationMs) {
  const seconds = Math.max(0, Math.round(durationMs / 1000))
  if (seconds < 60) return `${seconds} 秒`
  return `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`
}

function nodeInstruction(workflow, node, previousSummary) {
  const kindHints = {
    prompt: '完成这个 Agent 任务。',
    file: '使用可用的文件工具完成这个文件处理任务。',
    mcp: '优先使用已启用的 MCP 工具完成这个任务。',
    condition: '检查条件并给出明确结论，然后说明依据。',
  }
  return [
    `你正在执行工作流「${workflow.name}」的节点「${node.label}」。`,
    kindHints[node.kind] || '',
    node.prompt,
    previousSummary ? `\n前序节点结果：\n${previousSummary}` : '',
    '\n只处理当前节点，完成后简洁总结结果，供后续节点继续使用。',
  ]
    .filter(Boolean)
    .join('\n')
}

function validateRunnable(workflow) {
  const graph = analyzeWorkflowGraph(workflow.nodes, workflow.edges)
  const executable = graph.nodes.filter((node) => EXECUTABLE_KINDS.has(node.kind))
  if (!executable.length) throw new Error('工作流至少需要一个可执行节点。')
  const invalid = executable.find((node) => !node.prompt)
  if (invalid) throw new Error(`节点「${invalid.label}」还没有填写 Prompt。`)
  if (graph.nodes.length > 1 && !graph.edges.length) throw new Error('工作流节点尚未建立连接。')
  if (graph.invalidTriggerTargets.length)
    throw new Error(`触发器「${graph.invalidTriggerTargets[0].label}」不能连接上游节点。`)
  if (graph.unconnected.length)
    throw new Error(`节点「${graph.unconnected[0].label}」尚未连接到工作流。`)
  if (graph.hasCycle) throw new Error('工作流不能包含循环连接。')
  return graph
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
    const stored = await readJson(this.path, defaultState())
    this.state = {
      version: 1,
      workflows: (Array.isArray(stored.workflows) ? stored.workflows : []).map((workflow) => {
        const normalized = normalizeStoredWorkflow(workflow, this.cwd)
        if (normalized.lastStatus === 'running') normalized.lastStatus = 'interrupted'
        return normalized
      }),
      runs: (Array.isArray(stored.runs) ? stored.runs : []).slice(-200).map((run) =>
        run.status === 'running'
          ? {
              ...run,
              status: 'interrupted',
              finishedAt: new Date().toISOString(),
              error: '应用重启，工作流运行已中断。',
            }
          : run,
      ),
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

  getState() {
    return {
      ...clone(this.state),
      limits: { maxConcurrent: this.maxConcurrent, running: this.active.size },
    }
  }

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
    workflow.lastRunAt = current.lastRunAt
    workflow.lastStatus = current.lastStatus
    workflow.lastSummary = current.lastSummary
    workflow.lastError = current.lastError
    this.state.workflows[index] = workflow
    await this.save()
    return clone(workflow)
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

  async runNow(id, { trigger = 'manual' } = {}) {
    const workflow = this.state.workflows.find((item) => item.id === id)
    if (!workflow) return null
    if ([...this.active.values()].some((record) => record.workflowId === id))
      throw new Error('工作流已经在运行。')
    if (this.active.size >= this.maxConcurrent)
      throw new Error(`工作流并发已达到上限（${this.maxConcurrent}）。`)
    const graph = validateRunnable(workflow)
    const run = {
      id: randomUUID(),
      workflowId: workflow.id,
      workflowName: workflow.name,
      trigger,
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
        error: '',
        sessionId: '',
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
    }
    this.active.set(run.id, record)
    await this.save()
    void this.execute(workflow, run, graph, record)
    return clone(run)
  }

  async stop(runId) {
    const record = this.active.get(runId)
    if (!record) return null
    record.cancelled = true
    await Promise.all(
      [...record.sessionIds].map((sessionId) => this.agent.abort(sessionId).catch(() => {})),
    )
    return clone(this.state.runs.find((run) => run.id === runId))
  }

  async execute(workflow, run, graph, record) {
    const started = Date.now()
    let failedNode = null
    try {
      for (const node of graph.order) {
        if (record.cancelled)
          throw Object.assign(new Error('工作流已停止。'), { code: 'WORKFLOW_CANCELLED' })
        const nodeRun = run.nodes.find((item) => item.id === node.id)
        const predecessors = (graph.incoming.get(node.id) || [])
          .map((edge) => run.nodes.find((item) => item.id === edge.source))
          .filter(Boolean)
        const previousSummary = predecessors
          .map((item) => `${item.label}：${item.summary || item.error || '已完成'}`)
          .join('\n')
        run.currentNodeId = node.id
        run.currentNodeLabel = node.label
        nodeRun.status = 'running'
        await this.save()

        if (!EXECUTABLE_KINDS.has(node.kind)) {
          nodeRun.status = 'completed'
          nodeRun.summary =
            predecessors.length === 1
              ? predecessors[0].summary
              : previousSummary ||
                (node.kind === 'notification'
                  ? '通知将在工作流结束后发送。'
                  : node.kind === 'trigger'
                    ? '工作流已触发。'
                    : '控制节点已通过。')
          run.completedNodes += 1
          await this.save()
          continue
        }

        let lastError = null
        for (let attempt = 0; attempt <= node.retries; attempt += 1) {
          nodeRun.attempts = attempt + 1
          try {
            let timeoutTimer
            const predecessor = predecessors.length === 1 ? predecessors[0] : null
            const predecessorBranches = predecessor
              ? (graph.outgoing.get(predecessor.id) || []).length
              : 0
            const inheritedSessionId =
              predecessor && predecessorBranches === 1 ? predecessor.sessionId : ''
            let activeSessionId = inheritedSessionId
            const prompt = this.agent.prompt({
              sessionId: inheritedSessionId,
              message: nodeInstruction(workflow, node, previousSummary),
              cwd: workflow.cwd,
              title: `工作流 · ${workflow.name}`,
              model: node.model || workflow.model,
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
                const error = Object.assign(
                  new Error(`节点「${node.label}」执行超过 ${node.timeoutMinutes} 分钟。`),
                  { code: 'WORKFLOW_TIMEOUT' },
                )
                reject(error)
              }, node.timeoutMinutes * 60_000)
              timeoutTimer.unref?.()
            })
            let result
            try {
              result = await Promise.race([prompt, timeout])
            } finally {
              clearTimeout(timeoutTimer)
            }
            if (record.cancelled)
              throw Object.assign(new Error('工作流已停止。'), { code: 'WORKFLOW_CANCELLED' })
            run.sessionId = result.sessionId || run.sessionId
            nodeRun.sessionId = result.sessionId || nodeRun.sessionId
            if (nodeRun.sessionId) record.sessionIds.add(nodeRun.sessionId)
            nodeRun.summary = String(result.text || '节点已完成。')
              .trim()
              .slice(0, 1200)
            nodeRun.status = 'completed'
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
        if (lastError) {
          const message = lastError instanceof Error ? lastError.message : String(lastError)
          nodeRun.error = message
          if (node.failurePolicy === 'skip') {
            nodeRun.status = 'skipped'
            nodeRun.summary = `已跳过：${message}`
          } else {
            nodeRun.status = 'failed'
            failedNode = node
            throw lastError
          }
        }
        run.completedNodes += 1
        await this.save()
      }

      run.status = 'completed'
      const terminalNodes = graph.order.filter(
        (node) => !(graph.outgoing.get(node.id) || []).length,
      )
      run.summary =
        terminalNodes
          .map((node) => run.nodes.find((item) => item.id === node.id)?.summary)
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
      const runningNode = run.nodes.find((item) => item.status === 'running')
      if (runningNode) {
        runningNode.status = cancelled ? 'cancelled' : 'failed'
        runningNode.error = message
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
      await this.notifications
        .notify(
          'workflow.completed',
          {
            workflow: {
              name: workflow.name,
              summary: run.summary,
              duration: durationLabel(run.durationMs),
              runId: run.id,
            },
          },
          { platforms: workflow.notifications },
        )
        .catch(() => {})
    } else if (workflow.notifications.length && run.status === 'failed') {
      await this.notifications
        .notify(
          'workflow.failed',
          {
            workflow: {
              name: workflow.name,
              node:
                failedNode?.label ||
                run.nodes.find((node) => node.status === 'failed')?.label ||
                '未知节点',
              error: run.error,
              runId: run.id,
            },
          },
          { platforms: workflow.notifications },
        )
        .catch(() => {})
    }
  }

  async dispose() {
    for (const record of this.active.values()) {
      record.cancelled = true
      await Promise.all(
        [...record.sessionIds].map((sessionId) => this.agent.abort(sessionId).catch(() => {})),
      )
    }
    await this.writeQueue.catch(() => {})
  }
}
