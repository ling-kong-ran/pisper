import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'
import { NOTIFICATION_PLATFORMS } from '@/types/notifications'
import type {
  NotificationPlatform,
  NotificationSettingsData,
  NotificationTemplate,
} from '@/types/notifications'

export type NotificationTestResult = {
  sent: number
  title?: string
  body?: string
  preview?: string
}
type Options = { signal?: AbortSignal }
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalidResponseError()
  return value as Record<string, unknown>
}
function text(value: unknown, limit: number): string {
  if (typeof value !== 'string' || value.length > limit) throw invalidResponseError()
  return value
}
export function parseNotificationSettings(value: unknown): NotificationSettingsData {
  const input = object(value)
  const browser = object(input.browser)
  if (
    typeof browser.enabled !== 'boolean' ||
    !Array.isArray(input.templates) ||
    !Array.isArray(input.scopes)
  )
    throw invalidResponseError()
  const connections: NotificationSettingsData['connections'] = {}
  const rawConnections = object(input.connections)
  for (const platform of NOTIFICATION_PLATFORMS) {
    if (platform === 'browser' || rawConnections[platform] === undefined) continue
    const raw = rawConnections[platform]
    if (raw === null) {
      connections[platform] = null
      continue
    }
    const connection = object(raw)
    if (
      (connection.enabled !== undefined && typeof connection.enabled !== 'boolean') ||
      (connection.status !== undefined && typeof connection.status !== 'string')
    )
      throw invalidResponseError()
    connections[platform] = {
      ...connection,
      enabled: connection.enabled as boolean | undefined,
      status: connection.status as string | undefined,
    }
  }
  const ids = new Set<string>()
  const templates = input.templates.map((raw): NotificationTemplate => {
    const template = object(raw)
    const id = text(template.id, 200)
    if (
      !id ||
      ids.has(id) ||
      typeof template.enabled !== 'boolean' ||
      !Array.isArray(template.variables)
    )
      throw invalidResponseError()
    ids.add(id)
    const channels: NotificationTemplate['channels'] = {}
    const rawChannels = object(template.channels)
    for (const platform of NOTIFICATION_PLATFORMS) {
      if (rawChannels[platform] === undefined) continue
      const channel = object(rawChannels[platform])
      if (channel.enabled !== undefined && typeof channel.enabled !== 'boolean')
        throw invalidResponseError()
      channels[platform] = {
        content: text(channel.content, 12_000),
        ...(channel.enabled === undefined ? {} : { enabled: channel.enabled }),
      }
    }
    return {
      id,
      name: text(template.name, 500),
      description: text(template.description, 2000),
      enabled: template.enabled,
      variables: template.variables.map((variable) => text(variable, 200)),
      channels,
    }
  })
  const scopes = input.scopes.map((raw) => {
    const scope = object(raw)
    if (!NOTIFICATION_PLATFORMS.includes(scope.platform as NotificationPlatform))
      throw invalidResponseError()
    return { ...scope, platform: scope.platform as NotificationPlatform }
  })
  return { browser: { enabled: browser.enabled }, connections, scopes, templates }
}
function parseTestResult(value: unknown): NotificationTestResult {
  const input = object(value)
  if (typeof input.sent !== 'number' || !Number.isInteger(input.sent) || input.sent < 0)
    throw invalidResponseError()
  return {
    sent: input.sent,
    ...(input.title === undefined ? {} : { title: text(input.title, 2000) }),
    ...(input.body === undefined ? {} : { body: text(input.body, 12_000) }),
    ...(input.preview === undefined ? {} : { preview: text(input.preview, 12_000) }),
  }
}
export function supportsNotificationChannelPreferences(data: NotificationSettingsData) {
  return (
    data.templates.length > 0 &&
    data.templates.every((template) =>
      NOTIFICATION_PLATFORMS.every(
        (platform) => typeof template.channels[platform]?.enabled === 'boolean',
      ),
    )
  )
}
const endpoint = (event: string, platform: NotificationPlatform) =>
  `/api/settings/notifications/templates/${encodeURIComponent(event)}/${platform}`
const update = (event: string, platform: NotificationPlatform, body: unknown, options: Options) =>
  requestJson(endpoint(event, platform), {
    method: 'PUT',
    body,
    ...options,
    parse: parseNotificationSettings,
  })
export const notificationApi = {
  load: (options: Options = {}) =>
    requestJson('/api/settings/notifications', { ...options, parse: parseNotificationSettings }),
  setBrowserEnabled: (enabled: boolean, options: Options = {}) =>
    requestJson('/api/settings/notifications/browser', {
      method: 'PATCH',
      body: { enabled },
      ...options,
      parse: parseNotificationSettings,
    }),
  setEventEnabled: (event: string, enabled: boolean, options: Options = {}) =>
    update(event, 'browser', { enabled }, options),
  setChannelEnabled: async (
    event: string,
    platform: NotificationPlatform,
    enabled: boolean,
    options: Options = {},
  ) => {
    const data = await update(event, platform, { channelEnabled: enabled }, options)
    if (data.templates.find((item) => item.id === event)?.channels[platform]?.enabled !== enabled)
      throw invalidResponseError()
    return data
  },
  saveTemplate: (
    event: string,
    platform: NotificationPlatform,
    content: string,
    options: Options = {},
  ) => update(event, platform, { content }, options),
  testSavedTemplate: (event: string, platform: NotificationPlatform, options: Options = {}) =>
    requestJson(`${endpoint(event, platform)}/test`, {
      method: 'POST',
      body: {},
      ...options,
      parse: parseTestResult,
    }),
}
