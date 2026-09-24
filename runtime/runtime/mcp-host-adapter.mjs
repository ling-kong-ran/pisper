// 对外 MCP 只接收明确列出的领域能力，避免把 AgentRuntimeService 的内部状态与
// 密钥配置一并传给传输层。具体会话权限仍由 Runtime 本身执行。
export function createMcpHostAdapter(runtime) {
  return {
    capabilities: runtime.capabilities,
    cwd: runtime.cwd,
    listSessions: () => runtime.listSessions(),
    getSessionMessagePage: (id, options) => runtime.getSessionMessagePage(id, options),
    getSessionLive: (id) => runtime.getSessionLive(id),
    createSession: (name, cwd) => runtime.createSession(name, cwd),
    renameSession: (id, name, options) => runtime.renameSession(id, name, options),
    hasSession: async (id) =>
      Boolean(
        runtime.sessions?.has(id) ||
        runtime.pendingSessions?.has(id) ||
        (await runtime.findSessionInfo(id)),
      ),
    promptFromChannel: (input) => runtime.promptFromChannel(input),
    abortSession: (id) => runtime.abortSession(id),
    getSessionGoal: (id) => runtime.getSessionGoal(id),
    pauseSessionGoal: (id) => runtime.pauseSessionGoal(id),
    searchMemory: (query, limit) =>
      runtime.memory?.searchRelevant
        ? runtime.memory.searchRelevant(query, { cwd: runtime.cwd, limit })
        : [],
    getWorkflows: () => runtime.getWorkflows(),
    runWorkflow: (id, input) => runtime.runWorkflow(id, input),
    getWorkflowRun: (id) => runtime.getWorkflowRun(id),
    stopWorkflowRun: (id) => runtime.stopWorkflowRun(id),
    getSchedules: () => runtime.getSchedules(),
    runSchedule: (id) => runtime.runSchedule(id),
    getTodayUsage: () => runtime.getTodayUsage(),
    listAssets: (input) => runtime.listAssets(input),
    getSessionFileChanges: (id) => runtime.getSessionFileChanges(id),
  }
}
