// mcp_list / mcp_manage 的说明。

export const MCP_LIST = Object.freeze({
  description: 'Read configured MCP services, connection status, and available tools.',
  snippet: 'List configured MCP services and their current tool availability',
  guidelines: [
    'Use mcp_list before changing an existing MCP service so you have its exact service ID and current transport.',
    'The result is already credential-safe; never try to read MCP credential files directly.',
  ],
  params: {
    refresh: 'Reconnect and refresh tool discovery before returning.',
  },
})

export const MCP_MANAGE = Object.freeze({
  description:
    'Manage MCP services with structured parameters. To add a service, use action="add" and put the complete connection in config: stdio requires name, transport="stdio", and command; http or sse requires name, transport, and url.',
  snippet: 'Manage MCP services with typed fields instead of shell commands or local HTTP requests',
  guidelines: [
    'Always use mcp_manage for MCP configuration. Never call the application MCP API through bash, PowerShell, curl, fetch, or an inline script.',
    'To add a stdio server, call { action: "add", config: { name, transport: "stdio", command, args?, cwd?, env? } }. To add a remote server, call { action: "add", config: { name, transport: "http" or "sse", url, headers? } }.',
    'Use Windows paths exactly as ordinary strings in the command or cwd field; do not add JavaScript, JSON, or shell escaping beyond the structured argument encoding.',
    'Call mcp_list first when updating, deleting, testing, or toggling an existing service.',
    'Do not read credential files. Put credentials only in the structured env or headers object supplied by the user or an approved onboarding flow.',
  ],
})
