// 手机代理条的文案推导：状态词与时长标签。只读看板卡片，不读写任何 store。
// 桌面泳道条与 ⌘J 代理面板已退役，第 2 行细节与内联键宽度的推导随之删除。
import { useI18n } from '@/app/use-i18n'
import { formatCompactDuration, relativeTime } from '@/lib/format'
import type { BoardItem } from './board-model'
import { elapsedSince } from './card-presentation'

type Translate = ReturnType<typeof useI18n>['t']

export function useStripWords() {
  const { t } = useI18n()
  return (item: BoardItem) => {
    if (item.status === 'approval') return t('board:strip.wordApproval')
    if (item.status === 'question') return t('board:strip.wordQuestion')
    if (item.status === 'failed') return t('board:strip.wordFailed')
    if (item.status === 'running') return t('board:status.running')
    if (item.status === 'queued') return t('board:status.queued')
    if (item.status === 'stopped') return t('board:status.stopped')
    return t('board:status.done')
  }
}

// 时长标签拆成无衬线前缀（“已等”“已运行”）与等宽数值；已完成用相对时间（“40 分钟前”），
// 与看板卡片一致，不让裸时长读起来像耗时。
export type StripSince = { prefix: string; value: string; mono: boolean }

export function sinceLabel(
  item: BoardItem,
  now: number,
  t: Translate,
  language = 'zh-CN',
): StripSince | null {
  if (item.lane === 'done') {
    if (item.since === null) return null
    return { prefix: '', value: relativeTime(item.since, language), mono: false }
  }
  const elapsed = elapsedSince(item.lane === 'running' ? item.startedAt : item.since, now)
  if (elapsed === null) return null
  const value = formatCompactDuration(elapsed)
  if (item.lane === 'attention') return { prefix: t('board:card.waited'), value, mono: true }
  if (item.lane === 'running') return { prefix: t('board:card.ran'), value, mono: true }
  return { prefix: '', value, mono: true }
}
