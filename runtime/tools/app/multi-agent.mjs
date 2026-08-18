// 多 Agent 工具：spawn/list/send/followup/wait/interrupt 六件套，
// 是父会话调度子 Agent 的接口（内部运行时工具，不进插件目录）。
import { DEFAULT_MAX_BYTES, defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import {
  DEFAULT_AGENT_MAX_TURNS,
  MAX_AGENT_MAX_TURNS,
} from '../../services/multi-agent-service.mjs'

const category = 'collaboration'
const source = 'app'

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
      'Spawn an asynchronous Agent for explicitly requested delegation or parallel work',
    promptGuidelines: [
      'Do not use spawn_agent unless the user explicitly asks for subagents, delegation, parallel agent work, or an applicable project instruction requires it.',
      'Use spawn_agent only for a concrete, bounded task that can run independently while you continue non-overlapping local work.',
      'Do not delegate the immediate critical-path step and then wait idly for it.',
      'Provide a self-contained message with every constraint and piece of context the Agent needs; Agents never inherit the parent transcript.',
      'Agents inherit the current model, current reasoning level, tools, permission mode, and workspace boundary. They cannot recursively spawn more Agents.',
      'A spawned Agent is a background task. Its running state must not delay replying to the user, handling later user instructions, or spawning other independent Agents.',
      'After spawning, acknowledge only that the work was delegated to a background Agent, phrased briefly in the user’s language, without explaining mailbox, prompt, or context internals.',
      'Do not call list_agents or wait_agent merely to monitor progress. Completed results remain durable in the parent mailbox and update the UI, but are never injected into the parent prompt or model context.',
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
      maxTurns: Type.Optional(
        Type.Integer({
          minimum: 1,
          maximum: MAX_AGENT_MAX_TURNS,
          description: `Maximum Agent Loop turns. Defaults to ${DEFAULT_AGENT_MAX_TURNS}.`,
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

export const factories = {
  spawn_agent: createSpawnAgentTool,
  list_agents: createListAgentsTool,
  send_message: createSendMessageTool,
  followup_task: createFollowupTaskTool,
  wait_agent: createWaitAgentTool,
  interrupt_agent: createInterruptAgentTool,
}

// Internal runtime tools: always available to the primary Agent, never shown in the plugins UI.
export function createMultiAgentTools(context = {}) {
  return manifests.map((manifest) => factories[manifest.id](context))
}
