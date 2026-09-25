// 对外 MCP 工具目录。只调用 Runtime 已有领域入口；会话内的命令、文件和浏览器
// 操作仍由 Pisper 自己的工具权限与审批流程处理。
import { randomUUID } from 'node:crypto'
import { z } from 'zod'
import { redactSecretText, redactSecretValue } from '../security/secret-redaction.mjs'

const MAX_RESULT_BYTES = 96 * 1024
const MAX_ACTIVE_RUNS = 4
const MAX_RUNS = 100
const MAX_RUN_AGE_MS = 30 * 60_000
const sessionId = z.string().min(1).max(160)
const runId = z.string().min(1).max(160)
const identifier = z.string().min(1).max(160)

function wellFormedJson(value) {
  if (typeof value === 'string') return value.toWellFormed()
  if (Array.isArray(value)) return value.map(wellFormedJson)
  if (value && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value).map(([key, child]) => [key.toWellFormed(), wellFormedJson(child)]),
    )
  return value
}

function result(value) {
  let json
  try {
    // 先转为普通 JSON 值（保留 Date 的 ISO 表示），再逐字段脱敏；对序列化后的
    // JSON 字符串运行正则会吞掉引号或括号，使 MCP 客户端拿到无效 JSON。
    // MCP 也可能由严格的 Rust 客户端读取，孤立代理项必须归一化后再发送。
    json = JSON.stringify(
      redactSecretValue(wellFormedJson(JSON.parse(JSON.stringify(value ?? null)))),
    )
  } catch {
    return {
      content: [{ type: 'text', text: '{"error":"result_not_serializable"}' }],
      isError: true,
    }
  }
  if (Buffer.byteLength(json, 'utf8') > MAX_RESULT_BYTES) {
    return {
      content: [
        {
          type: 'text',
          text: JSON.stringify({
            error: 'result_too_large',
            message: '结果超过大小限制。请缩小查询范围或减少返回条数。',
          }),
        },
      ],
      isError: true,
    }
  }
  return { content: [{ type: 'text', text: json }] }
}

function failure(error) {
  const message = redactSecretText(error instanceof Error ? error.message : String(error))
  const code = error?.code === 'not_found' ? 'not_found' : 'tool_failed'
  return {
    content: [
      {
        type: 'text',
        text: JSON.stringify({ error: code, message: message.slice(0, 500) }),
      },
    ],
    isError: true,
  }
}

function requireFound(value, kind) {
  if (value != null) return value
  throw Object.assign(new Error(`${kind}不存在。`), { code: 'not_found' })
}

function pruneRuns(runs) {
  const cutoff = Date.now() - MAX_RUN_AGE_MS
  for (const [id, run] of runs) {
    if (run.status !== 'running' && run.endedAtMs < cutoff) runs.delete(id)
  }
  while (runs.size > MAX_RUNS) {
    const oldest = [...runs].find(([, run]) => run.status !== 'running')
    if (!oldest) break
    runs.delete(oldest[0])
  }
}

function publicRun(run) {
  return {
    id: run.id,
    sessionId: run.sessionId,
    status: run.status,
    startedAt: run.startedAt,
    endedAt: run.endedAt || null,
    text: run.text.slice(0, 32_000),
    error: run.error || null,
    needsApproval: run.needsApproval,
  }
}

function visibleSession(message) {
  return {
    id: message?.id,
    role: message?.role,
    text: typeof message?.text === 'string' ? message.text.slice(0, 12_000) : '',
    timestamp: message?.timestamp,
  }
}

function register(server, name, config, handler, context) {
  server.registerTool(name, config, async (input) => {
    try {
      if (!context.isEnabled()) throw new Error('Pisper MCP 服务已关闭。')
      const runtime = await context.getRuntime()
      if (!runtime) throw new Error('Pisper Runtime 尚未就绪。')
      return result(await handler(runtime, input, context))
    } catch (error) {
      return failure(error)
    }
  })
}

/**
 * @param {import('@modelcontextprotocol/sdk/server/mcp.js').McpServer} server
 * @param {{getRuntime:()=>unknown, isEnabled:()=>boolean, runs:Map<string, any>}} context
 */
