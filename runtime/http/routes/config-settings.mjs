// 配置与设置路由：Provider/模型配置、压缩/记忆偏好、通知设置等。
const notificationChannels = ['feishu', 'weixin', 'browser']

function notificationText(value, fallback, maxLength) {
  return (
    String(value || '')
      .trim()
      .slice(0, maxLength) || fallback
  )
}

export const configSettingsRoutes = [
  {
    method: 'GET',
    path: '/api/config',
    async handler({ runtime, json }) {
      json(200, await runtime.getConfig())
    },
  },
  {
    method: 'PUT',
    path: '/api/config',
    async handler({ runtime, body, json }) {
      json(200, await runtime.saveConfig(await body()))
    },
  },
  {
    method: 'GET',
    path: '/api/settings/compaction',
    handler({ runtime, json }) {
      json(200, runtime.getCompactionPreference())
    },
  },
  {
    method: 'PATCH',
    path: '/api/settings/compaction',
    async handler({ runtime, body, json }) {
      json(200, await runtime.updateCompactionPreference(await body()))
    },
  },
  {
    method: 'GET',
    path: '/api/settings/memory',
    handler({ runtime, json }) {
      json(200, runtime.getMemoryPreference())
    },
  },
  {
    method: 'PATCH',
    path: '/api/settings/memory',
    async handler({ runtime, body, json }) {
      json(200, await runtime.updateMemoryPreference(await body()))
    },
  },
  {
    method: 'GET',
    path: '/api/settings/notifications',
    async handler({ runtime, json }) {
      json(200, await runtime.getNotificationSettings())
    },
  },
  {
    method: 'PATCH',
    path: '/api/settings/notifications/browser',
    async handler({ runtime, body, json }) {
      json(200, await runtime.updateBrowserNotifications(await body()))
    },
  },
  {
    method: 'GET',
    path: '/api/settings/notifications/browser/events',
    async handler({ runtime, url, json }) {
      json(200, await runtime.getBrowserNotificationEvents(url.searchParams.get('after') || ''))
    },
  },
  {
    method: 'POST',
    path: '/api/settings/notifications/chat-completed',
    async handler({ runtime, body, json }) {
      const input = await body()
      const settings = await runtime.getNotificationSettings()
      let channelError = ''
      try {
        await runtime.notifyChannels(
          'chat.completed',
          {
            chat: {
              title: notificationText(input.title, 'Pisper conversation', 160),
              summary: notificationText(input.summary, 'The Agent has finished responding.', 1_000),
              model: notificationText(input.model, 'unknown', 160),
            },
          },
          { platforms: ['feishu', 'weixin'] },
        )
      } catch (error) {
        channelError = error instanceof Error ? error.message : String(error)
      }
      json(202, {
        accepted: true,
        systemNotificationEnabled: settings.browser?.enabled === true,
        channelError,
      })
    },
  },
  {
    method: 'POST',
    path: '/api/settings/notifications/chat-waiting',
    async handler({ runtime, body, json }) {
      const input = await body()
      const title = notificationText(input.title, 'Pisper conversation', 160)
      const settings = await runtime.getNotificationSettings()
      let channelError = ''
      try {
        await runtime.notifyChannels(
          'chat.waiting',
          {
            chat: {
              title,
              tool: notificationText(input.tool, 'Agent action', 160),
              reason: notificationText(input.reason, 'Your confirmation is required.', 1_000),
              model: notificationText(input.model, 'unknown', 160),
            },
          },
          { platforms: ['feishu', 'weixin'] },
        )
      } catch (error) {
        channelError = error instanceof Error ? error.message : String(error)
      }
      json(202, {
        accepted: true,
        systemNotificationEnabled: settings.browser?.enabled === true,
        channelError,
      })
    },
  },
  {
    method: 'POST',
    path: '/api/settings/notifications/templates/:templateId/:channel/test',
    where: { channel: notificationChannels },
    async handler({ runtime, params, json }) {
      json(200, await runtime.testNotificationTemplate(params.templateId, params.channel))
    },
  },
  {
    method: 'PUT',
    path: '/api/settings/notifications/templates/:templateId/:channel',
    where: { channel: notificationChannels },
    async handler({ runtime, params, body, json }) {
      json(
        200,
        await runtime.saveNotificationTemplate(params.templateId, params.channel, await body()),
      )
    },
  },
  {
    method: 'GET',
    path: '/api/providers/discovery',
    async handler({ runtime, json }) {
      json(200, await runtime.getProviderDiscovery())
    },
  },
  {
    method: 'POST',
    path: '/api/providers/:providerId/import',
    async handler({ runtime, params, json }) {
      json(200, await runtime.importDiscoveredProvider(params.providerId))
    },
  },
  {
    method: 'POST',
    path: '/api/providers',
    async handler({ runtime, body, json }) {
      json(201, await runtime.createProvider(await body()))
    },
  },
  {
    method: 'POST',
    path: '/api/providers/models/refresh',
    async handler({ runtime, json }) {
      json(200, await runtime.refreshProviderModels())
    },
  },
  {
    method: 'PUT',
    path: '/api/providers/:providerId/connection',
    async handler({ runtime, params, body, json }) {
      json(200, await runtime.setProviderConnection(params.providerId, await body()))
    },
  },
  {
    method: 'PUT',
    path: '/api/providers/:providerId/api-key',
    async handler({ runtime, params, body, json }) {
      json(200, await runtime.setProviderApiKey(params.providerId, await body()))
    },
  },
  {
    method: 'PUT',
    path: '/api/providers/:providerId/enabled',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      json(200, await runtime.setProviderEnabled(params.providerId, Boolean(input.enabled)))
    },
  },
  {
    method: 'POST',
    path: '/api/providers/:providerId/models',
    async handler({ runtime, params, body, json }) {
      json(201, await runtime.addProviderModel(params.providerId, await body()))
    },
  },
  {
    method: 'POST',
    path: '/api/providers/:providerId/models/discover',
    async handler({ runtime, params, body, json }) {
      json(200, await runtime.discoverProviderModels(params.providerId, await body()))
    },
  },
  {
    method: 'POST',
    path: '/api/providers/:providerId/models/batch',
    async handler({ runtime, params, body, json }) {
      json(201, await runtime.addProviderModels(params.providerId, (await body()).models))
    },
  },
  {
    method: 'DELETE',
    path: '/api/providers/:providerId',
    async handler({ runtime, params, json }) {
      const deleted = await runtime.deleteProvider(params.providerId)
      if (!deleted) json(404, { error: 'Provider 不存在。' })
      else json(200, deleted)
    },
  },
]
