// 完整会话目录（含归档、不上看板的旧会话）：侧栏、历史等需要全量列表的视图从这里读。
// 读的是看板同一份权威缓存，轮询由看板同步统一负责，调用方不得再设置自己的 refetchInterval。
// 与 startupQueryOptions<T> 一致，类型参数只声明调用方关心的字段视图，不做运行时校验。
import { useQuery } from '@tanstack/react-query'
import { startupQueryOptions } from '@/lib/startup-queries'
import type { BoardSessionsData } from './board-store'
import { useBoardSync } from './useBoardSync'

export function useSessionListQuery<T extends { sessions: unknown[] } = BoardSessionsData>() {
  useBoardSync()
  return useQuery(startupQueryOptions<T>('sessions'))
}
