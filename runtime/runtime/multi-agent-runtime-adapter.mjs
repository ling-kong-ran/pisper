// 多 Agent 运行时适配：把服务能力收敛为 Pi 工具所需的窄接口，避免主运行时继续膨胀。
import { createHash } from 'node:crypto'
import { readFile, realpath } from 'node:fs/promises'
import { relative, resolve, sep } from 'node:path'
import { Worker } from 'node:worker_threads'
import { createPisperBashTool } from '../tools/host-bash.mjs'
import { createTeamMemberTools, TEAM_MEMBER_TOOL_NAMES } from '../tools/app/multi-agent.mjs'
import { filterToolsForExecutionMode } from '../security/execution-mode.mjs'
import { localDayKey } from './conversation-memory-capture.mjs'

const MAX_TEAM_SCRIPT_BYTES = 256 * 1024
const MAX_WORKFLOW_RESULT_BYTES = 1024 * 1024
const MAX_WORKFLOW_AGENTS = 64
export const WORKFLOW_VM_TIMEOUT_MS = 5_000

function executeTeamWorkflowWorker({ body, path, argsJson, runAgent }) {
  return new Promise((resolveResult, rejectResult) => {
    const worker = new Worker(new URL('../workers/team-workflow-worker.mjs', import.meta.url), {
      resourceLimits: {
        maxOldGenerationSizeMb: 64,
        maxYoungGenerationSizeMb: 16,
        stackSizeMb: 4,
      },
      workerData: {
        body,
        path,
        argsJson,
        maxAgents: MAX_WORKFLOW_AGENTS,
        maxResultBytes: MAX_WORKFLOW_RESULT_BYTES,
        vmTimeoutMs: WORKFLOW_VM_TIMEOUT_MS,
      },
    })
    let settled = false
    let watchdog = null
    const finish = (callback, value) => {
      if (settled) return
      settled = true
      if (watchdog) clearTimeout(watchdog)
      worker.removeAllListeners()
      void worker.terminate()
      callback(value)
    }
    const armWatchdog = () => {
      if (watchdog) clearTimeout(watchdog)
      watchdog = setTimeout(() => {
        finish(
          rejectResult,
          new Error(`Team workflow script did not yield within ${WORKFLOW_VM_TIMEOUT_MS}ms.`),
        )
      }, WORKFLOW_VM_TIMEOUT_MS + 250)
      watchdog.unref?.()
    }
    // Worker 退出与异步 Agent 结果可能同时发生，统一捕获回传失败避免产生未处理 rejection。
    const sendAgentResponse = (message) => {
      if (settled) return
      try {
        worker.postMessage(message)
      } catch (error) {
        finish(rejectResult, error)
      }
    }
    worker.on('message', (message) => {
      if (message?.type === 'heartbeat') {
        armWatchdog()
        return
      }
      if (message?.type === 'agent_request') {
        void (async () => {
          try {
            const payload = String(message.payload || '{}')
            if (Buffer.byteLength(payload, 'utf8') > MAX_WORKFLOW_RESULT_BYTES)
              throw new Error(
                `Workflow Agent requests are limited to ${MAX_WORKFLOW_RESULT_BYTES} bytes.`,
              )
            const result = await runAgent(JSON.parse(payload))
            sendAgentResponse({
              type: 'agent_response',
              id: message.id,
              resultJson: JSON.stringify(result) || 'null',
            })
          } catch (error) {
            sendAgentResponse({
              type: 'agent_response',
              id: message.id,
              error: {
                message: error instanceof Error ? error.message : String(error),
                ...(error?.code ? { code: String(error.code) } : {}),
              },
            })
          }
        })()
        return
      }
      if (message?.type === 'result') {
        try {
          finish(resolveResult, {
            logs: Array.isArray(message.logs) ? message.logs : [],
            result: JSON.parse(String(message.resultJson || 'null')),
          })
        } catch (error) {
          finish(rejectResult, error)
        }
        return
      }
      if (message?.type === 'error') finish(rejectResult, new Error(String(message.error || '')))
    })
    worker.once('error', (error) => finish(rejectResult, error))
    worker.once('exit', (code) => {
      if (!settled)
        finish(rejectResult, new Error(`Team workflow worker exited unexpectedly (${code}).`))
    })
    armWatchdog()
  })
}

