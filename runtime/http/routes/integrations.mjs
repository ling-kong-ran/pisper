// 集成路由：飞书/微信渠道的接入引导、消息、通知模板与插件/技能管理。
const channelKinds = ['feishu', 'weixin']

export const integrationRoutes = [
  {
    method: 'GET',
    path: '/api/channels',
    async handler({ runtime, json }) {
      json(200, await runtime.getChannels())
    },
  },
  {
    method: 'POST',
    path: '/api/channels/:channel/onboarding',
    where: { channel: channelKinds },
    async handler({ runtime, params, json }) {
      json(201, await runtime.startChannelOnboarding(params.channel))
    },
  },
  {
    method: 'GET',
    path: '/api/channels/:channel/onboarding/:onboardingId',
    where: { channel: channelKinds },
    handler({ runtime, params, json }) {
      const result = runtime.getChannelOnboarding(params.channel, params.onboardingId)
      if (!result) json(404, { error: '扫码任务不存在或已过期。' })
      else json(200, result)
    },
  },
  {
    method: 'DELETE',
    path: '/api/channels/:channel/onboarding/:onboardingId',
    where: { channel: channelKinds },
    handler({ runtime, params, json }) {
      json(200, {
        cancelled: runtime.cancelChannelOnboarding(params.channel, params.onboardingId),
      })
    },
  },
  {
    method: 'POST',
    path: '/api/channels/:channel/onboarding/:onboardingId/verify',
    where: { channel: channelKinds },
    async handler({ runtime, params, body, json }) {
      const result = runtime.verifyChannelOnboarding(
        params.channel,
        params.onboardingId,
        (await body()).code,
      )
      if (!result) json(404, { error: '扫码任务不存在或已过期。' })
      else json(200, result)
    },
  },
  {
    method: 'POST',
    path: '/api/channels/:channel/reconnect',
    where: { channel: channelKinds },
    async handler({ runtime, params, json }) {
      json(200, await runtime.reconnectChannel(params.channel))
    },
  },
  {
    method: 'PATCH',
    path: '/api/channels/:channel',
    where: { channel: channelKinds },
    async handler({ runtime, params, body, json }) {
      json(200, await runtime.updateChannel(params.channel, await body()))
    },
  },
  {
    method: 'DELETE',
    path: '/api/channels/:channel',
    where: { channel: channelKinds },
    async handler({ runtime, params, json }) {
      await runtime.deleteChannel(params.channel)
      json(200, { deleted: true })
    },
  },
  {
    method: 'DELETE',
    path: '/api/channels/scopes/:scopeId',
    async handler({ runtime, params, json }) {
      const deleted = await runtime.resetChannelScope(params.scopeId)
      if (!deleted) json(404, { error: '渠道会话不存在。' })
      else json(200, { deleted: true })
    },
  },
  {
    method: 'GET',
    path: '/api/plugins',
    async handler({ runtime, url, json }) {
      json(200, await runtime.getPlugins(url.searchParams.get('sessionId') || ''))
    },
  },
  {
    method: 'POST',
    path: '/api/plugins/inspect',
    async handler({ runtime, body, json }) {
      json(200, await runtime.inspectPlugin(await body()))
    },
  },
  {
    method: 'POST',
    path: '/api/plugins/install',
    async handler({ runtime, body, json }) {
      json(201, await runtime.installPlugin(await body()))
    },
  },
  {
    method: 'PATCH',
    path: '/api/plugins/:pluginId',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      if (typeof input.enabled !== 'boolean') throw new Error('插件启用状态无效。')
      json(200, await runtime.setPluginEnabled(params.pluginId, input.enabled))
    },
  },
  {
    method: 'PATCH',
    path: '/api/plugins/:pluginId/capabilities/:capabilityName',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      if (typeof input.enabled !== 'boolean') throw new Error('插件能力启用状态无效。')
      json(
        200,
        await runtime.setPluginCapabilityEnabled(
          params.pluginId,
          params.capabilityName,
          input.enabled,
        ),
      )
    },
  },
  {
    method: 'DELETE',
    path: '/api/plugins/:pluginId',
    async handler({ runtime, params, json }) {
      json(200, await runtime.uninstallPlugin(params.pluginId))
    },
  },
  {
    method: 'POST',
    path: '/api/plugins/web-search/test',
    async handler({ runtime, body, json }) {
      const result = await runtime.testWebSearch(await body())
      json(200, { count: result.results.length, provider: result.provider })
    },
  },
  {
    method: 'PUT',
    path: '/api/plugins',
    async handler({ runtime, body, json }) {
      json(200, await runtime.savePlugins(await body()))
    },
  },
  {
    method: 'GET',
    path: '/api/mcp',
    async handler({ runtime, url, json }) {
      json(200, await runtime.getMcpDashboard({ refresh: url.searchParams.get('refresh') !== '0' }))
    },
  },
  {
    method: 'POST',
    path: '/api/mcp',
    async handler({ runtime, body, json }) {
      json(201, await runtime.createMcpServer(await body()))
    },
  },
  {
    method: 'POST',
    path: '/api/mcp/:serverId/test',
    async handler({ runtime, params, json }) {
      json(200, await runtime.testMcpServer(params.serverId))
    },
  },
  {
    method: 'PATCH',
    path: '/api/mcp/:serverId/tools/:toolName',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      if (typeof input.enabled !== 'boolean') throw new Error('MCP 工具启用状态无效。')
      const result = await runtime.setMcpToolEnabled(
        params.serverId,
        params.toolName,
        input.enabled,
      )
      if (!result) json(404, { error: 'MCP 服务或工具不存在。' })
      else json(200, result)
    },
  },
  {
    method: 'PATCH',
    path: '/api/mcp/:serverId',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      if ('enabled' in input && typeof input.enabled !== 'boolean') {
        throw new Error('MCP 服务启用状态无效。')
      }
      const result = await runtime.updateMcpServer(params.serverId, input)
      if (!result) json(404, { error: 'MCP 服务不存在。' })
      else json(200, result)
    },
  },
  {
    method: 'DELETE',
    path: '/api/mcp/:serverId',
    async handler({ runtime, params, json }) {
      const deleted = await runtime.deleteMcpServer(params.serverId)
      if (!deleted) json(404, { error: 'MCP 服务不存在。' })
      else json(200, { deleted: true })
    },
  },
  {
    method: 'GET',
    path: '/api/skills',
    async handler({ runtime, url, json }) {
      json(200, await runtime.getSkillsDashboard(url.searchParams.get('sessionId') || ''))
    },
  },
  {
    method: 'POST',
    path: '/api/skills/install',
    async handler({ runtime, url, body, json }) {
      json(201, await runtime.installSkill(await body(), url.searchParams.get('sessionId') || ''))
    },
  },
  {
    method: 'POST',
    path: '/api/skills/reload',
    async handler({ runtime, url, json }) {
      json(200, await runtime.reloadSkills(url.searchParams.get('sessionId') || ''))
    },
  },
  {
    method: 'PATCH',
    path: '/api/skills/:skillName',
    async handler({ runtime, params, url, body, json }) {
      const input = await body()
      if ('enabled' in input && typeof input.enabled !== 'boolean') {
        throw new Error('技能启用状态无效。')
      }
      if ('modelInvocationEnabled' in input && typeof input.modelInvocationEnabled !== 'boolean') {
        throw new Error('技能自动调用状态无效。')
      }
      const result = await runtime.updateSkill(
        params.skillName,
        input,
        url.searchParams.get('sessionId') || '',
      )
      if (!result) json(404, { error: '技能不存在。' })
      else json(200, result)
    },
  },
  {
    method: 'DELETE',
    path: '/api/skills/:skillName',
    async handler({ runtime, params, url, json }) {
      const deleted = await runtime.deleteSkill(
        params.skillName,
        url.searchParams.get('sessionId') || '',
      )
      if (!deleted) json(404, { error: '技能不存在。' })
      else json(200, { deleted: true })
    },
  },
]
