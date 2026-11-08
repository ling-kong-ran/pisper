// 多 Agent 工具：spawn/list/send/followup/wait/interrupt 六件套，
// 是父会话调度子 Agent 的接口（内部运行时工具，不进插件目录）。
import { DEFAULT_MAX_BYTES, defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { agentProfileSummary } from '../../services/agent-profiles.mjs'
import {
  AGENT_SUMMARY_TRUNCATED,
  AGENT_TARGET,
  FOLLOWUP_TASK,
  INTERRUPT_AGENT,
  LIST_AGENTS,
  LIST_TEAM_MEMBERS,
  RUN_TEAM_WORKFLOW,
  SEND_MESSAGE,
  SEND_TEAM_MESSAGE,
  SPAWN_AGENT,
  UPDATE_TEAM_TASK,
  WAIT_AGENT,
  interruptResultText,
  spawnAgentDescription,
  spawnResultText,
} from '../../prompts/tools/multi-agent.mjs'
const category = 'collaboration'
const source = 'app'

export const TEAM_MEMBER_TOOL_NAMES = Object.freeze(['send_team_message', 'list_team_members'])

export const manifests = [
  {
    id: 'spawn_agent',
    name: 'Spawn Agent',
    category,
    risk: 'medium',
    scope: 'Current chat workspace and permission boundary',
    capability: 'Start or queue an Agent asynchronously without blocking the parent',
    source,
  },
  {
    id: 'list_agents',
    name: 'List Agents',
    category,
    risk: 'low',
    scope: 'Current chat',
    capability: 'List status, duration, tools, and results',
    source,
  },
  {
    id: 'send_message',
    name: 'Send Message',
    category,
    risk: 'low',
    scope: 'Agents in the current chat',
    capability: 'Send a message without starting a new task by itself',
    source,
  },
  {
    id: 'followup_task',
    name: 'Follow-up Task',
    category,
    risk: 'medium',
    scope: 'Agents in the current chat',
    capability: 'Reuse Agent context for follow-up work',
    source,
  },
  {
    id: 'wait_agent',
    name: 'Wait Agent',
    category,
    risk: 'low',
    scope: 'Current chat',
    capability: 'Briefly wait for a terminal result without owning total task timeout',
    source,
  },
  {
    id: 'interrupt_agent',
    name: 'Interrupt Agent',
    category,
    risk: 'medium',
    scope: 'Agents in the current chat',
    capability: 'Stop the Agent current run',
    source,
  },
  {
    id: 'update_team_task',
    name: 'Update Team Task',
    category,
    risk: 'medium',
    scope: 'Team task graph in the current chat',
    capability: 'Adjust task dependencies, ownership, role label, or deliverable',
    source,
  },
  {
    id: 'run_team_workflow',
    name: 'Run Team Workflow',
    category,
    risk: 'medium',
    scope: 'Current Team and workspace-relative JavaScript script',
    capability: 'Run dynamic fan-out, branching, aggregation, and verification through Team agents',
    source,
  },
]

function text(value) {
  return { content: [{ type: 'text', text: value }] }
}

function requireRuntime(runtime, method) {
  if (typeof runtime?.[method] !== 'function')
    throw new Error('Multi-agent runtime is not available.')
  return runtime[method]
}

function utf8Prefix(value, maxBytes) {
  const buffer = Buffer.from(value, 'utf8')
  if (buffer.length <= maxBytes) return value
  let end = maxBytes
  while (end > 0 && (buffer[end] & 0xc0) === 0x80) end -= 1
  return buffer.subarray(0, end).toString('utf8')
}

function boundedToolText(value) {
  const textValue = String(value || '')
  if (Buffer.byteLength(textValue, 'utf8') <= DEFAULT_MAX_BYTES) return textValue
  const suffix = AGENT_SUMMARY_TRUNCATED
  return `${utf8Prefix(textValue, DEFAULT_MAX_BYTES - Buffer.byteLength(suffix, 'utf8'))}${suffix}`
}

function compactAgent(agent) {
  const elapsed = Number.isFinite(agent.durationMs)
    ? `, ${(agent.durationMs / 1000).toFixed(1)}s`
    : ''
  // 活跃状态的 Agent 附带距上次活动的空闲时长，让 lead 能区分「在干活」与「已卡死」。
  let idle = ''
  if (['queued', 'starting', 'running'].includes(agent.status) && agent.lastActivityAt) {
    const idleMs = Date.now() - new Date(agent.lastActivityAt).getTime()
    if (Number.isFinite(idleMs) && idleMs >= 30_000) idle = `, idle ${(idleMs / 1000).toFixed(0)}s`
  }
  const output = agent.output ? `\n${agent.output}` : agent.error ? `\nError: ${agent.error}` : ''
  return `${agent.canonicalName} · ${agent.status}${elapsed}${idle}${output}`
}

function compactAgents(agents) {
  return boundedToolText(agents.map(compactAgent).join('\n\n'))
}

function agentDetails(agent) {
  const { fullOutput: _fullOutput, ...details } = agent
  return details
}

function createSpawnAgentTool({ multiAgentRuntime, agentProfiles = [], tierModels = {} }) {
  return defineTool({
    name: 'spawn_agent',
    label: 'Spawn Agent',
    // 定义列表在会话创建时写进工具说明（父模型据此挑选）；之后新增的定义按名字调用仍然有效。
    description: spawnAgentDescription({
      definitions: agentProfiles.map((profile) => agentProfileSummary(profile, profile.selection)),
      tierModels,
    }),
    // 子 Agent 的规则都在这一份里，list_agents、wait_agent 不再各写一遍。
    promptGuidelines: SPAWN_AGENT.guidelines,
    parameters: Type.Object({
      taskName: Type.String({
        minLength: 1,
        maxLength: 48,
        description: SPAWN_AGENT.params.taskName,
      }),
      message: Type.String({
        minLength: 1,
        maxLength: 12_000,
        description: SPAWN_AGENT.params.message,
      }),
      role: Type.Optional(Type.String({ maxLength: 80, description: SPAWN_AGENT.params.role })),
      agent: Type.Optional(Type.String({ maxLength: 48, description: SPAWN_AGENT.params.agent })),
      tier: Type.Optional(
        Type.String({
          enum: ['fast', 'standard', 'strong'],
          description: SPAWN_AGENT.params.tier,
        }),
      ),
      files: Type.Optional(
        Type.Array(Type.String({ maxLength: 240 }), {
          maxItems: 96,
          description: SPAWN_AGENT.params.files,
        }),
      ),
      dependsOn: Type.Optional(
        Type.Array(Type.String({ maxLength: 48 }), {
          maxItems: 32,
          description: SPAWN_AGENT.params.dependsOn,
        }),
      ),
    }),
    async execute(_toolCallId, params) {
      const agent = await requireRuntime(multiAgentRuntime, 'spawn')(params)
      return { ...text(spawnResultText(agent)), details: agent }
    },
  })
}

function createListAgentsTool({ multiAgentRuntime }) {
  return defineTool({
    name: 'list_agents',
    label: 'List Agents',
    description: LIST_AGENTS.description,
    parameters: Type.Object({}),
    async execute() {
      const agents = await requireRuntime(multiAgentRuntime, 'list')()
      return {
        ...text(
          agents.length ? compactAgents(agents) : 'No Agents have been created in this session.',
        ),
        details: { agents: agents.map(agentDetails) },
      }
    },
  })
}

function createSendMessageTool({ multiAgentRuntime }) {
  return defineTool({
    name: 'send_message',
    label: 'Send Message',
    description: SEND_MESSAGE.description,
    parameters: Type.Object({
      target: Type.String({ minLength: 1, description: AGENT_TARGET.target }),
      message: Type.String({ minLength: 1, maxLength: 12_000 }),
    }),
    async execute(_toolCallId, params) {
      const agent = await requireRuntime(multiAgentRuntime, 'sendMessage')(
        params.target,
        params.message,
      )
      return { ...text(`Message queued for ${agent.canonicalName}.`), details: agent }
    },
  })
}

function createFollowupTaskTool({ multiAgentRuntime }) {
  return defineTool({
    name: 'followup_task',
    label: 'Follow-up Task',
    description: FOLLOWUP_TASK.description,
    parameters: Type.Object({
      target: Type.String({ minLength: 1, description: AGENT_TARGET.target }),
      message: Type.String({ minLength: 1, maxLength: 12_000 }),
    }),
    async execute(_toolCallId, params) {
      const agent = await requireRuntime(multiAgentRuntime, 'followup')(
        params.target,
        params.message,
      )
      return { ...text(`Follow-up queued for ${agent.canonicalName}.`), details: agent }
    },
  })
}

function createWaitAgentTool({ multiAgentRuntime }) {
  return defineTool({
    name: 'wait_agent',
    label: 'Wait Agent',
    description: WAIT_AGENT.description,
    parameters: Type.Object({
      target: Type.Optional(
        Type.String({ minLength: 1, description: AGENT_TARGET.optionalTarget }),
      ),
      timeoutMs: Type.Optional(
        Type.Integer({ minimum: 250, maximum: 30_000, description: WAIT_AGENT.params.timeoutMs }),
      ),
    }),
    async execute(_toolCallId, params) {
      const result = await requireRuntime(multiAgentRuntime, 'wait')(
        params.timeoutMs,
        params.target,
      )
      const summary = result.agents.length
        ? compactAgents(result.agents)
        : 'No Agents have been created in this session.'
      return {
        ...text(result.timedOut ? `No Agent completed before timeout.\n\n${summary}` : summary),
        details: { ...result, agents: result.agents.map(agentDetails) },
      }
    },
  })
}

function createInterruptAgentTool({ multiAgentRuntime }) {
  return defineTool({
    name: 'interrupt_agent',
    label: 'Interrupt Agent',
    description: INTERRUPT_AGENT.description,
    parameters: Type.Object({
      target: Type.String({ minLength: 1, description: AGENT_TARGET.target }),
    }),
    async execute(_toolCallId, params) {
      const agent = await requireRuntime(multiAgentRuntime, 'interrupt')(params.target)
      // 中断可能恰好落在多文件编辑中间，提醒调用方先验证工作区再决定接管方式。
      return { ...text(interruptResultText(agent)), details: agent }
    },
  })
}

function createUpdateTeamTaskTool({ multiAgentRuntime }) {
  return defineTool({
    name: 'update_team_task',
    label: 'Update Team Task',
    description: UPDATE_TEAM_TASK.description,
    promptSnippet: UPDATE_TEAM_TASK.snippet,
    promptGuidelines: UPDATE_TEAM_TASK.guidelines,
    parameters: Type.Object({
      target: Type.String({ minLength: 1, description: UPDATE_TEAM_TASK.params.target }),
      taskName: Type.Optional(Type.String({ maxLength: 48 })),
      role: Type.Optional(Type.String({ maxLength: 80 })),
      agent: Type.Optional(Type.String({ maxLength: 48 })),
      message: Type.Optional(Type.String({ maxLength: 12_000 })),
      files: Type.Optional(Type.Array(Type.String({ maxLength: 240 }), { maxItems: 96 })),
      dependsOn: Type.Optional(Type.Array(Type.String({ maxLength: 48 }), { maxItems: 32 })),
    }),
    async execute(_toolCallId, params) {
      const task = await requireRuntime(multiAgentRuntime, 'updateTask')(params.target, params)
      return { ...text(JSON.stringify(task, null, 2)), details: task }
    },
  })
}

function createRunTeamWorkflowTool({ multiAgentRuntime }) {
  return defineTool({
    name: 'run_team_workflow',
    label: 'Run Team Workflow',
    description: RUN_TEAM_WORKFLOW.description,
    promptSnippet: RUN_TEAM_WORKFLOW.snippet,
    promptGuidelines: RUN_TEAM_WORKFLOW.guidelines,
    parameters: Type.Object({
      path: Type.String({
        minLength: 1,
        maxLength: 240,
        description: RUN_TEAM_WORKFLOW.params.path,
      }),
      args: Type.Optional(
        Type.Record(Type.String(), Type.Unknown(), {
          description: RUN_TEAM_WORKFLOW.params.args,
        }),
      ),
    }),
    async execute(_toolCallId, params) {
      const result = await requireRuntime(multiAgentRuntime, 'runScript')(params.path, params.args)
      const summary = {
        scriptPath: result.scriptPath,
        meta: result.meta,
        logs: result.logs,
        result: result.result,
        taskCount: result.taskCount,
      }
      return { ...text(JSON.stringify(summary, null, 2)), details: result }
    },
  })
}

function createTeamMemberSendMessageTool({ teamMemberRuntime }) {
  return defineTool({
    name: 'send_team_message',
    label: 'Send Team Message',
    description: SEND_TEAM_MESSAGE.description,
    parameters: Type.Object({
      target: Type.String({
        minLength: 1,
        description: SEND_TEAM_MESSAGE.params.target,
      }),
      message: Type.String({ minLength: 1, maxLength: 12_000 }),
    }),
    async execute(_toolCallId, params) {
      const result = await teamMemberRuntime.sendMessage(params.target, params.message)
      return {
        ...text(`Message delivered to ${result.agent.canonicalName || result.agent.taskName}.`),
        details: result,
      }
    },
  })
}

function createTeamMemberListTool({ teamMemberRuntime }) {
  return defineTool({
    name: 'list_team_members',
    label: 'List Team Members',
    description: LIST_TEAM_MEMBERS.description,
    parameters: Type.Object({}),
    async execute() {
      const members = await teamMemberRuntime.listMembers()
      return { ...text(JSON.stringify(members, null, 2)), details: { members } }
    },
  })
}

export function createTeamMemberTools({ teamMemberRuntime } = {}) {
  if (!teamMemberRuntime) return []
  return [
    createTeamMemberSendMessageTool({ teamMemberRuntime }),
    createTeamMemberListTool({ teamMemberRuntime }),
  ]
}

export const factories = {
  spawn_agent: createSpawnAgentTool,
  list_agents: createListAgentsTool,
  send_message: createSendMessageTool,
  followup_task: createFollowupTaskTool,
  wait_agent: createWaitAgentTool,
  interrupt_agent: createInterruptAgentTool,
  update_team_task: createUpdateTeamTaskTool,
  run_team_workflow: createRunTeamWorkflowTool,
}

// Internal runtime tools: always available to the primary Agent, never shown in the plugins UI.
export function createMultiAgentTools(context = {}) {
  return manifests.map((manifest) => factories[manifest.id](context))
}