const WORKFLOW_FORBIDDEN_PATTERNS = [
  /\bimport\s*(?:\(|[A-Za-z{*])/,
  /\brequire\s*\(/,
  /\b(?:process|globalThis|global|Buffer|Deno|Bun)\s*[.[]/,
  /\b(?:eval|Function)\s*\(/,
  /\bDate\s*\.\s*(?:now|parse)\s*\(/,
  /\bMath\s*\.\s*random\s*\(/,
  /\b__pisper[A-Za-z0-9_]*\b/,
  /\bwhile\s*\(\s*(?:true|1)\s*\)/,
  /\bfor\s*\(\s*;\s*;\s*\)/,
]

function normalizedScriptName(value) {
  return String(value || '')
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '_')
    .replace(/_+/g, '_')
    .replace(/^_|_$/g, '')
    .slice(0, 48)
}

function findObjectLiteralEnd(source, start) {
  if (source[start] !== '{') throw new Error('Workflow meta must be a plain object literal.')
  let depth = 0
  let quote = ''
  let escaped = false
  for (let index = start; index < source.length; index += 1) {
    const character = source[index]
    if (quote) {
      if (escaped) escaped = false
      else if (character === '\\') escaped = true
      else if (character === quote) quote = ''
      continue
    }
    if (character === '"' || character === "'" || character === '`') {
      quote = character
      continue
    }
    if (character === '{') depth += 1
    else if (character === '}') {
      depth -= 1
      if (depth === 0) return index + 1
    }
  }
  throw new Error('Workflow meta object is incomplete.')
}

function parseWorkflowString(source, start) {
  const quote = source[start]
  if (quote !== '"' && quote !== "'")
    throw new Error('Workflow meta values must be quoted strings.')
  const escapes = { b: '\b', f: '\f', n: '\n', r: '\r', t: '\t', v: '\v', 0: '\0' }
  let value = ''
  for (let index = start + 1; index < source.length; index += 1) {
    const character = source[index]
    if (character === quote) return { value, end: index + 1 }
    if (character !== '\\') {
      value += character
      continue
    }
    const escaped = source[++index]
    if (escaped === 'u' || escaped === 'x') {
      const length = escaped === 'u' ? 4 : 2
      const hex = source.slice(index + 1, index + 1 + length)
      if (!new RegExp(`^[0-9a-fA-F]{${length}}$`).test(hex))
        throw new Error('Workflow meta contains an invalid string escape.')
      value += String.fromCodePoint(Number.parseInt(hex, 16))
      index += length
    } else value += escapes[escaped] ?? escaped
  }
  throw new Error('Workflow meta contains an incomplete string.')
}

function parseWorkflowMeta(source) {
  const values = Object.create(null)
  let index = 1
  const skipWhitespace = () => {
    while (/\s/.test(source[index] || '')) index += 1
  }
  while (index < source.length - 1) {
    skipWhitespace()
    if (source[index] === '}') break
    const keyMatch = /^[A-Za-z_$][A-Za-z0-9_$]*/.exec(source.slice(index))
    if (!keyMatch || !['name', 'description'].includes(keyMatch[0]))
      throw new Error('Workflow meta supports only literal name and description fields.')
    const key = keyMatch[0]
    if (Object.hasOwn(values, key)) throw new Error(`Workflow meta repeats ${key}.`)
    index += key.length
    skipWhitespace()
    if (source[index++] !== ':') throw new Error(`Workflow meta ${key} is missing a colon.`)
    skipWhitespace()
    const parsed = parseWorkflowString(source, index)
    values[key] = parsed.value
    index = parsed.end
    skipWhitespace()
    if (source[index] === ',') index += 1
    else if (source[index] !== '}')
      throw new Error('Workflow meta fields must be separated by commas.')
  }
  const name = String(values.name || '').trim()
  const description = String(values.description || '').trim()
  if (!name || !description) throw new Error('Workflow meta requires name and description.')
  if (name.length > 80 || description.length > 500)
    throw new Error('Workflow meta name or description is too long.')
  return { name, description }
}

function parseWorkflowSource(source) {
  for (const pattern of WORKFLOW_FORBIDDEN_PATTERNS)
    if (pattern.test(source))
      throw new Error(`Workflow script uses a forbidden capability: ${pattern}.`)
  const declaration = /^\s*export\s+const\s+meta\s*=\s*/.exec(source)
  if (!declaration) throw new Error('Workflow script must begin with export const meta.')
  const metaStart = declaration[0].length
  const metaEnd = findObjectLiteralEnd(source, metaStart)
  const meta = parseWorkflowMeta(source.slice(metaStart, metaEnd))
  const body = source.slice(metaEnd).trimStart()
  if (/\bexport\s+/.test(body))
    throw new Error('Workflow scripts may not export values beyond meta.')
  return { meta, body }
}

async function readWorkflowScript(cwd, requestedPath) {
  const root = resolve(cwd)
  const candidate = resolve(root, String(requestedPath || '').trim())
  if (candidate !== root && !candidate.startsWith(`${root}${sep}`))
    throw new Error('Team workflow scripts must stay inside the current workspace.')
  const [realRoot, realCandidate] = await Promise.all([realpath(root), realpath(candidate)])
  if (realCandidate !== realRoot && !realCandidate.startsWith(`${realRoot}${sep}`))
    throw new Error('Team workflow scripts must stay inside the current workspace.')
  const source = await readFile(realCandidate, 'utf8')
  if (Buffer.byteLength(source, 'utf8') > MAX_TEAM_SCRIPT_BYTES)
    throw new Error(`Team workflow scripts are limited to ${MAX_TEAM_SCRIPT_BYTES} bytes.`)
  const parsed = parseWorkflowSource(source)
  return {
    ...parsed,
    path: relative(root, candidate) || candidate,
    scriptFingerprint: createHash('sha256').update(source).digest('hex'),
  }
}

export function multiAgentUpdateActivity(agent, updatedAgent, previousActivity) {
  const communication = agent?.communication || null
  const currentActivity = communication
    ? {
        type: 'communication',
        id: communication.id,
        communication,
        updatedAt: communication.sentAt,
      }
    : updatedAgent
      ? {
          type: 'agent',
          agent: updatedAgent,
          updatedAt: updatedAgent.lastActivityAt || new Date().toISOString(),
        }
      : previousActivity || null
  return { communication, currentActivity }
}

// 子 Agent 完成后的用量归账集中在适配层；Team 用量只做统一统计，预算仅在用户显式设置时生效。
export function createSubagentUsageHandler({
  runtimeService,
  getRuntimeSession,
  getRuntimeValue,
  teamWorkflows,
  multiAgents,
}) {
  return async ({ id, runNumber, runUsage, completedAt }) => {
    const sessionId = getRuntimeSession()?.sessionId
    await runtimeService.recordUsage(
      localDayKey(completedAt),
      `agent:${sessionId}:${id}:${runNumber}`,
      runUsage,
    )
    const goal = runtimeService.goals.get(sessionId)
    if (goal?.status !== 'active') return
    const updatedGoal = await runtimeService.goals.account(sessionId, {
      goalId: goal.id,
      usage: runUsage,
    })
    if (updatedGoal?.mode === 'team' && updatedGoal.status === 'budget_limited') {
      await teamWorkflows.markBudgetLimited(sessionId, updatedGoal.teamTokenBudget)
      multiAgents.abortParent(
        sessionId,
        'Team token budget was reached; remaining members were stopped.',
      )
    }
    const runtimeValue = getRuntimeValue()
    if (runtimeValue) runtimeService.syncGoalTools(runtimeValue, updatedGoal)
    runtimeService.emitGoalUpdate(sessionId, updatedGoal)
  }
}

export function createMultiAgentRuntime({
  getRuntimeSession,
  multiAgents,
  teamWorkflows,
  effectiveCwd,
  executionMode,
  enabledTools,
  planReader,
  baseToolNames,
  getExecutionMode,
  getToolRisk,
  createInheritedCustomTools,
  waitAgent,
  installSubagentPermissions,
  onCompleted,
  emitAgentUpdate,
}) {
  const agentLeaseBindings = new Map()

  // 待运行输入已完整持久化在 Team task 中；恢复时直接重建，避免进程内 Map 丢失依赖任务。
  async function schedulePendingTeamSpawns(sessionId) {
    if (!teamWorkflows.isActive(sessionId)) return
    const expired = await teamWorkflows.requeueExpiredLeases(sessionId)
    for (const lease of expired) {
      if (!lease.agentId) continue
      agentLeaseBindings.delete(lease.agentId)
      try {
        multiAgents.interrupt(
          sessionId,
          lease.agentId,
          `Team task lease expired: ${lease.taskName}.`,
        )
      } catch {}
    }
    for (const task of teamWorkflows.readyTasks(sessionId)) {
      if (!task.autoStart) continue
      await spawn({
        taskName: task.taskName,
        role: task.role,
        message: task.message,
        files: task.files,
        dependsOn: task.dependsOn,
        autoStart: task.autoStart,
        workflowFingerprint: task.workflowFingerprint,
        teamTaskId: task.id,
      }).catch(() => {})
    }
  }

  function teamMemberTask(sessionId, agentId) {
    return teamWorkflows.get(sessionId)?.tasks?.find((task) => task.agentId === agentId) || null
  }

  async function sendTeamMessage(senderId, target, message) {
    const runtimeSession = getRuntimeSession()
    const sessionId = runtimeSession?.sessionId
    if (!sessionId || !teamWorkflows.isActive(sessionId))
      throw new Error('No active Team is available.')
    const recipientBefore = multiAgents.find(sessionId, target)
    if (!recipientBefore) throw new Error(`Unknown agent: ${target}`)
    const recipient = await multiAgents.sendMessageFromAgent(sessionId, senderId, target, message)
    const senderTask = teamMemberTask(sessionId, senderId)
    const recipientTask = teamMemberTask(sessionId, recipient.id)
    const communication = await teamWorkflows.recordCommunication(sessionId, {
      fromAgentId: senderId,
      fromTaskName: senderTask?.taskName,
      toAgentId: recipient.id,
      toTaskName: recipientTask?.taskName || recipient.taskName,
      message,
      status: ['queued', 'starting'].includes(recipientBefore.status) ? 'queued' : 'delivered',
    })
    emitAgentUpdate(sessionId, { id: senderId, communication })
    return { agent: recipient, communication }
  }

  function listTeamMembers(sessionId) {
    const team = teamWorkflows.get(sessionId)
    return multiAgents.list(sessionId).map((agent) => {
      const task = team?.tasks?.find((candidate) => candidate.agentId === agent.id)
      return {
        id: agent.id,
        canonicalName: agent.canonicalName,
        taskName: task?.taskName || agent.taskName,
        role: task?.role || '',
        status: agent.status,
        output: agent.output,
        currentActivity: agent.currentActivity,
      }
    })
  }

  const runScript = async (requestedPath, scriptArgs) => {
    const runtimeSession = getRuntimeSession()
    const sessionId = runtimeSession?.sessionId
    if (!sessionId || !teamWorkflows.isActive(sessionId))
      throw new Error('No active Team is available.')
    const { body, meta, path, scriptFingerprint } = await readWorkflowScript(
      effectiveCwd,
      requestedPath,
    )
    await teamWorkflows.setScriptPath(sessionId, path)
    const workflowTasks = new Map()
    const workflowAliases = new Map()
    let taskNumber = 0
    const argsJson = scriptArgs === undefined ? '' : JSON.stringify(scriptArgs)
    if (Buffer.byteLength(argsJson, 'utf8') > MAX_WORKFLOW_RESULT_BYTES)
      throw new Error(`Workflow args are limited to ${MAX_WORKFLOW_RESULT_BYTES} bytes.`)

    const waitForAgent = async (agentId) => {
      while (true) {
        const result = await waitAgent(30_000, agentId)
        if (!result?.timedOut) return result?.agent || multiAgents.find(sessionId, agentId)
      }
    }

    const waitForTask = async (taskId) => {
      let waitedAgentId = ''
      while (true) {
        const task = teamWorkflows.getTask(sessionId, taskId)
        if (!task) throw new Error(`Workflow task disappeared while waiting: ${taskId}`)
        if (task.status === 'completed') return task
        if (['failed', 'interrupted'].includes(task.status))
          throw new Error(task.error || `Workflow task ${task.taskName} ${task.status}.`)
        if (task.agentId && task.agentId !== waitedAgentId) {
          waitedAgentId = task.agentId
          await waitForAgent(task.agentId)
        } else {
          await schedulePendingTeamSpawns(sessionId)
          await new Promise((resolvePromise) => setTimeout(resolvePromise, 100))
        }
      }
    }

    const structuredResult = (output, schema) => {
      if (!schema) return output
      const text = String(output || '').trim()
      const fenced = text.match(/^```(?:json)?\s*([\s\S]*?)\s*```$/i)?.[1] || text
      try {
        return JSON.parse(fenced)
      } catch {
        return null
      }
    }

    const runAgent = async (prompt, options = {}) => {
      // 每次桥接调用都重新检查 Team，防止脚本暂停后仍通过已捕获的 agent 闭包启动成员。
      if (!teamWorkflows.isActive(sessionId)) throw new Error('Team workflow is no longer active.')
      if (++taskNumber > MAX_WORKFLOW_AGENTS)
        throw new Error(`A Team workflow cannot start more than ${MAX_WORKFLOW_AGENTS} agents.`)
      const label = String(options?.label || options?.taskName || `agent_${taskNumber}`).trim()
      const currentPhase = String(options?.__pisperPhase || '').trim()
      const baseName = normalizedScriptName(
        `${currentPhase ? `${currentPhase}_` : ''}${label}_${taskNumber}`,
      )
      if (!baseName) throw new Error('Workflow agent labels must contain usable characters.')
      const taskName = baseName
      const dependencies = (Array.isArray(options?.dependsOn) ? options.dependsOn : [])
        .map((dependency) => workflowAliases.get(normalizedScriptName(dependency)) || dependency)
        .map(normalizedScriptName)
        .filter(Boolean)
      const schemaText = options?.schema ? JSON.stringify(options.schema) : ''
      const message = `${String(prompt || '').trim()}${
        schemaText ? `\n\nReturn only a JSON value matching this schema:\n${schemaText}` : ''
      }`
      const workflowFingerprint = createHash('sha256')
        .update(
          JSON.stringify({
            scriptFingerprint,
            args: argsJson,
            taskName,
            message,
            role: options?.role || '',
            files: options?.files || [],
            dependsOn: dependencies,
            schema: schemaText,
          }),
        )
        .digest('hex')
      const previous = teamWorkflows.findTask(sessionId, taskName)
      if (previous) {
        workflowTasks.set(taskName, previous.id)
        workflowAliases.set(normalizedScriptName(label), taskName)
      }
      if (previous?.status === 'completed' && previous.workflowFingerprint === workflowFingerprint)
        return structuredResult(previous.output, options?.schema)
      if (previous && ['starting', 'running'].includes(previous.status)) {
        if (previous.workflowFingerprint !== workflowFingerprint)
          throw new Error(`Workflow task ${taskName} is already running with different inputs.`)
        const completed = await waitForTask(previous.id)
        return structuredResult(completed?.output || '', options?.schema)
      }
      const agent = await spawn({
        taskName,
        role: options?.role,
        message,
        files: options?.files,
        dependsOn: dependencies,
        autoStart: true,
        workflowFingerprint,
        ...(previous &&
        ['queued', 'blocked', 'completed', 'failed', 'interrupted'].includes(previous.status)
          ? { teamTaskId: previous.id }
          : {}),
      })
      const taskId = agent.teamTaskId || teamWorkflows.findTask(sessionId, taskName)?.id
      if (!taskId) return structuredResult(agent.output || '', options?.schema)
      workflowTasks.set(taskName, taskId)
      workflowAliases.set(normalizedScriptName(label), taskName)
      const completed = await waitForTask(taskId)
      return structuredResult(completed?.output || agent.output || '', options?.schema)
    }

    const { logs, result } = await executeTeamWorkflowWorker({
      body,
      path,
      argsJson,
      runAgent: (input) => runAgent(input.prompt, input.options),
    })
    return {
      scriptPath: path,
      meta,
      logs,
      result,
      taskCount: workflowTasks.size,
      tasks: teamWorkflows.get(sessionId)?.tasks || [],
      team: teamWorkflows.get(sessionId),
    }
  }

  const spawn = async (input) => {
    const runtimeSession = getRuntimeSession()
    if (!runtimeSession?.model) throw new Error('当前会话没有可用模型，无法启动 Agent。')
    let teamTask = null
    let reusedTeamTask = false
    let initialAgentRunNumber = null
    let resolveTaskBinding = null
    const existingTeamTaskId = String(input?.teamTaskId || '').trim()
    const teamInput = existingTeamTaskId ? input : { ...input, autoStart: true }
    try {
      const sessionId = runtimeSession.sessionId
      const existingTask = teamWorkflows.isActive(sessionId)
        ? teamWorkflows.findTask?.(sessionId, input?.taskName)
        : null
      reusedTeamTask = Boolean(
        existingTeamTaskId ||
        (existingTask &&
          ['blocked', 'interrupted', 'failed', 'queued'].includes(existingTask.status)),
      )
      teamTask = teamWorkflows.isActive(sessionId)
        ? existingTeamTaskId
          ? await teamWorkflows.updateTask(sessionId, existingTeamTaskId, teamInput)
          : existingTask &&
              ['blocked', 'interrupted', 'failed', 'queued'].includes(existingTask.status)
            ? await teamWorkflows.updateTask(sessionId, existingTask.id, teamInput)
            : await teamWorkflows.registerTask(sessionId, teamInput)
        : null
      if (existingTeamTaskId && !teamTask)
        throw new Error(`Unknown Team task: ${existingTeamTaskId}`)
      if (teamTask && !teamWorkflows.taskReady(sessionId, teamTask.id)) {
        return {
          id: `team:${teamTask.id}`,
          taskName: teamTask.taskName,
          canonicalName: `/team/${teamTask.taskName}`,
          status: 'queued',
          message: teamTask.message,
          output: 'Waiting for Team dependencies before starting this task.',
          teamTaskId: teamTask.id,
        }
      }
      if (teamTask) {
        const claimed = await teamWorkflows.claimTask(sessionId, teamTask.id)
        if (!claimed) throw new Error(`Team task ${teamTask.taskName} is not ready to run.`)
        teamTask = claimed
      }
      const claimedLease = teamTask ? { taskId: teamTask.id, leaseId: teamTask.leaseId } : null
      const taskBinding = claimedLease
        ? new Promise((resolveBinding) => {
            resolveTaskBinding = resolveBinding
          })
        : Promise.resolve(null)
      const syncLeasedProgress = async (progress) => {
        if (!claimedLease) {
          emitAgentUpdate(sessionId, progress)
          return true
        }
        const progressRunNumber = Number(progress?.runNumber) || 0
        const binding =
          initialAgentRunNumber == null || progressRunNumber <= initialAgentRunNumber
            ? await taskBinding
            : agentLeaseBindings.get(progress?.id)
        if (!binding) return false
        const accepted = await teamWorkflows
          .updateLeasedAgent(sessionId, binding.taskId, binding.leaseId, progress)
          .catch(() => null)
        if (!accepted) return false
        if (progress?.status === 'running')
          await teamWorkflows.markCommunicationsDelivered(sessionId, progress.id).catch(() => {})
        if (['completed', 'failed', 'interrupted'].includes(progress?.status))
          agentLeaseBindings.delete(progress.id)
        emitAgentUpdate(sessionId, progress)
        return true
      }
      const agent = await multiAgents.spawn({
        ...input,
        parentSessionId: sessionId,
        cwd: effectiveCwd,
        model: runtimeSession.model,
        thinkingLevel: runtimeSession.thinkingLevel,
        allowedTools: [
          ...(() => {
            const active = new Set(runtimeSession?.getActiveToolNames?.() || [])
            return filterToolsForExecutionMode(
              baseToolNames.filter((name) => active.has(name)),
              getExecutionMode(),
              getToolRisk,
            )
          })(),
          ...(planReader ? [planReader.name] : []),
          ...(teamWorkflows.isActive(sessionId) ? TEAM_MEMBER_TOOL_NAMES : []),
        ],
        createCustomTools: async ({ id }) => {
          const childBashTool =
            enabledTools.includes('bash') &&
            ['approval-required', 'workspace-write', 'full-access'].includes(executionMode)
              ? await createPisperBashTool(effectiveCwd)
              : null
          return {
            tools: [
              ...createInheritedCustomTools(childBashTool),
              ...(planReader ? [planReader] : []),
              ...(teamWorkflows.isActive(sessionId)
                ? createTeamMemberTools({
                    teamMemberRuntime: {
                      sendMessage: (target, message) => sendTeamMessage(id, target, message),
                      listMembers: async () => listTeamMembers(sessionId),
                    },
                  })
                : []),
            ],
          }
        },
        onProgress: (progress) => {
          void syncLeasedProgress(progress)
        },
        onSession: installSubagentPermissions,
        onCompleted,
        onTerminal: async (terminal) => {
          if (!teamWorkflows.get(sessionId)) return
          await syncLeasedProgress(terminal)
          await schedulePendingTeamSpawns(sessionId)
        },
      })
      initialAgentRunNumber = Number(agent.runNumber) || 0
      if (teamTask) {
        const bound = await teamWorkflows.bindAgent(sessionId, teamTask.id, agent, {
          leaseId: teamTask.leaseId,
        })
        if (bound)
          agentLeaseBindings.set(agent.id, {
            taskId: teamTask.id,
            leaseId: teamTask.leaseId,
          })
        resolveTaskBinding?.(bound ? { taskId: teamTask.id, leaseId: teamTask.leaseId } : null)
        resolveTaskBinding = null
        if (!bound) {
          try {
            multiAgents.interrupt(
              sessionId,
              agent.id,
              `Team task lease changed before Agent binding: ${teamTask.taskName}.`,
            )
          } catch {}
          throw new Error(`Team task lease changed before Agent binding: ${teamTask.taskName}.`)
        }
        emitAgentUpdate(sessionId, agent)
        await schedulePendingTeamSpawns(sessionId)
      }
      return teamTask ? { ...agent, teamTaskId: teamTask.id } : agent
    } catch (error) {
      resolveTaskBinding?.(null)
      resolveTaskBinding = null
      if (teamTask) {
        if (reusedTeamTask)
          await teamWorkflows
            .releaseTask(runtimeSession.sessionId, teamTask.id, error, {
              leaseId: teamTask.leaseId,
            })
            .catch(() => {})
        else await teamWorkflows.discardTask(runtimeSession.sessionId, teamTask.id)
      }
      emitAgentUpdate(runtimeSession.sessionId, {
        id: '',
        status: 'failed',
        lastActivityAt: new Date().toISOString(),
      })
      throw error
    }
  }

  return {
    spawn,
    resume: async () => schedulePendingTeamSpawns(getRuntimeSession().sessionId),
    list: () => multiAgents.list(getRuntimeSession().sessionId),
    sendMessage: async (target, message) => {
      const sessionId = getRuntimeSession().sessionId
      const recipientBefore = multiAgents.find(sessionId, target)
      const agent = await multiAgents.sendMessage(sessionId, target, message)
      if (teamWorkflows.isActive(sessionId)) {
        const communication = await teamWorkflows.recordCommunication(sessionId, {
          fromAgentId: 'lead',
          toAgentId: agent.id,
          toTaskName: teamMemberTask(sessionId, agent.id)?.taskName || agent.taskName,
          message,
          status: ['queued', 'starting'].includes(recipientBefore?.status) ? 'queued' : 'delivered',
        })
        emitAgentUpdate(sessionId, { id: agent.id, communication })
        return { ...agent, communication }
      }
      return agent
    },
    followup: async (target, message) => {
      const sessionId = getRuntimeSession().sessionId
      const existing = multiAgents.find(sessionId, target)
      let retryBinding = null
      if (
        existing &&
        teamWorkflows.isActive(sessionId) &&
        ['completed', 'failed', 'interrupted'].includes(existing.status)
      ) {
        const retried = await teamWorkflows.prepareRetry(sessionId, existing)
        const claimed = retried ? await teamWorkflows.claimTask(sessionId, retried.id) : null
        if (!claimed) throw new Error(`Team task ${existing.taskName} could not be retried.`)
        retryBinding = { taskId: claimed.id, leaseId: claimed.leaseId }
        agentLeaseBindings.set(existing.id, retryBinding)
      }
      try {
        const agent = await multiAgents.followup(sessionId, target, message)
        if (teamWorkflows.isActive(sessionId)) {
          const binding = agentLeaseBindings.get(agent.id)
          const accepted = binding
            ? await teamWorkflows
                .updateLeasedAgent(sessionId, binding.taskId, binding.leaseId, agent)
                .catch(() => null)
            : null
          if (accepted) emitAgentUpdate(sessionId, agent)
        }
        return agent
      } catch (error) {
        if (existing && retryBinding && teamWorkflows.isActive(sessionId)) {
          agentLeaseBindings.delete(existing.id)
          await teamWorkflows
            .releaseTask(sessionId, retryBinding.taskId, error, {
              leaseId: retryBinding.leaseId,
            })
            .catch(() => {})
          emitAgentUpdate(sessionId, {
            id: existing.id,
            status: 'failed',
            error: error instanceof Error ? error.message : String(error),
          })
        }
        throw error
      }
    },
    wait: (timeoutMs, target) => waitAgent(timeoutMs, target),
    interrupt: (target) => multiAgents.interrupt(getRuntimeSession().sessionId, target),
    runScript,
    updateTask: async (target, input = {}) => {
      const sessionId = getRuntimeSession().sessionId
      if (!teamWorkflows.isActive(sessionId)) throw new Error('No active Team is available.')
      const task = teamWorkflows.findTask(sessionId, target)
      if (!task) throw new Error(`Unknown Team task: ${target}`)
      const updated = await teamWorkflows.updateTask(sessionId, task.id, input)
      await schedulePendingTeamSpawns(sessionId)
      emitAgentUpdate(sessionId, { id: '', status: updated.status })
      return updated
    },
  }
}
