// “更多”磁贴角标（规格 2.5）：工作流运行中、定时任务启用数、渠道断开数（琥珀：需要人处理）、MCP 在线数。
// 每项只在对应磁贴可见（Runtime 没有裁掉）时读取，随更多页懒加载；数据所有者是 React Query 缓存，
// 离开页面即停止轮询。读取失败或数量为 0 时不显示角标，不用占位数字伪装。
import { useQuery } from '@tanstack/react-query'
import type { Translate } from '@/app/navigation'
import { fetchDisconnectedChannelCount } from '@/features/channels/channel-status'
import { fetchOnlineMcpCount } from '@/features/mcp/mcp-status'
import { fetchEnabledScheduleCount } from '@/features/schedules/schedule-status'
import { fetchRunningWorkflowCount } from '@/features/workflows/workflow-status'

export type MoreTileBadgeTone = 'run' | 'attn' | 'neutral'
export type MoreTileBadge = { label: string; tone: MoreTileBadgeTone }

const SOURCES = [
  {
    key: 'page:workflows',
    load: fetchRunningWorkflowCount,
    tone: 'run',
    // 运行数变化快，页面打开期间 15 秒刷新一次。
    interval: 15_000,
  },
  {
    key: 'page:schedules',
    load: fetchEnabledScheduleCount,
    tone: 'neutral',
    interval: 60_000,
  },
  {
    key: 'page:channels',
    load: fetchDisconnectedChannelCount,
    tone: 'attn',
    interval: 30_000,
  },
  {
    key: 'page:mcp',
    load: fetchOnlineMcpCount,
    tone: 'neutral',
    interval: 60_000,
  },
] as const

function useTileCount(source: (typeof SOURCES)[number], enabled: boolean) {
  return useQuery({
    queryKey: ['more-tile-badge', source.key],
    queryFn: ({ signal }) => source.load(signal),
    enabled,
    staleTime: 10_000,
    refetchInterval: enabled ? source.interval : false,
    retry: false,
  }).data
}

export function useMoreTileBadges(t: Translate, tileKeys: ReadonlySet<string>) {
  // 四个查询固定顺序调用（Hook 规则），是否真正发请求由 enabled 决定。
  const counts = [
    useTileCount(SOURCES[0], tileKeys.has(SOURCES[0].key)),
    useTileCount(SOURCES[1], tileKeys.has(SOURCES[1].key)),
    useTileCount(SOURCES[2], tileKeys.has(SOURCES[2].key)),
    useTileCount(SOURCES[3], tileKeys.has(SOURCES[3].key)),
  ]
  // 文案键保持字面量，便于 i18n 静态检查；顺序与 SOURCES 一致。
  const labels = [
    (count: number) => t('navigation:more.badgeRunning', { count }),
    (count: number) => t('navigation:more.badgeEnabled', { count }),
    (count: number) => t('navigation:more.badgeDisconnected', { count }),
    (count: number) => t('navigation:more.badgeOnline', { count }),
  ]
  const badges = new Map<string, MoreTileBadge>()
  SOURCES.forEach((source, index) => {
    const count = counts[index]
    if (typeof count === 'number' && count > 0 && tileKeys.has(source.key))
      badges.set(source.key, { label: labels[index](count), tone: source.tone })
  })
  return badges
}
