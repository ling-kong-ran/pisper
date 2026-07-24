export type NotificationPlatform = 'feishu' | 'weixin' | 'browser'

export type NotificationTemplate = {
  id: string
  name: string
  description: string
  enabled: boolean
  variables: string[]
  channels: Partial<Record<NotificationPlatform, { content: string }>>
}

export type NotificationScope = {
  platform: NotificationPlatform
  [key: string]: unknown
}

export type NotificationSettingsData = {
  browser: { enabled: boolean }
  connections: Partial<Record<Exclude<NotificationPlatform, 'browser'>, unknown>>
  scopes: NotificationScope[]
  templates: NotificationTemplate[]
}
