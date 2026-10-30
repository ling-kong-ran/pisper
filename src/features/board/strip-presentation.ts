// 泳道条与代理面板的文案推导：状态词、第 2 行细节与时长标签。只读看板卡片，不读写任何 store。
import { useI18n } from '@/app/use-i18n'
import { formatCompactDuration, relativeTime } from '@/lib/format'
import type { BoardItem } from './board-model'
import { approvalArgsText, elapsedSince } from './card-presentation'

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

// 第 2 行细节：只用 Runtime 给出的数据，没有就留空。mono 只给命令、尾巴与错误行；
// 结果摘要、提问与“正在读取”等文字用无衬线（规格 6.4：中文永不等宽）。suffix 是跟在
// 等宽内容后面的无衬线说明（例如“可重试”）。
export type StripDetail = { text: string; mono: boolean; suffix?: string }

const EMPTY_DETAIL: StripDetail = { text: '', mono: false }

export function stripDetail(item: BoardItem, t: Translate): StripDetail {
  if (item.status === 'approval') {
    const approval = item.approvals[0]
    if (!approval)
      return item.approvalsState === 'loading'
        ? { text: t('board:card.approvalLoading'), mono: false }
        : EMPTY_DETAIL
    const text = approvalArgsText(approval) || approval.argsText || approval.toolName
    return { text: text.split('\n')[0] || approval.toolName, mono: true }
  }
  if (item.status === 'failed') {
    const line = (item.error || item.tail.at(-1) || '').split('\n')[0]
    return line
      ? { text: line, mono: true, suffix: t('board:strip.retryable') }
      : { text: t('board:strip.retryable'), mono: false }
  }
  if (item.status === 'running') {
    if (item.tail.length) return { text: item.tail.at(-1) || '', mono: true }
    const activity = item.activity
    if (activity?.type === 'tool')
      return {
        text: activity.summary ? `${activity.name} · ${activity.summary}` : activity.name,
        mono: true,
      }
    return EMPTY_DETAIL
  }
  return { text: item.preview, mono: false }
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

// 芯片是否带两个内联键（标准审批的 [拒绝][允许]）：与 StripItemActions 的分支同一判断，
// 泳道条按它给芯片留宽度。critical、截断的预览或详情未到时只有一个“查看”。
export function stripItemHasInlineActions(item: BoardItem) {
  const approval = item.approvals[0]
  return (
    item.status === 'approval' &&
    approval !== undefined &&
    approval.severity !== 'critical' &&
    !approval.truncated
  )
}
