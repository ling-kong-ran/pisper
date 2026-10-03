// 通道领域类型：平台、连接状态和引导流程。
import type { LucideIcon } from 'lucide-react'

export type ChannelPlatform = 'feishu' | 'weixin' | 'qq' | 'telegram'
export type ChannelStatus = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'failed'
export type OnboardingStatus =
  | 'starting'
  | 'waiting'
  | 'scanned'
  | 'verification_required'
  | 'authorizing'
  | 'connecting'
  | 'completed'
  | 'failed'
  | 'cancelled'
export type BadgeTone = 'blue' | 'green' | 'red' | 'amber' | 'gray'
export type ProviderDefinition = {
  Icon: LucideIcon
  tone: 'blue' | 'green'
}
export type ReplyModel = { provider: string; model: string }
export type ChannelConnection = {
  enabled: boolean
  status: ChannelStatus
  defaultCwd: string
  replyModel: ReplyModel | null
  executionMode: 'approval-required' | 'workspace-write' | 'full-access'
  runMode: 'plan' | 'goal' | 'team'
  accessMode: 'owner' | 'all'
  ownerConfigured: boolean
  lastError?: string
  connectedAt?: string | null
}
export type ChannelScope = {
  key: string
  platform: ChannelPlatform
  title: string
  lastMessage?: string
  updatedAt?: string
  model?: string
  cwd?: string
}
export type ChannelModel = { provider: string; model: string; label: string }
export type ChannelsData = {
  providers: Array<Record<string, unknown>>
  connections: Record<ChannelPlatform, ChannelConnection | null>
  scopes: ChannelScope[]
  models: ChannelModel[]
}
export type OnboardingJob = {
  id?: string
  platform: ChannelPlatform
  status: OnboardingStatus
  error?: string
  qrDataUrl?: string
  qrUrl?: string
  setupUrl?: string
  expireAt?: string
  needsVerifyCode?: boolean
  manual?: boolean
  mode?: 'manual' | 'qr'
  fields?: string[]
  required?: string[]
}
export type ManualCredentials = {
  appId?: string
  appSecret?: string
  token?: string
}
export type ManualOnboardingDescriptor = {
  mode: 'manual'
  platform: ChannelPlatform
  fields: string[]
  required: string[]
  setupUrl?: string
  qrDataUrl?: string
}
