import { bodyJson, json, sseSend } from './response.mjs'
import { redactSecretText } from '../security/secret-redaction.mjs'

function publicError(error) {
  return redactSecretText(error instanceof Error ? error.message : String(error))
}

export function createApiHandler(
  runtime,
  { updates, sponsors, desktopPet, engineVersion = 'unknown' } = {},
) {
  return async function handleApi(req, res, url) {
    if (!url.pathname.startsWith('/api/')) return false
    try {
      if (req.method === 'GET' && url.pathname === '/api/health') {
        json(res, 200, { ok: true, engine: '@earendil-works/pi-coding-agent', version: engineVersion })
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/app-update') {
        if (!updates) throw new Error('更新检查服务尚未初始化。')
        try {
          json(
            res,
            200,
            await updates.check({ refresh: url.searchParams.get('refresh') === '1' }),
          )
        } catch (error) {
          json(res, 502, { error: publicError(error) })
        }
        return true
      }
      const sponsorMatch = url.pathname.match(/^\/api\/sponsors\/([^/]+)$/)
      if (req.method === 'GET' && sponsorMatch) {
        if (!sponsors) throw new Error('赞助内容服务尚未初始化。')
        json(
          res,
          200,
          await sponsors.getPlacement(decodeURIComponent(sponsorMatch[1]), {
            locale: url.searchParams.get('locale') || 'zh-CN',
            refresh: url.searchParams.get('refresh') === '1',
          }),
        )
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/config') {
        json(res, 200, await runtime.getConfig())
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/desktop-pet') {
        if (!desktopPet) throw new Error('桌面宠物服务尚未初始化。')
        json(res, 200, desktopPet.status())
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/desktop-pet/catalog') {
        if (!desktopPet) throw new Error('桌面宠物服务尚未初始化。')
        json(res, 200, await desktopPet.search(url.searchParams.get('query') || ''))
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/desktop-pet/enabled') {
        if (!desktopPet) throw new Error('桌面宠物服务尚未初始化。')
        const body = await bodyJson(req)
        json(res, 200, desktopPet.setEnabled(Boolean(body?.enabled)))
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/desktop-pet/opacity') {
        if (!desktopPet) throw new Error('桌面宠物服务尚未初始化。')
        const body = await bodyJson(req)
        json(res, 200, desktopPet.setOpacity(body?.opacity))
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/desktop-pet/install') {
        if (!desktopPet) throw new Error('桌面宠物服务尚未初始化。')
        const body = await bodyJson(req)
        json(res, 200, await desktopPet.install(body?.slug))
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/desktop-pet/select') {
        if (!desktopPet) throw new Error('桌面宠物服务尚未初始化。')
        const body = await bodyJson(req)
        json(res, 200, desktopPet.select(body?.slug))
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/desktop-pet/sprite') {
        if (!desktopPet) throw new Error('桌面宠物服务尚未初始化。')
        const sprite = desktopPet.sprite(url.searchParams.get('slug') || '')
        if (!sprite) {
          json(res, 404, { error: '宠物资源不存在。' })
          return true
        }
        res.writeHead(200, {
          'Content-Type': sprite.mime,
          'Content-Length': sprite.buffer.length,
          'Cache-Control': 'private, max-age=86400',
          'X-Content-Type-Options': 'nosniff',
        })
        res.end(sprite.buffer)
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/settings/compaction') {
        json(res, 200, runtime.getCompactionPreference())
        return true
      }
      if (req.method === 'PATCH' && url.pathname === '/api/settings/compaction') {
        json(res, 200, await runtime.updateCompactionPreference(await bodyJson(req)))
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/settings/notifications') {
        json(res, 200, await runtime.getNotificationSettings())
        return true
      }
      if (req.method === 'PATCH' && url.pathname === '/api/settings/notifications/browser') {
        json(res, 200, await runtime.updateBrowserNotifications(await bodyJson(req)))
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/settings/notifications/browser/events') {
        json(res, 200, await runtime.getBrowserNotificationEvents(url.searchParams.get('after') || ''))
        return true
      }
      const notificationTemplateTestMatch = url.pathname.match(/^\/api\/settings\/notifications\/templates\/([^/]+)\/(feishu|weixin|browser)\/test$/)
      if (req.method === 'POST' && notificationTemplateTestMatch) {
        json(res, 200, await runtime.testNotificationTemplate(decodeURIComponent(notificationTemplateTestMatch[1]), notificationTemplateTestMatch[2]))
        return true
      }
      const notificationTemplateMatch = url.pathname.match(/^\/api\/settings\/notifications\/templates\/([^/]+)\/(feishu|weixin|browser)$/)
      if (req.method === 'PUT' && notificationTemplateMatch) {
        json(res, 200, await runtime.saveNotificationTemplate(decodeURIComponent(notificationTemplateMatch[1]), notificationTemplateMatch[2], await bodyJson(req)))
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/usage/today') {
        json(res, 200, await runtime.getTodayUsage())
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/schedules') {
        json(res, 200, await runtime.getSchedules())
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/schedules') {
        json(res, 201, await runtime.createSchedule(await bodyJson(req)))
        return true
      }
      const scheduleRunMatch = url.pathname.match(/^\/api\/schedules\/([^/]+)\/run$/)
      if (req.method === 'POST' && scheduleRunMatch) {
        const result = await runtime.runSchedule(decodeURIComponent(scheduleRunMatch[1]))
        if (!result) json(res, 404, { error: '定时任务不存在。' })
        else json(res, 202, result)
        return true
      }
      const scheduleMatch = url.pathname.match(/^\/api\/schedules\/([^/]+)$/)
      if (req.method === 'PATCH' && scheduleMatch) {
        const result = await runtime.updateSchedule(decodeURIComponent(scheduleMatch[1]), await bodyJson(req))
        if (!result) json(res, 404, { error: '定时任务不存在。' })
        else json(res, 200, result)
        return true
      }
      if (req.method === 'DELETE' && scheduleMatch) {
        const deleted = await runtime.deleteSchedule(decodeURIComponent(scheduleMatch[1]))
        if (!deleted) json(res, 404, { error: '定时任务不存在。' })
        else json(res, 200, { deleted: true })
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/workflows') {
        json(res, 200, await runtime.getWorkflows())
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/workflows') {
        json(res, 201, await runtime.createWorkflow(await bodyJson(req)))
        return true
      }
      const workflowRunStopMatch = url.pathname.match(/^\/api\/workflows\/runs\/([^/]+)\/stop$/)
      if (req.method === 'POST' && workflowRunStopMatch) {
        const result = await runtime.stopWorkflowRun(decodeURIComponent(workflowRunStopMatch[1]))
        if (!result) json(res, 404, { error: '工作流运行不存在或已经结束。' })
        else json(res, 202, result)
        return true
      }
      const workflowRunMatch = url.pathname.match(/^\/api\/workflows\/([^/]+)\/run$/)
      if (req.method === 'POST' && workflowRunMatch) {
        const result = await runtime.runWorkflow(decodeURIComponent(workflowRunMatch[1]))
        if (!result) json(res, 404, { error: '工作流不存在。' })
        else json(res, 202, result)
        return true
      }
      const workflowMatch = url.pathname.match(/^\/api\/workflows\/([^/]+)$/)
      if (req.method === 'PATCH' && workflowMatch) {
        const result = await runtime.updateWorkflow(decodeURIComponent(workflowMatch[1]), await bodyJson(req))
        if (!result) json(res, 404, { error: '工作流不存在。' })
        else json(res, 200, result)
        return true
      }
      if (req.method === 'DELETE' && workflowMatch) {
        const deleted = await runtime.deleteWorkflow(decodeURIComponent(workflowMatch[1]))
        if (!deleted) json(res, 404, { error: '工作流不存在。' })
        else json(res, 200, { deleted: true })
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/directories') {
        json(res, 200, await runtime.listDirectories(url.searchParams.get('path')))
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/assets') {
        json(res, 200, { assets: await runtime.listAssets({ query: url.searchParams.get('query'), kind: url.searchParams.get('kind'), sessionId: url.searchParams.get('sessionId') }) })
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/memory') {
        json(res, 200, runtime.getMemoryDashboard({
          query: url.searchParams.get('query') || '',
          spaceId: url.searchParams.get('spaceId') || '',
        }))
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/memory/candidates') {
        json(res, 200, runtime.getMemoryCandidateInbox({ limit: url.searchParams.get('limit') }))
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/memory/spaces') {
        json(res, 201, runtime.createMemorySpace(await bodyJson(req)))
        return true
      }
      const memorySpaceMatch = url.pathname.match(/^\/api\/memory\/spaces\/([^/]+)$/)
      if (req.method === 'PATCH' && memorySpaceMatch) {
        const updated = runtime.updateMemorySpace(decodeURIComponent(memorySpaceMatch[1]), await bodyJson(req))
        if (!updated) json(res, 404, { error: '星域不存在。' })
        else json(res, 200, updated)
        return true
      }
      if (req.method === 'DELETE' && memorySpaceMatch) {
        const deleted = runtime.deleteMemorySpace(decodeURIComponent(memorySpaceMatch[1]))
        if (!deleted) json(res, 404, { error: '星域不存在。' })
        else json(res, 200, { deleted: true })
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/memory/nodes') {
        json(res, 201, runtime.createMemory(await bodyJson(req)))
        return true
      }
      const memoryNodeMatch = url.pathname.match(/^\/api\/memory\/nodes\/([^/]+)$/)
      if (req.method === 'PATCH' && memoryNodeMatch) {
        const updated = runtime.updateMemory(decodeURIComponent(memoryNodeMatch[1]), await bodyJson(req))
        if (!updated) json(res, 404, { error: '星辰不存在。' })
        else json(res, 200, updated)
        return true
      }
      if (req.method === 'DELETE' && memoryNodeMatch) {
        const deleted = runtime.deleteMemory(decodeURIComponent(memoryNodeMatch[1]))
        if (!deleted) json(res, 404, { error: '星辰不存在。' })
        else json(res, 200, { deleted: true })
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/memory/candidates/reject-all') {
        json(res, 200, runtime.rejectAllMemoryCandidates())
        return true
      }
      const memoryCandidateActionMatch = url.pathname.match(/^\/api\/memory\/candidates\/([^/]+)\/(accept|reject)$/)
      if (req.method === 'POST' && memoryCandidateActionMatch) {
        const id = decodeURIComponent(memoryCandidateActionMatch[1])
        const result = memoryCandidateActionMatch[2] === 'accept'
          ? runtime.acceptMemoryCandidate(id)
          : runtime.rejectMemoryCandidate(id)
        if (!result) json(res, 404, { error: '候选记忆不存在或已处理。' })
        else json(res, 200, result)
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/channels') {
        json(res, 200, await runtime.getChannels())
        return true
      }
      const onboardingStartMatch = url.pathname.match(/^\/api\/channels\/(feishu|weixin)\/onboarding$/)
      if (req.method === 'POST' && onboardingStartMatch) {
        json(res, 201, await runtime.startChannelOnboarding(onboardingStartMatch[1]))
        return true
      }
      const onboardingMatch = url.pathname.match(/^\/api\/channels\/(feishu|weixin)\/onboarding\/([^/]+)$/)
      if (req.method === 'GET' && onboardingMatch) {
        const result = runtime.getChannelOnboarding(onboardingMatch[1], decodeURIComponent(onboardingMatch[2]))
        if (!result) json(res, 404, { error: '扫码任务不存在或已过期。' })
        else json(res, 200, result)
        return true
      }
      if (req.method === 'DELETE' && onboardingMatch) {
        json(res, 200, { cancelled: runtime.cancelChannelOnboarding(onboardingMatch[1], decodeURIComponent(onboardingMatch[2])) })
        return true
      }
      const onboardingVerifyMatch = url.pathname.match(/^\/api\/channels\/(feishu|weixin)\/onboarding\/([^/]+)\/verify$/)
      if (req.method === 'POST' && onboardingVerifyMatch) {
        const result = runtime.verifyChannelOnboarding(onboardingVerifyMatch[1], decodeURIComponent(onboardingVerifyMatch[2]), (await bodyJson(req)).code)
        if (!result) json(res, 404, { error: '扫码任务不存在或已过期。' })
        else json(res, 200, result)
        return true
      }
      const reconnectMatch = url.pathname.match(/^\/api\/channels\/(feishu|weixin)\/reconnect$/)
      if (req.method === 'POST' && reconnectMatch) {
        json(res, 200, await runtime.reconnectChannel(reconnectMatch[1]))
        return true
      }
      const channelMatch = url.pathname.match(/^\/api\/channels\/(feishu|weixin)$/)
      if (req.method === 'PATCH' && channelMatch) {
        json(res, 200, await runtime.updateChannel(channelMatch[1], await bodyJson(req)))
        return true
      }
      if (req.method === 'DELETE' && channelMatch) {
        await runtime.deleteChannel(channelMatch[1])
        json(res, 200, { deleted: true })
        return true
      }
      const channelScopeMatch = url.pathname.match(/^\/api\/channels\/scopes\/([^/]+)$/)
      if (req.method === 'DELETE' && channelScopeMatch) {
        const deleted = await runtime.resetChannelScope(decodeURIComponent(channelScopeMatch[1]))
        if (!deleted) json(res, 404, { error: '渠道会话不存在。' })
        else json(res, 200, { deleted: true })
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/assets') {
        json(res, 201, await runtime.createAsset(await bodyJson(req)))
        return true
      }
      const assetContentMatch = url.pathname.match(/^\/api\/assets\/([^/]+)\/content$/)
      if (req.method === 'GET' && assetContentMatch) {
        const content = await runtime.getAssetContent(decodeURIComponent(assetContentMatch[1]))
        if (!content) json(res, 404, { error: '资产不存在。' })
        else json(res, 200, content)
        return true
      }
      const assetDownloadMatch = url.pathname.match(/^\/api\/assets\/([^/]+)\/download$/)
      if (req.method === 'GET' && assetDownloadMatch) {
        const download = await runtime.getAssetDownload(decodeURIComponent(assetDownloadMatch[1]))
        if (!download) json(res, 404, { error: '资产不存在或不可下载。' })
        else {
          res.writeHead(200, {
            'Content-Type': download.asset.mimeType || 'application/octet-stream',
            'Content-Length': download.buffer.length,
            'Content-Disposition': `${url.searchParams.get('inline') === '1' ? 'inline' : 'attachment'}; filename*=UTF-8''${encodeURIComponent(download.asset.name)}`,
            'Cache-Control': 'private, max-age=60',
          })
          res.end(download.buffer)
        }
        return true
      }
      const assetMatch = url.pathname.match(/^\/api\/assets\/([^/]+)$/)
      if (req.method === 'DELETE' && assetMatch) {
        const deleted = await runtime.deleteAsset(decodeURIComponent(assetMatch[1]))
        if (!deleted) json(res, 404, { error: '资产不存在。' })
        else json(res, 200, { deleted: true })
        return true
      }
      if (req.method === 'PUT' && url.pathname === '/api/config') {
        json(res, 200, await runtime.saveConfig(await bodyJson(req)))
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/plugins') {
        json(res, 200, await runtime.getPlugins())
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/plugins/web-search/test') {
        const result = await runtime.testWebSearch(await bodyJson(req))
        json(res, 200, { count: result.results.length, provider: result.provider })
        return true
      }
      if (req.method === 'PUT' && url.pathname === '/api/plugins') {
        json(res, 200, await runtime.savePlugins(await bodyJson(req)))
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/mcp') {
        json(res, 200, await runtime.getMcpDashboard({ refresh: url.searchParams.get('refresh') !== '0' }))
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/mcp') {
        json(res, 201, await runtime.createMcpServer(await bodyJson(req)))
        return true
      }
      const mcpTestMatch = url.pathname.match(/^\/api\/mcp\/([^/]+)\/test$/)
      if (req.method === 'POST' && mcpTestMatch) {
        json(res, 200, await runtime.testMcpServer(decodeURIComponent(mcpTestMatch[1])))
        return true
      }
      const mcpToolMatch = url.pathname.match(/^\/api\/mcp\/([^/]+)\/tools\/([^/]+)$/)
      if (req.method === 'PATCH' && mcpToolMatch) {
        const body = await bodyJson(req)
        if (typeof body.enabled !== 'boolean') throw new Error('MCP 工具启用状态无效。')
        const result = await runtime.setMcpToolEnabled(
          decodeURIComponent(mcpToolMatch[1]),
          decodeURIComponent(mcpToolMatch[2]),
          body.enabled,
        )
        if (!result) json(res, 404, { error: 'MCP 服务或工具不存在。' })
        else json(res, 200, result)
        return true
      }
      const mcpMatch = url.pathname.match(/^\/api\/mcp\/([^/]+)$/)
      if (req.method === 'PATCH' && mcpMatch) {
        const body = await bodyJson(req)
        if ('enabled' in body && typeof body.enabled !== 'boolean') throw new Error('MCP 服务启用状态无效。')
        const result = await runtime.updateMcpServer(decodeURIComponent(mcpMatch[1]), body)
        if (!result) json(res, 404, { error: 'MCP 服务不存在。' })
        else json(res, 200, result)
        return true
      }
      if (req.method === 'DELETE' && mcpMatch) {
        const deleted = await runtime.deleteMcpServer(decodeURIComponent(mcpMatch[1]))
        if (!deleted) json(res, 404, { error: 'MCP 服务不存在。' })
        else json(res, 200, { deleted: true })
        return true
      }
      const skillSessionId = url.searchParams.get('sessionId') || ''
      if (req.method === 'GET' && url.pathname === '/api/skills') {
        json(res, 200, await runtime.getSkillsDashboard(skillSessionId))
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/skills/install') {
        json(res, 201, await runtime.installSkill(await bodyJson(req), skillSessionId))
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/skills/reload') {
        json(res, 200, await runtime.reloadSkills(skillSessionId))
        return true
      }
      const skillMatch = url.pathname.match(/^\/api\/skills\/([^/]+)$/)
      if (req.method === 'PATCH' && skillMatch) {
        const body = await bodyJson(req)
        if ('enabled' in body && typeof body.enabled !== 'boolean') throw new Error('技能启用状态无效。')
        if ('modelInvocationEnabled' in body && typeof body.modelInvocationEnabled !== 'boolean') throw new Error('技能自动调用状态无效。')
        const result = await runtime.updateSkill(decodeURIComponent(skillMatch[1]), body, skillSessionId)
        if (!result) json(res, 404, { error: '技能不存在。' })
        else json(res, 200, result)
        return true
      }
      if (req.method === 'DELETE' && skillMatch) {
        const deleted = await runtime.deleteSkill(decodeURIComponent(skillMatch[1]), skillSessionId)
        if (!deleted) json(res, 404, { error: '技能不存在。' })
        else json(res, 200, { deleted: true })
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/providers/discovery') {
        json(res, 200, await runtime.getProviderDiscovery())
        return true
      }
      const providerImportMatch = url.pathname.match(/^\/api\/providers\/([^/]+)\/import$/)
      if (req.method === 'POST' && providerImportMatch) {
        json(res, 200, await runtime.importDiscoveredProvider(decodeURIComponent(providerImportMatch[1])))
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/providers') {
        json(res, 201, await runtime.createProvider(await bodyJson(req)))
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/providers/models/refresh') {
        json(res, 200, await runtime.refreshProviderModels())
        return true
      }
      const providerEnabledMatch = url.pathname.match(/^\/api\/providers\/([^/]+)\/enabled$/)
      if (req.method === 'PUT' && providerEnabledMatch) {
        const body = await bodyJson(req)
        json(res, 200, await runtime.setProviderEnabled(decodeURIComponent(providerEnabledMatch[1]), Boolean(body.enabled)))
        return true
      }
      const providerModelsMatch = url.pathname.match(/^\/api\/providers\/([^/]+)\/models$/)
      if (req.method === 'POST' && providerModelsMatch) {
        json(res, 201, await runtime.addProviderModel(decodeURIComponent(providerModelsMatch[1]), await bodyJson(req)))
        return true
      }
      const providerModelDiscoveryMatch = url.pathname.match(/^\/api\/providers\/([^/]+)\/models\/discover$/)
      if (req.method === 'POST' && providerModelDiscoveryMatch) {
        json(res, 200, await runtime.discoverProviderModels(decodeURIComponent(providerModelDiscoveryMatch[1]), await bodyJson(req)))
        return true
      }
      const providerModelsBatchMatch = url.pathname.match(/^\/api\/providers\/([^/]+)\/models\/batch$/)
      if (req.method === 'POST' && providerModelsBatchMatch) {
        const body = await bodyJson(req)
        json(res, 201, await runtime.addProviderModels(decodeURIComponent(providerModelsBatchMatch[1]), body.models))
        return true
      }
      const providerMatch = url.pathname.match(/^\/api\/providers\/([^/]+)$/)
      if (req.method === 'DELETE' && providerMatch) {
        const deleted = await runtime.deleteProvider(decodeURIComponent(providerMatch[1]))
        if (!deleted) json(res, 404, { error: 'Provider 不存在。' })
        else json(res, 200, deleted)
        return true
      }
      if (req.method === 'GET' && url.pathname === '/api/sessions') {
        json(res, 200, { sessions: await runtime.listSessions() })
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/sessions') {
        const body = await bodyJson(req)
        json(res, 201, await runtime.createSession(body.name, body.cwd))
        return true
      }
      const sessionModelMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/model$/)
      if (req.method === 'PUT' && sessionModelMatch) {
        const body = await bodyJson(req)
        json(res, 200, await runtime.setSessionModel(decodeURIComponent(sessionModelMatch[1]), String(body.provider || ''), String(body.model || '')))
        return true
      }
      const sessionThinkingMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/thinking-level$/)
      if (req.method === 'GET' && sessionThinkingMatch) {
        json(res, 200, await runtime.getSessionThinkingState(decodeURIComponent(sessionThinkingMatch[1])))
        return true
      }
      if (req.method === 'PUT' && sessionThinkingMatch) {
        const body = await bodyJson(req)
        json(res, 200, await runtime.setSessionThinkingLevel(decodeURIComponent(sessionThinkingMatch[1]), String(body.level || '')))
        return true
      }
      const sessionCwdMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/cwd$/)
      if (req.method === 'PUT' && sessionCwdMatch) {
        const body = await bodyJson(req)
        const updated = await runtime.setSessionCwd(decodeURIComponent(sessionCwdMatch[1]), body.cwd)
        if (!updated) json(res, 404, { error: '会话不存在。' })
        else json(res, 200, updated)
        return true
      }
      const sessionGoalMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/goal$/)
      if (req.method === 'GET' && sessionGoalMatch) {
        const goal = runtime.getSessionGoal(decodeURIComponent(sessionGoalMatch[1]))
        if (!goal) json(res, 404, { error: '当前会话没有 Goal。' })
        else json(res, 200, { goal })
        return true
      }
      if (req.method === 'PATCH' && sessionGoalMatch) {
        const id = decodeURIComponent(sessionGoalMatch[1])
        const body = await bodyJson(req)
        if (body.action === 'pause') {
          const goal = await runtime.pauseSessionGoal(id)
          if (!goal) json(res, 404, { error: '当前会话没有进行中的 Goal。' })
          else json(res, 200, { goal })
        } else if (body.action === 'set-budget') {
          const goal = await runtime.setSessionGoalBudget(id, body.tokenBudget)
          json(res, 200, { goal })
        } else {
          throw new Error('Goal 操作无效。')
        }
        return true
      }
      const sessionGitMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/git\/changes$/)
      if (req.method === 'GET' && sessionGitMatch) {
        json(res, 200, await runtime.getSessionGitChanges(decodeURIComponent(sessionGitMatch[1])))
        return true
      }
      const sessionGitCommitMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/git\/commit$/)
      if (req.method === 'POST' && sessionGitCommitMatch) {
        const body = await bodyJson(req)
        json(res, 200, await runtime.commitSessionGitChanges(decodeURIComponent(sessionGitCommitMatch[1]), body.message))
        return true
      }
      const sessionGitPushMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/git\/push$/)
      if (req.method === 'POST' && sessionGitPushMatch) {
        json(res, 200, await runtime.pushSessionGitChanges(decodeURIComponent(sessionGitPushMatch[1])))
        return true
      }
      const sessionGitRevertMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/git\/revert$/)
      if (req.method === 'POST' && sessionGitRevertMatch) {
        json(res, 200, await runtime.revertSessionGitChanges(decodeURIComponent(sessionGitRevertMatch[1])))
        return true
      }
      const sessionExecutionMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/execution-mode$/)
      if (req.method === 'PUT' && sessionExecutionMatch) {
        const body = await bodyJson(req)
        const updated = await runtime.setSessionExecutionMode(decodeURIComponent(sessionExecutionMatch[1]), body.mode)
        if (!updated) json(res, 404, { error: '会话不存在。' })
        else json(res, 200, updated)
        return true
      }
      const sessionPermissionMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/permission$/)
      if (req.method === 'PUT' && sessionPermissionMatch) {
        const body = await bodyJson(req)
        const updated = await runtime.setSessionPermission(decodeURIComponent(sessionPermissionMatch[1]), body.mode)
        if (!updated) json(res, 404, { error: '会话不存在。' })
        else json(res, 200, updated)
        return true
      }
      const sessionApprovalMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/approvals\/([^/]+)$/)
      if (req.method === 'POST' && sessionApprovalMatch) {
        const body = await bodyJson(req)
        const resolution = runtime.resolveToolApproval(
          decodeURIComponent(sessionApprovalMatch[1]),
          decodeURIComponent(sessionApprovalMatch[2]),
          Boolean(body.approved),
        )
        if (!resolution.found) json(res, 404, { error: '授权请求不存在。' })
        else json(res, 200, resolution)
        return true
      }
      const sessionMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)$/)
      if (req.method === 'PATCH' && sessionMatch) {
        const id = decodeURIComponent(sessionMatch[1])
        const updated = await runtime.renameSession(id, (await bodyJson(req)).name, { manual: true })
        if (!updated) json(res, 404, { error: '会话不存在。' })
        else json(res, 200, updated)
        return true
      }
      if (req.method === 'DELETE' && sessionMatch) {
        const id = decodeURIComponent(sessionMatch[1])
        const deleted = await runtime.deleteSession(id)
        if (!deleted) json(res, 404, { error: '会话不存在。' })
        else json(res, 200, { deleted: true, id })
        return true
      }
      const messagesMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/messages$/)
      if (req.method === 'GET' && messagesMatch) {
        json(res, 200, await runtime.getSessionMessagePage(decodeURIComponent(messagesMatch[1]), {
          before: url.searchParams.get('before'),
          limit: url.searchParams.get('limit'),
        }))
        return true
      }
      const liveSessionMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/live$/)
      if (req.method === 'GET' && liveSessionMatch) {
        json(res, 200, await runtime.getSessionLive(decodeURIComponent(liveSessionMatch[1])))
        return true
      }
      const inputMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/input$/)
      if (req.method === 'POST' && inputMatch) {
        const body = await bodyJson(req)
        json(res, 200, await runtime.queueSessionMessage(decodeURIComponent(inputMatch[1]), {
          message: body.message,
          behavior: body.behavior,
        }))
        return true
      }
      const abortMatch = url.pathname.match(/^\/api\/sessions\/([^/]+)\/abort$/)
      if (req.method === 'POST' && abortMatch) {
        const id = decodeURIComponent(abortMatch[1])
        json(res, 200, { aborted: await runtime.abortSession(id), goal: runtime.getSessionGoal(id) })
        return true
      }
      if (req.method === 'POST' && url.pathname === '/api/chat') {
        const body = await bodyJson(req)
        if (!String(body.message || '').trim()) throw new Error('消息不能为空。')
        res.writeHead(200, {
          'Content-Type': 'text/event-stream; charset=utf-8',
          'Cache-Control': 'no-cache, no-transform',
          Connection: 'keep-alive',
          'X-Accel-Buffering': 'no',
        })
        res.flushHeaders?.()
        try {
          await runtime.streamPrompt({
            sessionId: body.sessionId,
            message: String(body.message).trim(),
            attachments: body.attachments,
            requestedToolNames: body.requestedToolNames,
            goalMode: Boolean(body.goalMode),
            goalTokenBudget: body.goalTokenBudget == null ? null : Number(body.goalTokenBudget),
            send: (event, data) => { if (!res.destroyed && !res.writableEnded) sseSend(res, event, data) },
          })
        } catch (error) {
          // streamPrompt normally emits its own terminal error snapshot and returns;
          // only send a fallback when an unexpected throw escapes.
          if (!res.destroyed && !res.writableEnded) sseSend(res, 'error', { message: publicError(error) })
        }
        if (!res.writableEnded) res.end()
        return true
      }
      json(res, 404, { error: '接口不存在。' })
    } catch (error) {
      json(res, 400, { error: publicError(error) })
    }
    return true
  }
}
