// 通道平台常量和标签工具函数。
import { Bot, MessageCircle, Send } from 'lucide-react'
import type { useI18n } from '@/app/i18n/use-i18n'
import type {
  BadgeTone,
  ChannelPlatform,
  ChannelStatus,
  OnboardingStatus,
  ProviderDefinition,
} from './channel-types'

export const PROVIDERS: Record<ChannelPlatform, ProviderDefinition> = {
  feishu: { Icon: Bot, tone: 'blue' },
  weixin: { Icon: MessageCircle, tone: 'green' },
  qq: { Icon: MessageCircle, tone: 'blue' },
  telegram: { Icon: Send, tone: 'blue' },
}

export const PROVIDER_ENTRIES = Object.entries(PROVIDERS) as Array<
  [ChannelPlatform, ProviderDefinition]
>

type Translate = ReturnType<typeof useI18n>['t']

export function providerName(platform: ChannelPlatform, t: Translate) {
  if (platform === 'feishu') return t('channels:channelsPage.feishu')
  if (platform === 'weixin') return t('channels:channelsPage.weChat')
  if (platform === 'qq') return t('channels:channelsPage.qq')
  return t('channels:channelsPage.telegram')
}

export function providerTitle(platform: ChannelPlatform, t: Translate) {
  if (platform === 'feishu') return t('channels:channelsPage.feishuAppBot')
  if (platform === 'weixin') return t('channels:channelsPage.weChat')
  if (platform === 'qq') return t('channels:channelsPage.qqBot')
  return t('channels:channelsPage.telegramBot')
}

export function providerTransport(platform: ChannelPlatform, t: Translate) {
  if (platform === 'feishu') return t('channels:channelsPage.webSocketPersistentConnection')
  if (platform === 'weixin') return t('channels:channelsPage.tencentILinkPersistentConnection')
  if (platform === 'qq') return t('channels:channelsPage.qqPersistentConnection')
  return t('channels:channelsPage.telegramPersistentConnection')
}

export function providerCapability(platform: ChannelPlatform, t: Translate) {
  if (platform === 'feishu') return t('channels:channelsPage.feishuCapabilities')
  if (platform === 'weixin') return t('channels:channelsPage.weChatCapabilities')
  if (platform === 'qq') return t('channels:channelsPage.qqCapabilities')
  return t('channels:channelsPage.telegramCapabilities')
}

export function isManualPlatform(platform: ChannelPlatform) {
  return platform === 'telegram'
}

export function connectActionLabel(
  platform: ChannelPlatform,
  connected: boolean,
  t: Translate,
) {
  if (connected)
    return t('channels:channelsPage.reconnectName', { name: providerName(platform, t) })
  return isManualPlatform(platform)
    ? t('channels:channelsPage.configureName', { name: providerName(platform, t) })
    : t('channels:channelsPage.connectNameByQRCode', { name: providerName(platform, t) })
}

export function channelStatusLabel(status: ChannelStatus, t: Translate) {
  if (status === 'connecting') return t('channels:channelsPage.connecting')
  if (status === 'connected') return t('channels:channelsPage.online')
  if (status === 'reconnecting') return t('channels:channelsPage.reconnecting')
  if (status === 'failed') return t('channels:channelsPage.connectionFailed')
  return t('channels:channelsPage.notConnected')
}

export function channelStatusTone(status: ChannelStatus): BadgeTone {
  if (status === 'connected') return 'green'
  if (status === 'connecting' || status === 'reconnecting') return 'amber'
  if (status === 'failed') return 'red'
  return 'gray'
}

export function onboardingStatusLabel(status: OnboardingStatus, t: Translate) {
  if (status === 'starting') return t('channels:channelsPage.requestingLoginQrCode')
  if (status === 'waiting') return t('channels:channelsPage.scanAndConfirmConnection')
  if (status === 'scanned') return t('channels:channelsPage.scannedWaitingForConfirmation')
  if (status === 'verification_required') return t('channels:channelsPage.enterPairingCode')
  if (status === 'authorizing') return t('channels:channelsPage.confirmingAuthorization')
  if (status === 'connecting') return t('channels:channelsPage.authorizationSucceededConnecting')
  if (status === 'completed') return t('channels:channelsPage.channelConnected')
  if (status === 'failed') return t('channels:channelsPage.connectionFailed')
  return t('channels:channelsPage.cancelled')
}

export function expiresIn(value: string | number | Date, locale = 'zh-CN') {
  const seconds = Math.max(0, Math.ceil((new Date(value).getTime() - Date.now()) / 1000))
  if (locale === 'en-US')
    return seconds >= 60 ? `in ${Math.ceil(seconds / 60)} min` : `in ${seconds} sec`
  return seconds >= 60 ? `${Math.ceil(seconds / 60)} 分钟后` : `${seconds} 秒后`
}
