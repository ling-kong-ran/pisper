// 侧栏行右侧的时间文字（规格 v2 第 2.2 节）：纯函数，侧栏、手机抽屉与测试共用。
// 需要你：等待时长只取一个单位（“33s”“18m”“2h”“3d”），窄列里宽度稳定、一眼可读；
// 运行中：实时计时“3:12”（超过一小时“1:02:05”），配等宽数字；
// 今天完成：完成时刻“14:05”（24 小时制，按本机时区）。
// 非法或负数一律按 0 处理，缺失的时间返回空串，由界面隐藏。
import type { SidebarReason } from './sidebar-model'

type Translate = (key: string, params?: Record<string, unknown>) => string

// “需要你”的中文原因（侧栏行、全部代理行、读屏播报共用）：键写成字面量（i18n 检查要求），
// 组合时按“失败 → 批准 → 问题”的顺序用“ · ”连接。
export function reasonText(t: Translate, reasons: readonly SidebarReason[]) {
  return reasons
    .map((reason, index) => {
      if (reason.kind === 'failed') return t('board:sidebar.reason.failed')
      if (reason.kind === 'approval')
        return reason.critical
          ? t('board:sidebar.reason.criticalApproval')
          : t('board:sidebar.reason.approval')
      if (reason.count === null) return t('board:sidebar.reason.question')
      return index === 0
        ? t('board:sidebar.reason.questions', { count: reason.count })
        : t('board:sidebar.reason.questionCount', { count: reason.count })
    })
    .join(' · ')
}

export function formatSidebarWait(milliseconds: number | null) {
  if (milliseconds === null || !Number.isFinite(milliseconds)) return ''
  const seconds = Math.max(0, Math.floor(milliseconds / 1000))
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h`
  return `${Math.floor(hours / 24)}d`
}

export function formatSidebarClock(milliseconds: number | null) {
  if (milliseconds === null || !Number.isFinite(milliseconds)) return ''
  const total = Math.max(0, Math.floor(milliseconds / 1000))
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const seconds = String(total % 60).padStart(2, '0')
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, '0')}:${seconds}`
    : `${minutes}:${seconds}`
}

export function formatSidebarTime(timestamp: number | null) {
  if (timestamp === null || !Number.isFinite(timestamp)) return ''
  const date = new Date(timestamp)
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`
}

// 置顶行的相对时间：今天之内写时刻，更早写“月/日”；与历史会话的日期写法一致，不出现“3 天前”。
export function formatSidebarDay(timestamp: number | null, now: number) {
  if (timestamp === null || !Number.isFinite(timestamp)) return ''
  const date = new Date(timestamp)
  const today = new Date(now)
  if (
    date.getFullYear() === today.getFullYear() &&
    date.getMonth() === today.getMonth() &&
    date.getDate() === today.getDate()
  )
    return formatSidebarTime(timestamp)
  return `${date.getMonth() + 1}/${date.getDate()}`
}
