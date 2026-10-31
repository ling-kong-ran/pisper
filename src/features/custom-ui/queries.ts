// 目录与实例查询的共享选项：放置轻量入口（placements.ts）与各放置宿主（hosts/*）读同一份缓存。
// 单独成文件，是因为 hosts/* 不能在运行时导入 placements.ts（placements 经 import() 到达 hosts，
// 反向静态导入会形成依赖环）；这里只依赖请求函数与查询键，不带任何界面代码。
import { listComponentCatalog, listComponentInstances } from './custom-ui-api'
import { customUiQueryKeys } from './query-keys'

/** 目录查询：所有放置与组件页读同一份缓存，最后一个使用者离开时才取消请求。 */
export function componentCatalogQueryOptions(enabled = true) {
  return {
    queryKey: customUiQueryKeys.catalog(),
    queryFn: ({ signal }: { signal?: AbortSignal }) => listComponentCatalog(signal),
    enabled,
    staleTime: 30_000,
    retry: false,
    refetchOnWindowFocus: false,
  } as const
}

/** 实例查询（全部放置位置、含已停用的）。 */
export function componentInstancesQueryOptions(enabled = true) {
  return {
    queryKey: customUiQueryKeys.instances(),
    queryFn: ({ signal }: { signal?: AbortSignal }) => listComponentInstances(signal),
    enabled,
    staleTime: 30_000,
    retry: false,
    refetchOnWindowFocus: false,
  } as const
}
