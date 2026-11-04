// 角色名与设置项的文案（键必须是字面量，i18n 检查逐个核对）。
import type { Translate } from '@/app/navigation'
import type { BotCharacter } from './avatar'
import type { Bot, BotPlatform, BotSchedule } from './bots-api'

export function characterLabel(t: Translate, character: BotCharacter) {
  switch (character) {
    case 'bean':
      return t('bots:character.bean')
    case 'robot':
      return t('bots:character.robot')
    case 'cat':
      return t('bots:character.cat')
    case 'owl':
      return t('bots:character.owl')
    case 'ghost':
      return t('bots:character.ghost')
    case 'cloud':
      return t('bots:character.cloud')
    case 'octo':
      return t('bots:character.octo')
    case 'bunny':
      return t('bots:character.bunny')
    case 'bear':
      return t('bots:character.bear')
    case 'fox':
      return t('bots:character.fox')
    case 'star':
      return t('bots:character.star')
    default:
      return t('bots:character.blob')
  }
}

export function executionModeLabel(t: Translate, mode: Bot['executionMode']) {
  if (mode === 'full-access') return t('bots:details.modeFull')
  if (mode === 'workspace-write') return t('bots:details.modeWorkspace')
  return t('bots:details.modeApproval')
}

export function platformLabel(t: Translate, platform: BotPlatform) {
  if (platform === 'weixin') return t('bots:triggers.weixin')
  if (platform === 'qq') return t('bots:triggers.qq')
  if (platform === 'telegram') return t('bots:triggers.telegram')
  return t('bots:triggers.feishu')
}

export function weekdayLabel(t: Translate, day: number) {
  switch (day) {
    case 0:
      return t('bots:triggers.sunday')
    case 2:
      return t('bots:triggers.tuesday')
    case 3:
      return t('bots:triggers.wednesday')
    case 4:
      return t('bots:triggers.thursday')
    case 5:
      return t('bots:triggers.friday')
    case 6:
      return t('bots:triggers.saturday')
    default:
      return t('bots:triggers.monday')
  }
}

export function scheduleLabel(t: Translate, schedule: BotSchedule) {
  if (schedule.frequency === 'once')
    return t('bots:triggers.onceAt', { date: schedule.date, time: schedule.time })
  if (schedule.frequency === 'weekly')
    return t('bots:triggers.weeklyAt', {
      day: weekdayLabel(t, schedule.dayOfWeek),
      time: schedule.time,
    })
  if (schedule.frequency === 'daily') return t('bots:triggers.dailyAt', { time: schedule.time })
  return t('bots:triggers.otherSchedule')
}

// “a, b，c 、d” → ['a', 'b', 'c', 'd']：聊天与人的 ID 用逗号、顿号或空格分开都行。
export function splitIds(value: string) {
  return [
    ...new Set(
      value
        .split(/[\s,，、]+/)
        .map((id) => id.trim())
        .filter(Boolean),
    ),
  ]
}
