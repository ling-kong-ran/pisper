// 多 Agent 工具（spawn / list / send / followup / wait / interrupt、Team 任务与工作流、Team 成员）的说明。

export const FOLLOWUP_TASK = Object.freeze({
  description: 'Give an existing Agent another task, keeping its context.',
})

export const INTERRUPT_AGENT = Object.freeze({
  description: "Stop an Agent's current run, keeping its record.",
})

export const LIST_AGENTS = Object.freeze({
  description: "List this session's Agents and their status.",
})

export const LIST_TEAM_MEMBERS = Object.freeze({
  description: 'Inspect the current Team roster and member task states for coordination.',
})

export const RUN_TEAM_WORKFLOW = Object.freeze({
  description:
    'Execute a workspace JavaScript workflow that orchestrates Team agents through a restricted API.',
  snippet: 'Write and execute a dynamic JavaScript Team workflow script',
  guidelines: [
    'Use a workspace-relative .js file beginning with export const meta = { name, description }.',
    'The script may use agent(prompt, options), pipeline(items, worker), parallel(tasks), phase(title), log(message), and args; it must not import modules or access files directly.',
    'Begin with export const meta = { name, description }, then use top-level await and return the final serializable result.',
    'Use agent results to branch, fan out, aggregate, verify, and converge; options may include label, role, agent (an agent definition name), files, dependsOn, and schema.',
  ],
  params: {
    path: 'Workspace-relative path to a dynamic JavaScript Team workflow.',
    args: 'Structured input exposed to the script as the read-only args global.',
  },
})

export const SEND_MESSAGE = Object.freeze({
  description: 'Send information to a running Agent.',
})

export const SEND_TEAM_MESSAGE = Object.freeze({
  description: 'Send a direct handoff or question to another member of the current Team.',
  params: {
    target: 'Target task name, Agent id, or canonical name.',
  },
})

export const SPAWN_AGENT = Object.freeze({
  guidelines: [
    'For substantial work, check once whether independent workstreams exist and use spawn_agent proactively when delegation shortens the critical path, even if the user did not explicitly request subagents: codebase investigation, platform-specific checks, test diagnosis, review of a bounded area.',
    'Do not spawn for trivial work, a single linear task, tightly coupled changes in the same files, or when coordination costs more than doing it directly. Keep the immediate critical-path step in the parent session and keep working after delegating.',
    "Agents never inherit the transcript: give a self-contained message with every constraint and piece of context. They use this session's permission mode and workspace and cannot spawn Agents.",
    "Pass agent when a definition fits; it sets model, reasoning, tools and standing instructions, so do not send file-changing work to one without write tools. Otherwise pass tier: fast for lookups and summaries, standard for ordinary edits, strong for design, hard debugging and review. Without either, the Agent inherits this session's model, reasoning and tools.",
    "Agents run in the background and must not delay replying to the user, handling later instructions, or spawning other independent Agents. After spawning, say briefly in the user's language that the work was delegated, without internals.",
    'Do not call list_agents or wait_agent merely to monitor progress: results are automatically delivered to the parent session. Use list_agents only when asked for status and wait_agent only when the user asks to wait for a result; never repeat wait_agent after a timeout.',
  ],
  params: {
    taskName: 'Short stable name (letters, digits, - or _)',
    message: 'Self-contained task',
    role: 'Team role, e.g. investigator or tester',
    agent: 'Agent definition name (.pisper/agents/*.md)',
    tier: 'Ignored if the definition names a model or tier',
    files: 'Paths this task owns while active (Team)',
    dependsOn: 'Task names that must finish first (Team)',
  },
})

export const UPDATE_TEAM_TASK = Object.freeze({
  description:
    'Update a queued Team task when new evidence changes its scope, dependencies, role, or deliverable.',
  snippet: 'Adjust the Team task graph when new evidence changes the work plan',
  guidelines: [
    'Update only queued, blocked, failed, or interrupted tasks; interrupt a running task before changing its graph entry.',
    'Keep dependencies explicit and avoid cycles or overlapping active file ownership.',
  ],
  params: {
    target: 'Team task id or task name.',
  },
})

export const WAIT_AGENT = Object.freeze({
  description: 'Wait briefly for an Agent to finish.',
  params: {
    timeoutMs: 'Default 15000',
  },
})

export const AGENT_TARGET = Object.freeze({
  target: 'Agent id, task name or canonical name',
  optionalTarget: 'Agent id, task name or canonical name; omit for any active Agent',
})

export const AGENT_SUMMARY_TRUNCATED =
  '\n\n[Agent summary truncated to the Pisper tool-output limit.]'

const TIERS = ['fast', 'standard', 'strong']

/**
 * spawn_agent 的说明：可用的 Agent 定义（每个一行摘要）与档位此刻对应的模型。
 * 档位一个都没设置时全部跟随当前会话模型，不必写。
 * @param {{ definitions?: string[], tierModels?: Record<string, string | undefined> }} [options]
 */
export function spawnAgentDescription({ definitions = [], tierModels = {} } = {}) {
  const parts = ['Start an isolated background Agent on a concrete, bounded subtask.']
  if (definitions.length)
    parts.push(
      `Agent definitions available in this workspace (pass the name as agent):\n${definitions.join('\n')}`,
    )
  if (TIERS.some((tier) => tierModels[tier]))
    parts.push(
      `Model tiers (pass as tier when no agent definition fits): ${TIERS.map(
        (tier) => `${tier} → ${tierModels[tier] || 'this session model'}`,
      ).join('; ')}.`,
    )
  return parts.join('\n\n')
}

/** @param {any} agent */
export function spawnResultText(agent) {
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

// 中断可能恰好落在多文件编辑中间，提醒调用方先验证工作区再决定接管方式。
/** @param {{ canonicalName: string, status: string }} agent */
export function interruptResultText(agent) {
  const caution =
    agent.status === 'interrupted'
      ? ' Its workspace may hold partial multi-file edits; verify compilation and tests before continuing, and finish leftover work by respawning instead of messaging this Agent.'
      : ''
  return `${agent.canonicalName} is ${agent.status}.${caution}`
}
