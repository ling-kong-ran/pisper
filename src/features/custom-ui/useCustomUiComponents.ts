import { queryOptions, useQuery } from '@tanstack/react-query'
import { listCustomUiComponents } from './custom-ui-api'

export function customUiComponentsQueryOptions() {
  return queryOptions({
    queryKey: ['custom-ui-components'],
    queryFn: ({ signal }) => listCustomUiComponents(signal),
    staleTime: 30_000,
    retry: false,
    refetchOnWindowFocus: false,
  })
}

// 目录由查询缓存拥有；移除单个画布组件不应中断其他页面正在使用的同一请求。
export function useCustomUiComponents() {
  return useQuery(customUiComponentsQueryOptions())
}
