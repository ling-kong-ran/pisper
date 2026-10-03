// 定时计划常量：目标渠道、频率、时间单位和时区列表。
import { Bell, Bot, MessageCircle, Send } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { IntervalUnit, NotificationTarget, ScheduleExecutionMode, ScheduleFrequency } from './schedule-types'

export const TARGETS: Record<NotificationTarget, { name: string; Icon: LucideIcon }> = {
  browser: { name: '通知', Icon: Bell },
  feishu: { name: '飞书', Icon: Bot },
  weixin: { name: '微信', Icon: MessageCircle },
  qq: { name: 'QQ', Icon: MessageCircle },
  telegram: { name: 'Telegram', Icon: Send },
}

export const FREQUENCIES: Record<ScheduleFrequency, string> = {
  interval: '每隔一段时间',
  daily: '每天',
  weekly: '每周',
  monthly: '每月',
}

export const INTERVAL_UNITS: Record<IntervalUnit, string> = {
  minutes: '分钟',
  hours: '小时',
  days: '天',
}

export const TIMEZONES: string[] = [
  ...new Set([
    'Asia/Hong_Kong',
    'UTC',
    ...(typeof Intl.supportedValuesOf === 'function' ? Intl.supportedValuesOf('timeZone') : []),
  ]),
]

export const SCHEDULE_EXECUTION_MODES: ScheduleExecutionMode[] = ['full-access']
