// 角色名与设置项的文案（键必须是字面量，i18n 检查逐个核对）。
import type { Translate } from '@/app/navigation'
import type { BotCharacter } from './avatar'
import type { Bot } from './bots-api'

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

export function deliveryLabel(t: Translate, delivery: Bot['delivery']) {
  if (delivery === 'reply-only') return t('bots:details.deliveryReply')
  if (delivery === 'commit') return t('bots:details.deliveryCommit')
  return t('bots:details.deliveryDiff')
}
