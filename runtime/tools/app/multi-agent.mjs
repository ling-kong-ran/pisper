// 多 Agent 工具：spawn/list/send/followup/wait/interrupt 六件套，
// 是父会话调度子 Agent 的接口（内部运行时工具，不进插件目录）。
import { DEFAULT_MAX_BYTES, defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
const category = 'collaboration'
const source = 'app'

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
  const output = agent.output ? `\n${agent.output}` : agent.error ? `\nError: ${agent.error}` : ''
  return `${agent.canonicalName} · ${agent.status}${elapsed}${output}`
}

function compactAgents(agents) {
  return boundedToolText(agents.map(compactAgent).join('\n\n'))
}

function agentDetails(agent) {
  const { fullOutput: _fullOutput, ...details } = agent
  return details
}

function createSpawnAgentTool({ multiAgentRuntime }) {
  return defineTool({
    name: 'spawn_agent',
    label: 'Spawn Agent',
    description:
      'Spawn an isolated Agent for a concrete, bounded subtask that can run independently while you continue useful local work.',
    promptSnippet:
      'Proactively delegate concrete independent work to an asynchronous background Agent',
    promptGuidelines: [
      'For substantial work, briefly identify whether two or more concrete workstreams can proceed independently; use spawn_agent proactively when delegation will shorten the critical path, even if the user did not explicitly request subagents.',
      'Good candidates include independent codebase investigation, platform-specific checks, focused test diagnosis, and review of a bounded area while the parent continues implementation.',
      'Do not spawn for trivial work, a single linear task, tightly coupled changes in the same files, or work whose coordination cost is likely higher than doing it directly.',
      'Use spawn_agent only for a concrete, bounded task that can run independently while you continue non-overlapping local work.',
      'Keep the immediate critical-path step in the parent session and continue useful work after delegating it.',
      'Provide a self-contained message with every constraint and piece of context the Agent needs; Agents never inherit the parent transcript.',
      'Agents inherit the current model, current reasoning level, tools, permission mode, and workspace boundary. They cannot recursively spawn more Agents.',
      'A spawned Agent is a background task. Its running state must not delay replying to the user, handling later user instructions, or spawning other independent Agents.',
      'After spawning, acknowledge only that the work was delegated to a background Agent, phrased briefly in the user’s language, without explaining mailbox, prompt, or context internals.',
      'Do not call list_agents or wait_agent merely to monitor progress. Completed results remain durable, update the UI, and are automatically delivered to the parent session.',
      'Use wait_agent only when the user explicitly asks you to wait for a specific Agent result before replying.',
    ],
    parameters: Type.Object({
      taskName: Type.String({
        minLength: 1,
        maxLength: 48,
        description: 'Stable short task name using letters, digits, hyphens, or underscores.',
      }),
      message: Type.String({
        minLength: 1,
        maxLength: 12_000,
        description:
          'Concrete, self-contained delegated task including all required context and constraints.',
      }),
      role: Type.Optional(
        Type.String({
          maxLength: 80,
          description:
            'Team role such as investigator, architect, implementer, tester, or reviewer.',
        }),
      ),
      files: Type.Optional(
        Type.Array(Type.String({ maxLength: 240 }), {
          maxItems: 96,
          description: 'Workspace-relative files or directories this task owns while it is active.',
        }),
      ),
      dependsOn: Type.Optional(
        Type.Array(Type.String({ maxLength: 48 }), {
          maxItems: 32,
          description: 'Earlier Team task names that must be completed before this task starts.',
        }),
      ),
    }),
    async execute(_toolCallId, params) {
      const agent = await requireRuntime(multiAgentRuntime, 'spawn')(params)
      const action = agent.status === 'queued' ? 'Queued' : 'Started'
      return { ...text(`${action} ${agent.canonicalName} in the background.`), details: agent }
    },
  })
}

function createListAgentsTool({ multiAgentRuntime }) {
  return defineTool({
    name: 'list_agents',
    label: 'List Agents',
    description:
      'List Agents created by the current primary session, including live and completed states. Use only for an explicit status inspection, not periodic monitoring.',
    promptSnippet: 'Inspect Subagent status only when the user explicitly requests it',
    promptGuidelines: [
      'Do not call list_agents repeatedly to monitor background Agents.',
      'Running Agents do not block the parent from replying or starting other independent Agents.',
    ],
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
    description: 'Send information to an existing Agent without starting a separate new Agent.',
    parameters: Type.Object({
      target: Type.String({
        minLength: 1,
        description: 'Agent id, task name, or canonical name returned by spawn_agent.',
      }),
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
    description: 'Give an existing Agent another task while preserving that Agent context.',
    parameters: Type.Object({
      target: Type.String({
        minLength: 1,
        description: 'Agent id, task name, or canonical name returned by spawn_agent.',
      }),
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
    description:
      'Wait briefly for an Agent to reach a terminal state only when the user explicitly asks to wait. Never use this tool as a polling loop.',
    promptSnippet:
      'Wait for a Subagent only when the user explicitly requires its result before the current reply',
    promptGuidelines: [
      'Do not use wait_agent merely because an Agent is running.',
      'Never call wait_agent repeatedly after a timeout to poll for completion.',
      'The parent should normally reply while Agents continue in the background. Completion updates the UI and durable mailbox without injecting model context; inspect results later with list_agents or an explicitly requested wait_agent call.',
    ],
    parameters: Type.Object({
      target: Type.Optional(
        Type.String({
          minLength: 1,
          description:
            'Optional Agent id, task name, or canonical name. Without a target, returns when any currently active Agent finishes.',
        }),
      ),
      timeoutMs: Type.Optional(
        Type.Integer({
          minimum: 250,
          maximum: 30_000,
          description: 'Maximum time to wait for completion. Defaults to 15000 ms.',
        }),
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
    description:
      'Interrupt an Agent current run while preserving its record for inspection or a later follow-up.',
    parameters: Type.Object({
      target: Type.String({ minLength: 1, description: 'Agent id, task name, or canonical name.' }),
    }),
    async execute(_toolCallId, params) {
      const agent = await requireRuntime(multiAgentRuntime, 'interrupt')(params.target)
      return { ...text(`${agent.canonicalName} is ${agent.status}.`), details: agent }
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
      'Use agent results to branch, fan out, aggregate, verify, and converge; options may include label, role, files, dependsOn, and schema.',
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
