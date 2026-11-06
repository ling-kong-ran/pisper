// 多 Agent 工具：spawn/list/send/followup/wait/interrupt 六件套，
// 是父会话调度子 Agent 的接口（内部运行时工具，不进插件目录）。
import { DEFAULT_MAX_BYTES, defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { agentProfileSummary } from '../../services/agent-profiles.mjs'
const category = 'collaboration'
const source = 'app'
const AGENT_TARGET = 'Agent id, task name or canonical name'

export const TEAM_MEMBER_TOOL_NAMES = Object.freeze(['send_team_message', 'list_team_members'])

export const manifests = [
  {
    id: 'spawn_agent',
    name: 'Spawn Agent',
    category,
    risk: 'medium',
    description: 'Start a bounded subtask with an isolated context.',
    scope: 'Current chat workspace and permission boundary',
    capability: 'Start or queue an Agent asynchronously without blocking the parent',
    source,
  },
  {
    id: 'list_agents',
    name: 'List Agents',
    category,
    risk: 'low',
    description: 'Inspect Agents created by the current chat and their run status.',
    scope: 'Current chat',
    capability: 'List status, duration, tools, and results',
    source,
  },
  {
    id: 'send_message',
    name: 'Send Message',
    category,
    risk: 'low',
    description: 'Send additional information to a running Agent.',
    scope: 'Agents in the current chat',
    capability: 'Send a message without starting a new task by itself',
    source,
  },
  {
    id: 'followup_task',
    name: 'Follow-up Task',
    category,
    risk: 'medium',
    description: 'Give an existing Agent another task while preserving its context.',
    scope: 'Agents in the current chat',
    capability: 'Reuse Agent context for follow-up work',
    source,
  },
  {
    id: 'wait_agent',
    name: 'Wait Agent',
    category,
    risk: 'low',
    description: 'Wait for an Agent to complete, fail, or be interrupted.',
    scope: 'Current chat',
    capability: 'Briefly wait for a terminal result without owning total task timeout',
    source,
  },
  {
    id: 'interrupt_agent',
    name: 'Interrupt Agent',
    category,
    risk: 'medium',
    description: 'Interrupt a running Agent.',
    scope: 'Agents in the current chat',
    capability: 'Stop the Agent current run',
    source,
  },
  {
    id: 'update_team_task',
    name: 'Update Team Task',
    category,
    risk: 'medium',
    description: 'Change a queued Team task after inspecting new evidence.',
    scope: 'Team task graph in the current chat',
    capability: 'Adjust task dependencies, ownership, role label, or deliverable',
    source,
  },
  {
    id: 'run_team_workflow',
    name: 'Run Team Workflow',
    category,
    risk: 'medium',
    description: 'Execute a restricted JavaScript Team workflow from the workspace.',
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
  const suffix = '\n\n[Agent summary truncated to the Pisper tool-output limit.]'
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

// 定义列表在会话创建时写进工具说明（父模型据此挑选）；之后新增的定义按名字调用仍然有效。
function agentDefinitionsText(agentProfiles) {
  if (!agentProfiles.length) return ''
  return `\n\nAgent definitions available in this workspace (pass the name as agent):\n${agentProfiles
    .map((profile) => agentProfileSummary(profile, profile.selection))
    .join('\n')}`
}

// 档位此刻对应的模型；一个都没设置时全部跟随当前会话模型，不必写。
function tierModelsText(tierModels = {}) {
  if (!['fast', 'standard', 'strong'].some((tier) => tierModels[tier])) return ''
  return `\n\nModel tiers (pass as tier when no agent definition fits): ${[
    'fast',
    'standard',
    'strong',
  ]
    .map((tier) => `${tier} → ${tierModels[tier] || 'this session model'}`)
    .join('; ')}.`
}

function spawnResultText(agent) {
  const action = agent.status === 'queued' ? 'Queued' : 'Started'
  const definition = agent.agentProfile
    ? ` as ${agent.agentProfile} on ${agent.model || 'the parent model'}`
    : agent.modelTier
      ? ` on the ${agent.modelTier} tier (${agent.model || 'the parent model'})`
      : ''
  const fallback = agent.modelFallbackReasons?.length
    ? ` The requested model was unavailable (${agent.modelFallbackReasons.join(', ')}), so it ${
        agent.modelSource === 'tier' ? 'uses its tier model' : 'inherits the parent model'
      }.`
    : ''
  const skipped = agent.skippedProfileTools?.length
    ? ` Tools listed in the definition were skipped because this session does not have them: ${agent.skippedProfileTools.join(', ')}.`
    : ''
  return `${action} ${agent.canonicalName}${definition} in the background.${fallback}${skipped}`
}

function createSpawnAgentTool({ multiAgentRuntime, agentProfiles = [], tierModels = {} }) {
  return defineTool({
    name: 'spawn_agent',
    label: 'Spawn Agent',
    description: `Start an isolated background Agent on a concrete, bounded subtask.${agentDefinitionsText(agentProfiles)}${tierModelsText(tierModels)}`,
    // 子 Agent 的规则都在这一份里，list_agents、wait_agent 不再各写一遍。
    promptGuidelines: [
      'For substantial work, check once whether independent workstreams exist and use spawn_agent proactively when delegation shortens the critical path, even if the user did not explicitly request subagents: codebase investigation, platform-specific checks, test diagnosis, review of a bounded area.',
      'Do not spawn for trivial work, a single linear task, tightly coupled changes in the same files, or when coordination costs more than doing it directly. Keep the immediate critical-path step in the parent session and keep working after delegating.',
      "Agents never inherit the transcript: give a self-contained message with every constraint and piece of context. They use this session's permission mode and workspace and cannot spawn Agents.",
      "Pass agent when a definition fits; it sets model, reasoning, tools and standing instructions, so do not send file-changing work to one without write tools. Otherwise pass tier: fast for lookups and summaries, standard for ordinary edits, strong for design, hard debugging and review. Without either, the Agent inherits this session's model, reasoning and tools.",
      "Agents run in the background and must not delay replying to the user, handling later instructions, or spawning other independent Agents. After spawning, say briefly in the user's language that the work was delegated, without internals.",
      'Do not call list_agents or wait_agent merely to monitor progress: results are automatically delivered to the parent session. Use list_agents only when asked for status and wait_agent only when the user asks to wait for a result; never repeat wait_agent after a timeout.',
    ],
    parameters: Type.Object({
      taskName: Type.String({
        minLength: 1,
        maxLength: 48,
        description: 'Short stable name (letters, digits, - or _)',
      }),
      message: Type.String({
        minLength: 1,
        maxLength: 12_000,
        description: 'Self-contained task',
      }),
      role: Type.Optional(
        Type.String({ maxLength: 80, description: 'Team role, e.g. investigator or tester' }),
      ),
      agent: Type.Optional(
        Type.String({ maxLength: 48, description: 'Agent definition name (.pisper/agents/*.md)' }),
      ),
      tier: Type.Optional(
        Type.String({
          enum: ['fast', 'standard', 'strong'],
          description: 'Ignored if the definition names a model or tier',
        }),
      ),
      files: Type.Optional(
        Type.Array(Type.String({ maxLength: 240 }), {
          maxItems: 96,
          description: 'Paths this task owns while active (Team)',
        }),
      ),
      dependsOn: Type.Optional(
        Type.Array(Type.String({ maxLength: 48 }), {
          maxItems: 32,
          description: 'Task names that must finish first (Team)',
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
    description: "List this session's Agents and their status.",
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
    description: 'Send information to a running Agent.',
    parameters: Type.Object({
      target: Type.String({ minLength: 1, description: AGENT_TARGET }),
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
    description: 'Give an existing Agent another task, keeping its context.',
    parameters: Type.Object({
      target: Type.String({ minLength: 1, description: AGENT_TARGET }),
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
    description: 'Wait briefly for an Agent to finish.',
    parameters: Type.Object({
      target: Type.Optional(
        Type.String({ minLength: 1, description: `${AGENT_TARGET}; omit for any active Agent` }),
      ),
      timeoutMs: Type.Optional(
        Type.Integer({ minimum: 250, maximum: 30_000, description: 'Default 15000' }),
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
    description: "Stop an Agent's current run, keeping its record.",
    parameters: Type.Object({
      target: Type.String({ minLength: 1, description: AGENT_TARGET }),
    }),
    async execute(_toolCallId, params) {
      const agent = await requireRuntime(multiAgentRuntime, 'interrupt')(params.target)
      // 中断可能恰好落在多文件编辑中间，提醒调用方先验证工作区再决定接管方式。
      const caution =
        agent.status === 'interrupted'
          ? ' Its workspace may hold partial multi-file edits; verify compilation and tests before continuing, and finish leftover work by respawning instead of messaging this Agent.'
          : ''
      return { ...text(`${agent.canonicalName} is ${agent.status}.${caution}`), details: agent }
    },
  })
}

function createUpdateTeamTaskTool({ multiAgentRuntime }) {
  return defineTool({
    name: 'update_team_task',
    label: 'Update Team Task',
    description:
      'Update a queued Team task when new evidence changes its scope, dependencies, role, or deliverable.',
    promptSnippet: 'Adjust the Team task graph when new evidence changes the work plan',
    promptGuidelines: [
      'Update only queued, blocked, failed, or interrupted tasks; interrupt a running task before changing its graph entry.',
      'Keep dependencies explicit and avoid cycles or overlapping active file ownership.',
    ],
    parameters: Type.Object({
      target: Type.String({ minLength: 1, description: 'Team task id or task name.' }),
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
    description:
      'Execute a workspace JavaScript workflow that orchestrates Team agents through a restricted API.',
    promptSnippet: 'Write and execute a dynamic JavaScript Team workflow script',
    promptGuidelines: [
      'Use a workspace-relative .js file beginning with export const meta = { name, description }.',
      'The script may use agent(prompt, options), pipeline(items, worker), parallel(tasks), phase(title), log(message), and args; it must not import modules or access files directly.',
      'Begin with export const meta = { name, description }, then use top-level await and return the final serializable result.',
      'Use agent results to branch, fan out, aggregate, verify, and converge; options may include label, role, agent (an agent definition name), files, dependsOn, and schema.',
    ],
    parameters: Type.Object({
      path: Type.String({
        minLength: 1,
        maxLength: 240,
        description: 'Workspace-relative path to a dynamic JavaScript Team workflow.',
      }),
      args: Type.Optional(
        Type.Record(Type.String(), Type.Unknown(), {
          description: 'Structured input exposed to the script as the read-only args global.',
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
    description: 'Send a direct handoff or question to another member of the current Team.',
    parameters: Type.Object({
      target: Type.String({
        minLength: 1,
        description: 'Target task name, Agent id, or canonical name.',
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
    description: 'Inspect the current Team roster and member task states for coordination.',
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
