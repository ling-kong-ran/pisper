import { useIsMutating, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { MCP_MUTATION_KEY, mcpDashboardQueryOptions, mcpMutationOptions } from './mcp-queries'

export function useMcpDashboard() {
  const client = useQueryClient()
  const busy = useIsMutating({ mutationKey: MCP_MUTATION_KEY }) > 0
  const query = useQuery({
    ...mcpDashboardQueryOptions(),
    enabled: !busy,
    refetchInterval: busy ? false : 10_000,
  })
  const mutation = useMutation(mcpMutationOptions(client))
  return {
    data: query.data,
    loading: query.isPending,
    error: mutation.error || query.error,
    busy,
    change: mutation.mutateAsync,
    reload: () => {
      mutation.reset()
      return query.refetch()
    },
  }
}
