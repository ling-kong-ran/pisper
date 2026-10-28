// 通知设置：支持飞书/微信/QQ/Telegram/浏览器五类渠道，每个事件模板声明启停与
// 渠道文案，变量以 {{path}} 占位符在渲染时从事件数据取值。
export const NOTIFICATION_PLATFORMS = ['browser', 'feishu', 'weixin', 'qq', 'telegram'] as const
export type NotificationPlatform = (typeof NOTIFICATION_PLATFORMS)[number]

export type NotificationTemplate = {
  id: string
  name: string
  description: string
  enabled: boolean
  variables: string[]
  channels: Partial<Record<NotificationPlatform, { content: string; enabled?: boolean }>>
}

export type NotificationScope = {
  platform: NotificationPlatform
  [key: string]: unknown
}

export type NotificationSettingsData = {
  browser: { enabled: boolean }
  connections: Partial<
    Record<
      Exclude<NotificationPlatform, 'browser'>,
      { enabled?: boolean; status?: string; [key: string]: unknown } | null
    >
  >
  scopes: NotificationScope[]
  templates: NotificationTemplate[]
}
