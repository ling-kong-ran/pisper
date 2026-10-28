import { queryOptions, type QueryClient } from '@tanstack/react-query'
import { assetsApi, normalizeAssetFilter, type AssetFilter } from './assets-api'

export const ASSETS_LIST_KEY = ['assets', 'list'] as const
export function assetsListQueryOptions(filter: AssetFilter = {}) {
  const normalized = normalizeAssetFilter(filter)
  return queryOptions({
    queryKey: [...ASSETS_LIST_KEY, normalized] as const,
    queryFn: ({ signal }) => assetsApi.list(normalized, signal),
    staleTime: 0,
    retry: false,
  })
}
export function assetPreviewQueryOptions(id: string, generation: number) {
  return queryOptions({
    // 打开意图拥有一次独占读取，重开同资产不接收旧意图的回执。
    queryKey: ['assets', 'preview', id, generation] as const,
    queryFn: ({ signal }) => assetsApi.preview(id, signal),
    staleTime: 0,
    gcTime: 0,
    retry: false,
  })
}
export async function refreshAssetLists(client: QueryClient) {
  await client.cancelQueries({ queryKey: ASSETS_LIST_KEY })
  await client.invalidateQueries({ queryKey: ASSETS_LIST_KEY })
}
