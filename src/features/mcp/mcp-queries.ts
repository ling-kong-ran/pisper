import { queryOptions, type QueryClient } from '@tanstack/react-query'
import { mcpApi, type McpDashboard } from './mcp-api'

export const MCP_QUERY_KEY = ['mcp', 'dashboard'] as const
export const MCP_MUTATION_KEY = ['mcp', 'change'] as const

export function mcpDashboardQueryOptions() {
  return queryOptions({
    queryKey: MCP_QUERY_KEY,
    queryFn: ({ signal }) => mcpApi.dashboard(signal),
    staleTime: 0,
    retry: false,
  })
}

export function mcpMutationOptions(client: QueryClient) {
  return {
    mutationKey: MCP_MUTATION_KEY,
    scope: { id: 'mcp-dashboard' },
    retry: false,
    mutationFn: async (change: () => Promise<McpDashboard>) => {
      // 停止旧查询后再写入新快照，取消信号同时终止底层 HTTP 等待。
      await client.cancelQueries({ queryKey: MCP_QUERY_KEY })
      return change()
    },
    onSuccess: (data: McpDashboard) => client.setQueryData(MCP_QUERY_KEY, data),
    onSettled: () => client.invalidateQueries({ queryKey: MCP_QUERY_KEY }),
  }
}
