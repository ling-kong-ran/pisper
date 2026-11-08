// MCP 相关的工具说明：外部 MCP 服务接入会话时生成的工具（mcp-service.mjs），
// 以及 Pisper 自己作为 MCP 服务对外提供的工具（mcp-host-tools.mjs）。说明给模型读，统一英文、尽量短。

/**
 * 外部 MCP 服务的工具：说明沿用远端给的描述，补上来源，便于模型区分同名工具。
 * @param {{ name: string, serverName: string, remoteName: string, remoteDescription?: string }} input
 */
export function mcpRemoteTool({ name, serverName, remoteName, remoteDescription }) {
  return {
    description: `${remoteDescription || remoteName}\nRemote MCP server: ${serverName}. Remote tool name: ${remoteName}.`,
    promptSnippet: `Call ${remoteName} on the ${serverName} MCP server`,
    promptGuidelines: [
      `Use ${name} only when the ${serverName} MCP capability is relevant to the user's task.`,
    ],
  }
}

/** Pisper MCP 服务对外的工具说明，键是工具名。 */
export const MCP_HOST_TOOLS = Object.freeze({
  pisper_capabilities: 'List the Pisper Runtime capabilities available on this device.',
  pisper_list_sessions:
    'List Pisper conversations, optionally filtering by title. Returns at most 100.',
  pisper_get_session_messages:
    'Read one page of conversation messages. Use nextCursor to page backwards.',
  pisper_get_session_live:
    'Get live status of a conversation, including whether it is streaming or waiting for user approval.',
  pisper_create_session:
    'Create a Pisper conversation. New conversations retain Pisper’s default approval-required mode.',
  pisper_rename_session: 'Rename an existing Pisper conversation.',
  pisper_send_message:
    'Start a Pisper agent turn. Returns a run ID immediately; poll pisper_get_run. Pisper session permissions and approvals still apply.',
  pisper_get_run: 'Poll a Pisper turn started by pisper_send_message.',
  pisper_abort_session: 'Stop an active Pisper conversation turn.',
  pisper_get_goal: 'Read goal state for a Pisper conversation.',
  pisper_pause_goal: 'Pause an active Pisper goal.',
  pisper_search_memory: 'Search Pisper long-term memory for relevant user or project context.',
  pisper_list_workflows: 'List available Pisper workflows.',
  pisper_run_workflow: 'Start a published Pisper workflow using its existing approval rules.',
  pisper_get_workflow_run: 'Read a Pisper workflow run.',
  pisper_stop_workflow_run: 'Stop a Pisper workflow run.',
  pisper_list_schedules: 'List Pisper scheduled tasks.',
  pisper_run_schedule: 'Run an existing Pisper scheduled task now.',
  pisper_get_usage_today: 'Read today’s Pisper token usage.',
  pisper_list_assets: 'Search Pisper assets by name or session.',
  pisper_get_session_changes: 'Read tracked file changes for a Pisper conversation.',
})

/** 结果超过 MCP 返回上限时给调用方模型的提示。 */
export const MCP_HOST_RESULT_TOO_LARGE = 'Result too large; narrow the query or lower the limit.'
