// Team 工作流给模型看的文字：主 Agent 的团队执行提示、任务等待原因与完成屏障的拒绝原因。
// 纯函数，只拼文字；任务图与状态机留在 runtime/services/team-workflow.mjs。

export const TEAM_EXECUTION_MARKER = '[Pisper internal team execution]'

// 任务被依赖挡住时记在 blockedReason 里，list_agents / 团队状态会带给模型。
export const TEAM_BLOCKED_REASON = Object.freeze({
  autoClaim: 'Claimed automatically once dependencies finish',
  reclaim: 'Claimed again once dependencies finish',
  autoRetry: 'Retried automatically once dependencies finish',
})

export function teamWaitingReason(dependsOn) {
  return dependsOn.length ? `Waiting for: ${dependsOn.join(', ')}` : 'Workflow not finished'
}

export const TEAM_NOT_ACTIVE = 'No active Team is available.'

// 完成屏障：terminal 为真时任务都已停下，要给出 followup / 同名重启两条恢复路径。
export function teamUnfinishedReason(names, terminal) {
  const base = `The team still has unfinished work: ${names.join(', ')}.`
  return terminal
    ? `${base} Continue with followup_task, or restart with spawn_agent using the same taskName.`
    : base
}

export function teamMissingEvidenceReason(names) {
  return `Team tasks lack verifiable results: ${names.join(', ')}.`
}

export function teamOwnershipConflictNote(taskName, files = [], { resuming = false } = {}) {
  if (resuming) return `Team file ownership conflict detected while resuming: ${taskName}.`
  return `Team file ownership conflict with ${taskName}${files.length ? `: ${files.join(', ')}` : ''}.`
}

// Team 模式提示：主 Agent 负责动态拆解、并行委派、结果整合和最终验收。
// brief：完整的团队规则与目标已在当前上下文里时只发当前状态，规则不再每轮重复写进历史。
export function teamExecutionPrompt(goal, activeAgents = [], team = null, { brief = false } = {}) {
  const objective = String(goal?.objective || '').trim()
  const agents = Array.isArray(activeAgents)
    ? activeAgents
        .map((agent) => `${agent.canonicalName || agent.taskName || agent.id}: ${agent.status}`)
        .join(', ')
    : ''
  const tasks = Array.isArray(team?.tasks)
    ? team.tasks
        .map(
          (task) =>
            `${task.taskName}${task.role ? ` [${task.role}]` : ''} ${task.status}${task.dependsOn?.length ? ` (depends on ${task.dependsOn.join(', ')})` : ''}`,
        )
        .join('; ')
    : ''
  const status = `${
    tasks
      ? `
Current workflow status: ${tasks}`
      : ''
  }
${
  agents
    ? `
Current teammate status: ${agents}`
    : ''
}`
  if (brief)
    return `${TEAM_EXECUTION_MARKER}
Continue leading the team toward the objective, under the team operating rules stated earlier in this conversation.
${status}`
  return `${TEAM_EXECUTION_MARKER}
You are the lead of a dynamic software engineering team. The user objective below is the team's mission, not a request for a one-turn answer.

${teamObjectiveBlock(objective)}

Team operating rules:
- Start by maintaining a concrete Plan with non-overlapping workstreams, deliverables, dependencies, and acceptance evidence.
- Decompose the objective only as far as the repository and acceptance criteria require. A single direct task is valid; create multiple tasks when parallel work or independent evidence makes it useful.
- Give each subagent a bounded task, relevant file scope, expected output, and verification criteria. Roles are optional labels chosen from the actual work, not a fixed cast. Include role, files, and dependsOn in spawn_agent when applicable so the runtime can enforce ownership and dependencies.
- Members can inspect the teammate roster and send direct messages to one another. Use member-to-member messages for handoffs, questions, and evidence; do not route every coordination step through the lead.
- Tasks whose prerequisites are unfinished remain queued and are claimed automatically when they become ready. Let independent tasks run in parallel. Do not busy-poll; use completion notifications or a targeted wait only when a decision depends on a result.
- Treat subagent output as evidence and suggestions, not as proof that the project is complete. Inspect and integrate changes, resolve conflicts, run the relevant tests, and request follow-up work when a result is incomplete.
- Keep the parent session responsible for final integration, user-facing decisions, and the completion audit. Never call update_goal while a teammate is queued or running; call it with status "complete" only after every explicit requirement has concrete evidence and no required teammate result remains unresolved.
- If a task is too small to justify multiple agents, explain that decision internally and complete it directly instead of creating artificial work.
- When the task needs dynamic fan-out, branching, aggregation, or repeated verification, write a workspace JavaScript workflow and run it with run_team_workflow. The restricted script API provides agent, pipeline, parallel, phase, log, and args; the script itself has no file or Shell access. Prefer this when the orchestration logic should be visible, repeatable, and driven by intermediate results.

${TEAM_RUNTIME_GUARANTEES}
${status}`
}

const TEAM_RUNTIME_GUARANTEES =
  'The runtime persists the task graph, prevents unsafe overlapping active file ownership, recovers abandoned task leases, schedules ready work, and blocks team completion until registered work converges.'

function teamObjectiveBlock(objective) {
  return `<team_objective>
${objective}
</team_objective>`
}

// 这段文本里是否带着该目标的完整团队执行提示（目标原文加团队规则）。
export function hasFullTeamExecutionPrompt(text, goal) {
  const value = String(text || '')
  return (
    value.includes(teamObjectiveBlock(String(goal?.objective || '').trim())) &&
    value.includes(TEAM_RUNTIME_GUARANTEES)
  )
}
