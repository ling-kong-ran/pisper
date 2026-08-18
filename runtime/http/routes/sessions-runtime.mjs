// 会话运行时路由：健康检查、诊断、用量、会话 CRUD、会话运行（SSE 流式）、
// 消息/历史/树/标签、权限审批、模型/工作目录切换等核心 API。
export const sessionRuntimeRoutes = [
  {
    method: 'GET',
    path: '/api/health',
    handler({ services, json }) {
      json(200, {
        ok: true,
        engine: '@earendil-works/pi-coding-agent',
        version: services.engineVersion,
      })
    },
  },
  {
    method: 'GET',
    path: '/api/runtime/diagnostics',
    handler({ runtime, json }) {
      json(200, runtime.getRuntimeDiagnostics())
    },
  },
  {
    method: 'GET',
    path: '/api/usage/today',
    async handler({ runtime, json }) {
      json(200, await runtime.getTodayUsage())
    },
  },
  {
    method: 'GET',
    path: '/api/sessions',
    async handler({ runtime, json }) {
      json(200, { sessions: await runtime.listSessions() })
    },
  },
  {
    method: 'GET',
    path: '/api/session-labels',
    async handler({ runtime, url, json }) {
      json(200, {
        labels: await runtime.searchSessionTreeLabels(url.searchParams.get('query'), {
          limit: url.searchParams.get('limit'),
        }),
      })
    },
  },
  {
    method: 'POST',
    path: '/api/sessions',
    async handler({ runtime, body, json }) {
      const input = await body()
      json(201, await runtime.createSession(input.name, input.cwd))
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/derive',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      json(
        201,
        await runtime.deriveSession(
          params.sessionId,
          String(input.boundaryEntryId || ''),
          String(input.name || ''),
        ),
      )
    },
  },
  {
    method: 'GET',
    path: '/api/sessions/:sessionId/tree',
    async handler({ runtime, params, json }) {
      json(200, await runtime.getSessionTree(params.sessionId))
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/tree/navigate',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      if (typeof input.targetEntryId !== 'string' || !input.targetEntryId.trim()) {
        throw new Error('targetEntryId 不能为空。')
      }
      if (input.summarize != null && typeof input.summarize !== 'boolean') {
        throw new Error('summarize 必须是布尔值。')
      }
      if (input.includeTree != null && typeof input.includeTree !== 'boolean') {
        throw new Error('includeTree 必须是布尔值。')
      }
      json(
        200,
        await runtime.navigateSessionTree(params.sessionId, input.targetEntryId, {
          summarize: Boolean(input.summarize),
          ...(input.includeTree === false ? { includeTree: false } : {}),
        }),
      )
    },
  },
  {
    method: 'PUT',
    path: '/api/sessions/:sessionId/tree/labels/:entryId',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      if (typeof input.label !== 'string') throw new Error('label 必须是字符串。')
      json(200, await runtime.setSessionTreeLabel(params.sessionId, params.entryId, input.label))
    },
  },
  {
    method: 'GET',
    path: '/api/sessions/:sessionId/workspace-trust',
    async handler({ runtime, params, json }) {
      json(200, await runtime.getWorkspaceTrust(params.sessionId))
    },
  },
  {
    method: 'GET',
    path: '/api/sessions/:sessionId/commands',
    async handler({ runtime, params, json }) {
      json(200, await runtime.getSessionCommands(params.sessionId))
    },
  },
  {
    method: 'PUT',
    path: '/api/sessions/:sessionId/workspace-trust',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      if (typeof input.trusted !== 'boolean') throw new Error('trusted 必须是布尔值。')
      json(200, await runtime.setWorkspaceTrust(params.sessionId, input.trusted))
    },
  },
  {
    method: 'PUT',
    path: '/api/sessions/:sessionId/model',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      json(
        200,
        await runtime.setSessionModel(
          params.sessionId,
          String(input.provider || ''),
          String(input.model || ''),
        ),
      )
    },
  },
  {
    method: 'GET',
    path: '/api/sessions/:sessionId/thinking-level',
    async handler({ runtime, params, json }) {
      json(200, await runtime.getSessionThinkingState(params.sessionId))
    },
  },
  {
    method: 'PUT',
    path: '/api/sessions/:sessionId/thinking-level',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      json(200, await runtime.setSessionThinkingLevel(params.sessionId, String(input.level || '')))
    },
  },
  {
    method: 'PUT',
    path: '/api/sessions/:sessionId/cwd',
    async handler({ runtime, params, body, json }) {
      const updated = await runtime.setSessionCwd(params.sessionId, (await body()).cwd)
      if (!updated) json(404, { error: '会话不存在。' })
      else json(200, updated)
    },
  },
  {
    method: 'GET',
    path: '/api/sessions/:sessionId/goal',
    handler({ runtime, params, json }) {
      const goal = runtime.getSessionGoal(params.sessionId)
      if (!goal) json(404, { error: '当前会话没有 Goal。' })
      else json(200, { goal })
    },
  },
  {
    method: 'PATCH',
    path: '/api/sessions/:sessionId/goal',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      if (input.action === 'pause') {
        const goal = await runtime.pauseSessionGoal(params.sessionId)
        if (!goal) json(404, { error: '当前会话没有进行中的 Goal。' })
        else json(200, { goal })
      } else if (input.action === 'set-budget') {
        const goal = await runtime.setSessionGoalBudget(params.sessionId, input.tokenBudget)
        json(200, { goal })
      } else {
        throw new Error('Goal 操作无效。')
      }
    },
  },
  {
    method: 'GET',
    path: '/api/sessions/:sessionId/git/changes',
    async handler({ runtime, params, json }) {
      json(200, await runtime.getSessionGitChanges(params.sessionId))
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/git/commit',
    async handler({ runtime, params, body, json }) {
      json(200, await runtime.commitSessionGitChanges(params.sessionId, (await body()).message))
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/git/push',
    async handler({ runtime, params, json }) {
      json(200, await runtime.pushSessionGitChanges(params.sessionId))
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/git/revert',
    async handler({ runtime, params, json }) {
      json(200, await runtime.revertSessionGitChanges(params.sessionId))
    },
  },
  {
    method: 'GET',
    path: '/api/sessions/:sessionId/vcs/changes',
    async handler({ runtime, params, json }) {
      json(200, await runtime.getSessionVcsChanges(params.sessionId))
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/vcs/commit',
    async handler({ runtime, params, body, json }) {
      json(200, await runtime.commitSessionVcsChanges(params.sessionId, (await body()).message))
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/vcs/push',
    async handler({ runtime, params, json }) {
      json(200, await runtime.pushSessionVcsChanges(params.sessionId))
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/vcs/revert',
    async handler({ runtime, params, json }) {
      json(200, await runtime.revertSessionVcsChanges(params.sessionId))
    },
  },
  {
    method: 'PUT',
    path: '/api/sessions/:sessionId/execution-mode',
    async handler({ runtime, params, body, json }) {
      const updated = await runtime.setSessionExecutionMode(params.sessionId, (await body()).mode)
      if (!updated) json(404, { error: '会话不存在。' })
      else json(200, updated)
    },
  },
  {
    method: 'PUT',
    path: '/api/sessions/:sessionId/permission',
    async handler({ runtime, params, body, json }) {
      const updated = await runtime.setSessionPermission(params.sessionId, (await body()).mode)
      if (!updated) json(404, { error: '会话不存在。' })
      else json(200, updated)
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/approvals/:approvalId',
    async handler({ runtime, params, body, json }) {
      const resolution = runtime.resolveToolApproval(
        params.sessionId,
        params.approvalId,
        Boolean((await body()).approved),
      )
      if (!resolution.found) json(404, { error: '授权请求不存在。' })
      else json(200, resolution)
    },
  },
  {
    method: 'PATCH',
    path: '/api/sessions/:sessionId',
    async handler({ runtime, params, body, json }) {
      const updated = await runtime.renameSession(params.sessionId, (await body()).name, {
        manual: true,
      })
      if (!updated) json(404, { error: '会话不存在。' })
      else json(200, updated)
    },
  },
  {
    method: 'DELETE',
    path: '/api/sessions/:sessionId',
    async handler({ runtime, params, json }) {
      const deleted = await runtime.deleteSession(params.sessionId)
      if (!deleted) json(404, { error: '会话不存在。' })
      else json(200, { deleted: true, id: params.sessionId })
    },
  },
  {
    method: 'GET',
    path: '/api/sessions/:sessionId/messages',
    async handler({ runtime, params, url, json }) {
      json(
        200,
        await runtime.getSessionMessagePage(params.sessionId, {
          before: url.searchParams.get('before'),
          limit: url.searchParams.get('limit'),
        }),
      )
    },
  },
  {
    method: 'GET',
    path: '/api/sessions/:sessionId/live',
    async handler({ runtime, params, json }) {
      json(200, await runtime.getSessionLive(params.sessionId))
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/compact',
    async handler({ runtime, params, json }) {
      json(200, await runtime.compactSession(params.sessionId))
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/input',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      json(
        200,
        await runtime.queueSessionMessage(params.sessionId, {
          message: input.message,
          attachments: input.attachments,
          behavior: input.behavior,
        }),
      )
    },
  },
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/abort',
    async handler({ runtime, params, json }) {
      json(200, {
        aborted: await runtime.abortSession(params.sessionId),
        goal: runtime.getSessionGoal(params.sessionId),
      })
    },
  },
  {
    method: 'GET',
    path: '/api/sessions/:sessionId/workflow-runs',
    handler({ runtime, params, json }) {
      json(200, runtime.getSessionWorkflowRuns(params.sessionId))
    },
  },
  {
    method: 'POST',
    path: '/api/chat',
    async handler({ runtime, body, startSse, sendSse }) {
      const input = await body()
      const message = String(input.message || '').trim()
      const invocation =
        input.invocation && typeof input.invocation === 'object' ? input.invocation : null
      if (!message && !invocation) throw new Error('消息或资源调用不能为空。')
      startSse()
      if (invocation?.kind === 'workflow') {
        const result = await runtime.runWorkflow(String(invocation.resourceId || ''), {
          trigger: 'chat',
          inputs: invocation.arguments,
          sourceSessionId: String(input.sessionId || ''),
          sourceMessage: message,
        })
        if (!result) throw new Error('工作流不存在。')
        sendSse('invocation_started', { invocation, run: result.run })
        sendSse('done', {
          sessionId: String(input.sessionId || ''),
          text: '',
          invocation: { ...invocation, runId: result.run.id },
        })
        return
      }
      const skillName = invocation?.kind === 'skill' ? String(invocation.resourceName || '') : ''
      const toolName = invocation?.kind === 'tool' ? String(invocation.resourceId || '') : ''
      const prompt = skillName ? `/skill:${skillName}${message ? `\n${message}` : ''}` : message
      await runtime.streamPrompt({
        sessionId: input.sessionId,
        message: prompt,
        attachments: input.attachments,
        requestedToolNames: toolName ? [toolName] : input.requestedToolNames,
        goalMode: Boolean(input.goalMode),
        goalTokenBudget: input.goalTokenBudget == null ? null : Number(input.goalTokenBudget),
        send: sendSse,
      })
    },
  },
]