export function registerMcpHostTools(server, context) {
  const readOnly = { readOnlyHint: true }
  const changesState = { readOnlyHint: false }

  register(
    server,
    'pisper_capabilities',
    {
      description: 'List the Pisper Runtime capabilities available on this device.',
      annotations: readOnly,
    },
    async (runtime) => runtime.capabilities,
    context,
  )
  register(
    server,
    'pisper_list_sessions',
    {
      description: 'List Pisper conversations, optionally filtering by title. Returns at most 100.',
      inputSchema: z.object({
        query: z.string().max(160).optional(),
        limit: z.number().int().min(1).max(100).optional(),
      }),
      annotations: readOnly,
    },
    async (runtime, { query = '', limit = 30 }) => {
      const sessions = await runtime.listSessions()
      const matched = query
        ? sessions.filter((session) =>
            String(session.name || '')
              .toLowerCase()
              .includes(query.toLowerCase()),
          )
        : sessions
      return { sessions: matched.slice(0, limit), total: matched.length }
    },
    context,
  )
  register(
    server,
    'pisper_get_session_messages',
    {
      description: 'Read one page of conversation messages. Use nextCursor to page backwards.',
      inputSchema: z.object({
        sessionId,
        before: z.string().max(30).optional(),
        limit: z.number().int().min(1).max(20).optional(),
      }),
      annotations: readOnly,
    },
    async (runtime, { sessionId: id, before, limit = 10 }) => {
      const page = await runtime.getSessionMessagePage(id, { before, limit })
      return {
        messages: page.messages.map(visibleSession),
        pageInfo: page.pageInfo,
        model: page.model,
      }
    },
    context,
  )
  register(
    server,
    'pisper_get_session_live',
    {
      description:
        'Get live status of a conversation, including whether it is streaming or waiting for user approval.',
      inputSchema: z.object({ sessionId }),
      annotations: readOnly,
    },
    async (runtime, { sessionId: id }) => {
      const live = await runtime.getSessionLive(id)
      return {
        sessionId: id,
        streaming: Boolean(live.streaming),
        approvalsPending: Array.isArray(live.approvals) ? live.approvals.length : 0,
        messages: Array.isArray(live.messages) ? live.messages.slice(-8).map(visibleSession) : [],
      }
    },
    context,
  )
  register(
    server,
    'pisper_create_session',
    {
      description:
        'Create a Pisper conversation. New conversations retain Pisper’s default approval-required mode.',
      inputSchema: z.object({
        name: z.string().min(1).max(160).optional(),
        cwd: z.string().min(1).max(2_000).optional(),
      }),
      annotations: changesState,
    },
    async (runtime, { name, cwd }) => runtime.createSession(name, cwd),
    context,
  )
  register(
    server,
    'pisper_rename_session',
    {
      description: 'Rename an existing Pisper conversation.',
      inputSchema: z.object({ sessionId, name: z.string().min(1).max(160) }),
      annotations: changesState,
    },
    async (runtime, { sessionId: id, name }) =>
      requireFound(await runtime.renameSession(id, name, { manual: true }), '会话'),
    context,
  )
  register(
    server,
    'pisper_send_message',
    {
      description:
        'Start a Pisper agent turn. Returns a run ID immediately; poll pisper_get_run. Pisper session permissions and approvals still apply.',
      inputSchema: z.object({
        sessionId: sessionId.optional(),
        message: z.string().min(1).max(12_000),
        name: z.string().min(1).max(160).optional(),
        cwd: z.string().min(1).max(2_000).optional(),
        goalMode: z.boolean().optional(),
        teamMode: z.boolean().optional(),
      }),
      annotations: changesState,
    },
    async (runtime, input, { runs, isEnabled }) => {
      pruneRuns(runs)
      if ([...runs.values()].filter((run) => run.status === 'running').length >= MAX_ACTIVE_RUNS)
        throw new Error('外部会话运行数量已达到上限，请等待已有任务完成。')
      let id = input.sessionId
      if (id && !(await runtime.hasSession(id))) throw new Error('会话不存在。')
      if (!id) id = (await runtime.createSession(input.name || 'MCP 会话', input.cwd)).id
      const run = {
        id: `mcp_run_${randomUUID()}`,
        sessionId: id,
        status: 'running',
        text: '',
        error: '',
        needsApproval: false,
        startedAt: new Date().toISOString(),
        endedAt: '',
        endedAtMs: 0,
      }
      runs.set(run.id, run)
      void Promise.resolve()
        .then(async () => {
          if (!isEnabled()) throw new Error('Pisper MCP 服务已关闭。')
          const output = await runtime.promptFromChannel({
            sessionId: id,
            message: input.message,
            goalMode: Boolean(input.goalMode || input.teamMode),
            teamMode: Boolean(input.teamMode),
            onEvent: (event, data) => {
              if (event === 'permission_request') run.needsApproval = true
              if (event === 'permission_resolved') run.needsApproval = false
              if (event === 'text_delta' && typeof data?.delta === 'string')
                run.text = (run.text + data.delta).slice(-32_000)
              // streamPrompt 会发送 error 终态并正常返回，不能把该轮误报为成功。
              if (event === 'error') {
                run.status = 'failed'
                run.error = redactSecretText(String(data?.message || '会话运行失败。')).slice(
                  0,
                  500,
                )
              }
            },
          })
          if (run.status === 'running') {
            run.text = String(output.text || run.text).slice(0, 32_000)
            run.status = 'completed'
          }
        })
        .catch((error) => {
          run.status = 'failed'
          run.error = redactSecretText(
            error instanceof Error ? error.message : String(error),
          ).slice(0, 500)
        })
        .finally(() => {
          run.endedAt = new Date().toISOString()
          run.endedAtMs = Date.now()
          pruneRuns(runs)
        })
      return publicRun(run)
    },
    context,
  )
  register(
    server,
    'pisper_get_run',
    {
      description: 'Poll a Pisper turn started by pisper_send_message.',
      inputSchema: z.object({ runId }),
      annotations: readOnly,
    },
    async (_runtime, { runId: id }, { runs }) => {
      pruneRuns(runs)
      const run = runs.get(id)
      if (!run) throw new Error('运行不存在或结果已过期。')
      return publicRun(run)
    },
    context,
  )
  register(
    server,
    'pisper_abort_session',
    {
      description: 'Stop an active Pisper conversation turn.',
      inputSchema: z.object({ sessionId }),
      annotations: { readOnlyHint: false, destructiveHint: true },
    },
    async (runtime, { sessionId: id }) => ({ aborted: await runtime.abortSession(id) }),
    context,
  )
  register(
    server,
    'pisper_get_goal',
    {
      description: 'Read goal state for a Pisper conversation.',
      inputSchema: z.object({ sessionId }),
      annotations: readOnly,
    },
    async (runtime, { sessionId: id }) => runtime.getSessionGoal(id),
    context,
  )
  register(
    server,
    'pisper_pause_goal',
    {
      description: 'Pause an active Pisper goal.',
      inputSchema: z.object({ sessionId }),
      annotations: changesState,
    },
    async (runtime, { sessionId: id }) => runtime.pauseSessionGoal(id),
    context,
  )
  register(
    server,
    'pisper_search_memory',
    {
      description: 'Search Pisper long-term memory for relevant user or project context.',
      inputSchema: z.object({
        query: z.string().min(1).max(500),
        limit: z.number().int().min(1).max(12).optional(),
      }),
      annotations: readOnly,
    },
    async (runtime, { query, limit = 6 }) => ({
      memories: await runtime.searchMemory(query, limit),
    }),
    context,
  )
  register(
    server,
    'pisper_list_workflows',
    { description: 'List available Pisper workflows.', annotations: readOnly },
    async (runtime) => {
      const value = await runtime.getWorkflows()
      return { workflows: (value.workflows || []).slice(0, 100) }
    },
    context,
  )
  register(
    server,
    'pisper_run_workflow',
    {
      description: 'Start a published Pisper workflow using its existing approval rules.',
      inputSchema: z.object({
        workflowId: identifier,
        inputs: z.record(z.string(), z.unknown()).optional(),
      }),
      annotations: changesState,
    },
    async (runtime, { workflowId, inputs = {} }) =>
      requireFound(await runtime.runWorkflow(workflowId, { trigger: 'mcp', inputs }), '工作流'),
    context,
  )
  register(
    server,
    'pisper_get_workflow_run',
    {
      description: 'Read a Pisper workflow run.',
      inputSchema: z.object({ runId }),
      annotations: readOnly,
    },
    async (runtime, { runId: id }) => requireFound(await runtime.getWorkflowRun(id), '工作流运行'),
    context,
  )
  register(
    server,
    'pisper_stop_workflow_run',
    {
      description: 'Stop a Pisper workflow run.',
      inputSchema: z.object({ runId }),
      annotations: { readOnlyHint: false, destructiveHint: true },
    },
    async (runtime, { runId: id }) => requireFound(await runtime.stopWorkflowRun(id), '工作流运行'),
    context,
  )
  register(
    server,
    'pisper_list_schedules',
    { description: 'List Pisper scheduled tasks.', annotations: readOnly },
    async (runtime) => {
      const value = await runtime.getSchedules()
      return { tasks: (value.tasks || []).slice(0, 100) }
    },
    context,
  )
  register(
    server,
    'pisper_run_schedule',
    {
      description: 'Run an existing Pisper scheduled task now.',
      inputSchema: z.object({ scheduleId: identifier }),
      annotations: changesState,
    },
    async (runtime, { scheduleId }) =>
      requireFound(await runtime.runSchedule(scheduleId), '定时任务'),
    context,
  )
  register(
    server,
    'pisper_get_usage_today',
    { description: 'Read today’s Pisper token usage.', annotations: readOnly },
    async (runtime) => runtime.getTodayUsage(),
    context,
  )
  register(
    server,
    'pisper_list_assets',
    {
      description: 'Search Pisper assets by name or session.',
      inputSchema: z.object({
        query: z.string().max(160).optional(),
        sessionId: sessionId.optional(),
      }),
      annotations: readOnly,
    },
    async (runtime, { query = '', sessionId: id = '' }) => ({
      assets: (await runtime.listAssets({ query, sessionId: id })).slice(0, 100),
    }),
    context,
  )
  register(
    server,
    'pisper_get_session_changes',
    {
      description: 'Read tracked file changes for a Pisper conversation.',
      inputSchema: z.object({ sessionId }),
      annotations: readOnly,
    },
    async (runtime, { sessionId: id }) => runtime.getSessionFileChanges(id),
    context,
  )
}

export async function abortMcpHostRuns(runs, getRuntime) {
  const active = [...runs.values()].filter((run) => run.status === 'running')
  if (!active.length) return
  const runtime = await Promise.resolve()
    .then(getRuntime)
    .catch(() => null)
  await Promise.allSettled(active.map((run) => runtime?.abortSession(run.sessionId)))
}
