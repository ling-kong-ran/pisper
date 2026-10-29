// 泳道名称与固定排序说明的文案：看板泳道、手机区段与跳转条共用。
import type { BoardLane } from '@shared/session-board-status.mjs'
import { useI18n } from '@/app/use-i18n'

export function useLaneLabels() {
  const { t } = useI18n()
  return {
    name: {
      attention: t('board:lane.attention'),
      running: t('board:lane.running'),
      queued: t('board:lane.queued'),
      done: t('board:lane.done'),
    } satisfies Record<BoardLane, string>,
    order: {
      attention: t('board:lane.orderAttention'),
      running: t('board:lane.orderRunning'),
      queued: t('board:lane.orderQueued'),
      done: t('board:lane.orderDone'),
    } satisfies Record<BoardLane, string>,
  }
}
