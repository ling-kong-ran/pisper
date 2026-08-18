// 会话级命令：重命名/删除/清空等操作的确认与执行。
import { useCallback, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { workspaceName } from '@/lib/format'
import { ApiError } from '@/lib/http'
import { hasSystemDirectoryPicker, pickSystemDirectory } from '@/lib/pick-system-directory'
import type { SessionStateUpdate } from '@/lib/session-state'
import type { ModelOption, SessionState, SessionSummary } from '@/types/chat'
import { chatApi } from './chat-api'
import { chatErrorMessage } from './chat-errors'
import { announceSessionsUpdated } from './events'

type SessionCommandOptions = {
  notify: Notify
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  availableModels: ModelOption[]
  sessionStatesRef: React.MutableRefObject<Record<string, SessionState>>
  updateSessionState: (id: string, update: SessionStateUpdate) => void
  updateSessions: (
    update: SessionSummary[] | ((current: SessionSummary[]) => SessionSummary[]),
  ) => SessionSummary[]
  replaceSessionStates: (states: Record<string, SessionState>) => void
  setGlobalError: (error: string) => void
  syncLiveSession: (id: string) => Promise<void>
}

export function useSessionCommands({
  notify,
  requestText,
  requestConfirm,
  availableModels,
  sessionStatesRef,
  updateSessionState,
  updateSessions,
  replaceSessionStates,
  setGlobalError,
  syncLiveSession,
}: SessionCommandOptions) {
  const { t, language } = useI18n()
  const [workspaceSession, setWorkspaceSession] = useState<SessionSummary | null>(null)

  // 更新会话摘要项（函数式），供会话命令同步列表字段。
  const updateSessionSummary = useCallback(
    (sessionId: string, update: (session: SessionSummary) => SessionSummary) => {
      updateSessions((current) =>
        current.map((session) => (session.id === sessionId ? update(session) : session)),
      )
    },
    [updateSessions],
  )

  // 暂停目标模式（goal）：调运行时并同步状态与摘要。
  const pauseGoal = useCallback(
    async (sessionId: string) => {
      if (!sessionId) return
      try {
        const result = await chatApi.pauseGoal(sessionId)
        updateSessionState(sessionId, { goal: result.goal || null })
        updateSessionSummary(sessionId, (session) => ({
          ...session,
          goal: result.goal || null,
        }))
        notify(t('chat:chatPage.goalPaused'), 'info')
      } catch (error) {
        updateSessionState(sessionId, { error: chatErrorMessage(error) })
      }
    },
    [notify, t, updateSessionState, updateSessionSummary],
  )

  // 设置目标模式的 token 预算。
  const setGoalBudget = useCallback(
    async (sessionId: string, tokenBudget: number) => {
      if (!sessionId) return
      try {
        const result = await chatApi.setGoalBudget(sessionId, tokenBudget)
        updateSessionState(sessionId, { goal: result.goal || null })
        updateSessionSummary(sessionId, (session) => ({
          ...session,
          goal: result.goal || null,
        }))
        notify(t('chat:chatPage.goalTokenBudgetUpdated'), 'info')
      } catch (error) {
        updateSessionState(sessionId, { error: chatErrorMessage(error) })
      }
    },
    [notify, t, updateSessionState, updateSessionSummary],
  )

  // 手动压缩上下文：流式或已在压缩时忽略；先本地置“运行中”，
  // 成功后回写压缩结果与用量，失败时同步实时状态恢复。
  const compactSession = useCallback(
    async (sessionId: string) => {
      const current = sessionStatesRef.current[sessionId]
      if (!sessionId || current?.streaming || current?.compaction?.active) return
      updateSessionState(sessionId, {
        compaction: {
          ...(current?.compaction || {}),
          active: true,
          status: 'running',
          reason: 'manual',
        },
        error: '',
      })
      try {
        const result = await chatApi.compactSession(sessionId)
        updateSessionState(sessionId, {
          compaction: result.compaction || null,
          contextUsage: result.contextUsage || null,
        })
        notify(t('chat:chatPage.contextCompacted'))
      } catch (error) {
        await syncLiveSession(sessionId).catch(() => {})
        updateSessionState(sessionId, { error: chatErrorMessage(error) })
      }
    },
    [notify, sessionStatesRef, syncLiveSession, t, updateSessionState],
  )

  // 更新全局压缩阈值：调运行时偏好并批量回写所有会话的压缩预算。
  const setCompactionThreshold = useCallback(
    async (thresholdPercent: number) => {
      const preference = await chatApi.updateCompactionPreference(thresholdPercent)
      const states = Object.fromEntries(
        Object.entries(sessionStatesRef.current).map(([sessionId, state]) => {
          const contextWindow = Number(state.contextUsage?.contextWindow) || 0
          if (!contextWindow) return [sessionId, state]
          return [
            sessionId,
            {
              ...state,
              contextUsage: {
                ...state.contextUsage,
                compactAtPercent: preference.thresholdPercent,
                compactAtTokens: Math.floor((contextWindow * preference.thresholdPercent) / 100),
              },
            },
          ]
        }),
      )
      replaceSessionStates(states)
    },
    [replaceSessionStates, sessionStatesRef],
  )

  // 应用思考强度状态：模型不匹配时忽略（响应来自已切换的模型），
  // 兼容新旧字段名并同步摘要。
  const applyThinkingState = useCallback(
    (sessionId: string, payload: Record<string, unknown> = {}) => {
      const responseModel = String(payload.model || '')
      const currentModel = String(sessionStatesRef.current[sessionId]?.model || '')
      if (responseModel && currentModel && responseModel !== currentModel) return

      const availableThinkingLevels = Array.isArray(payload.availableThinkingLevels)
        ? payload.availableThinkingLevels.map((level) => String(level))
        : Array.isArray(payload.availableLevels)
          ? payload.availableLevels.map((level) => String(level))
          : []
      const thinkingLevel = String(payload.thinkingLevel || '')
      updateSessionState(sessionId, {
        thinkingLevel: thinkingLevel || undefined,
        availableThinkingLevels,
        thinkingStatus: String(payload.thinkingStatus || payload.status || ''),
        thinkingMessage: String(payload.thinkingMessage || payload.message || ''),
        switchingThinking: false,
      })
      if (thinkingLevel) {
        updateSessionSummary(sessionId, (session) => ({ ...session, thinkingLevel }))
      }
    },
    [sessionStatesRef, updateSessionState, updateSessionSummary],
  )

  // 加载会话思考强度（流式中跳过）。
  const loadSessionThinkingLevel = useCallback(
    async (sessionId: string) => {
      if (!sessionId || sessionStatesRef.current[sessionId]?.streaming) return
      try {
        const state = await chatApi.getThinkingLevel(sessionId)
        applyThinkingState(sessionId, state)
      } catch {
        updateSessionState(sessionId, {
          thinkingStatus: 'error',
          thinkingMessage: t('chat:focusSession.thinkingLevelsLoadFailed'),
        })
      }
    },
    [applyThinkingState, sessionStatesRef, t, updateSessionState],
  )

  // 切换会话模型：流式中忽略；先乐观更新（防止受控下拉回弹），
  // 成功后回写并提示是否压缩上下文，失败时回滚模型。
  const switchSessionModel = useCallback(
    async (sessionId: string, nextModel: string) => {
      const selected = availableModels.find((item) => item.key === nextModel)
      if (!sessionId || !selected || sessionStatesRef.current[sessionId]?.streaming) return
      const current = sessionStatesRef.current[sessionId]
      const previousModel = current?.model || ''
      const shouldOfferCompaction =
        Boolean(current?.messages?.length) && !current?.compaction?.active
      // Optimistic update so the controlled select does not snap back while the request is in flight.
      updateSessionState(sessionId, {
        switchingModel: true,
        model: selected.key,
        error: '',
      })
      updateSessionSummary(sessionId, (session) => ({ ...session, model: selected.key }))
      try {
        const updated = await chatApi.updateModel(sessionId, selected.provider, selected.modelId)
        updateSessionState(sessionId, {
          model: updated.model || selected.key,
          contextUsage: updated.contextUsage ?? null,
          switchingModel: false,
        })
        updateSessionSummary(sessionId, (session) => ({
          ...session,
          model: updated.model || selected.key,
        }))
        applyThinkingState(sessionId, updated)
        notify(t('chat:chatPage.switchedToModel', { model: selected.label }))
        if (shouldOfferCompaction) {
          const confirmed = await requestConfirm({
            title: t('chat:chatPage.compactAfterModelSwitch'),
            message: t('chat:chatPage.compactAfterModelSwitchDescription'),
            confirmLabel: t('chat:chatPage.compactNow'),
          })
          if (confirmed) await compactSession(sessionId)
        }
      } catch (error) {
        updateSessionState(sessionId, {
          switchingModel: false,
          model: previousModel || undefined,
          error: chatErrorMessage(error),
        })
        if (previousModel) {
          updateSessionSummary(sessionId, (session) => ({ ...session, model: previousModel }))
        }
      }
    },
    [
      applyThinkingState,
      availableModels,
      compactSession,
      notify,
      requestConfirm,
      sessionStatesRef,
      t,
      updateSessionState,
      updateSessionSummary,
    ],
  )

  // 切换思考强度：乐观更新 → PUT → 应用响应状态，失败回滚。
  const switchSessionThinkingLevel = useCallback(
    async (sessionId: string, nextLevel: string) => {
      const level = String(nextLevel || '').trim()
      if (!sessionId || !level || sessionStatesRef.current[sessionId]?.streaming) return
      const previous = sessionStatesRef.current[sessionId] || {}
      const previousLevel = String(previous.thinkingLevel || '')
      updateSessionState(sessionId, {
        switchingThinking: true,
        thinkingLevel: level,
        error: '',
      })
      updateSessionSummary(sessionId, (session) => ({ ...session, thinkingLevel: level }))
      try {
        const updated = await chatApi.setThinkingLevel(sessionId, level)
        applyThinkingState(sessionId, updated)
        notify(
          t('chat:chatPage.switchedToThinkingLevel', {
            level: String(updated.thinkingLevel || level),
          }),
        )
      } catch (error) {
        updateSessionState(sessionId, {
          switchingThinking: false,
          thinkingLevel: previousLevel || undefined,
          error: chatErrorMessage(error),
        })
        if (previousLevel) {
          updateSessionSummary(sessionId, (session) => ({
            ...session,
            thinkingLevel: previousLevel,
          }))
        }
      }
    },
    [applyThinkingState, notify, sessionStatesRef, t, updateSessionState, updateSessionSummary],
  )

  // 切换执行模式：切到 full-access 前强制二次确认；
  // 成功后同步状态与摘要并返回是否生效。
  const switchSessionExecutionMode = useCallback(
    async (sessionId: string, executionMode: string) => {
      if (!sessionId) return false
      updateSessionState(sessionId, { switchingPermission: true, error: '' })
      try {
        if (executionMode === 'full-access') {
          const confirmed = await requestConfirm({
            title: t('chat:chatPage.enableFullAccess'),
            message: t(
              'chat:chatPage.fullAccessAllowsTheAgentToUseFilesAndNetworkServicesOutsideTheWorkspaceAndShellCommandsWillNoLon',
            ),
            confirmLabel: t('chat:chatPage.enableFullAccess'),
          })
          if (!confirmed) {
            updateSessionState(sessionId, { switchingPermission: false })
            return false
          }
        }
        const updated = await chatApi.updateExecutionMode(sessionId, executionMode)
        updateSessionState(sessionId, {
          executionMode: updated.executionMode,
          permissionMode: updated.permissionMode,
          switchingPermission: false,
        })
        updateSessionSummary(sessionId, (session) => ({
          ...session,
          executionMode: updated.executionMode,
          permissionMode: updated.permissionMode,
        }))
        notify(
          t('chat:chatPage.executionModeChangedToMode', {
            mode:
              updated.executionMode === 'read-only'
                ? t('chat:chatPage.readOnly')
                : updated.executionMode === 'approval-required'
                  ? t('chat:chatPage.approvalRequired')
                  : updated.executionMode === 'workspace-write'
                    ? t('chat:focusSession.workspaceWrite')
                    : t('chat:chatPage.fullAccess'),
          }),
        )
        return true
      } catch (error) {
        updateSessionState(sessionId, {
          switchingPermission: false,
          error: chatErrorMessage(error),
        })
        return false
      }
    },
    [notify, requestConfirm, t, updateSessionState, updateSessionSummary],
  )

  // 处理工具审批：先本地移除待审批项（快速反馈），再 POST 结果；
  // 已在别处处理（404）时同步实时状态并提示。
  const resolveToolApproval = useCallback(
    async (sessionId: string, approvalId: string, approved: boolean) => {
      updateSessionState(sessionId, (current) => ({
        ...current,
        approvals: (current.approvals || []).filter((item) => item.id !== approvalId),
        error: '',
      }))
      try {
        const resolution = await chatApi.resolveApproval(sessionId, approvalId, approved)
        if (resolution.alreadyResolved) void syncLiveSession(sessionId)
      } catch (error) {
        await syncLiveSession(sessionId)
        if (error instanceof ApiError && error.status === 404) {
          notify(t('chat:chatPage.approvalStatusUpdated'), 'info')
          return
        }
        updateSessionState(sessionId, { error: chatErrorMessage(error) })
        throw error
      }
    },
    [notify, syncLiveSession, t, updateSessionState],
  )

  // 切换会话工作目录（流式中忽略），成功后同步摘要。
  const switchSessionCwd = useCallback(
    async (session: SessionSummary, cwd: string) => {
      if (!session?.id || sessionStatesRef.current[session.id]?.streaming) return
      updateSessionState(session.id, { switchingCwd: true, error: '' })
      try {
        const updated = await chatApi.updateCwd(session.id, cwd)
        updateSessionState(session.id, { cwd: updated.cwd, switchingCwd: false })
        updateSessionSummary(session.id, (current) => ({ ...current, cwd: updated.cwd }))
        setWorkspaceSession(null)
        notify(
          t('chat:chatPage.workingDirectoryChangedToWorkspace', {
            workspace: workspaceName(updated.cwd, language),
          }),
        )
      } catch (error) {
        updateSessionState(session.id, {
          switchingCwd: false,
          error: chatErrorMessage(error),
        })
        throw error
      }
    },
    [language, notify, sessionStatesRef, t, updateSessionState, updateSessionSummary],
  )

  const selectSessionWorkspace = useCallback(
    async (session: SessionSummary) => {
      if (!session?.id) return
      if (sessionStatesRef.current[session.id]?.streaming) {
        notify(t('chat:chatPage.stopTheActiveRunBeforeChangingTheWorkspace'), 'info')
        return
      }
      if (!hasSystemDirectoryPicker()) {
        setWorkspaceSession(session)
        return
      }
      try {
        const cwd = await pickSystemDirectory(session.cwd)
        if (cwd) await switchSessionCwd(session, cwd)
      } catch (error) {
        updateSessionState(session.id, {
          switchingCwd: false,
          error: chatErrorMessage(error),
        })
      }
    },
    [notify, sessionStatesRef, switchSessionCwd, t, updateSessionState],
  )

  const renameSession = useCallback(
    async (session: SessionSummary) => {
      if (!session?.id) return
      if (sessionStatesRef.current[session.id]?.streaming) {
        notify(t('chat:chatPage.stopTheActiveRunBeforeRenamingTheChat'), 'info')
        return
      }
      const name = await requestText({
        title: t('chat:chatPage.renameChat'),
        inputLabel: t('chat:chatPage.chatTitle'),
        value: session.name,
        confirmLabel: t('chat:chatPage.save'),
      })
      if (name === null || name === session.name) return
      try {
        const updated = await chatApi.renameSession(session.id, name)
        updateSessionSummary(session.id, (current) => ({ ...current, name: updated.name }))
        announceSessionsUpdated()
        notify(t('chat:chatPage.chatTitleUpdated'))
      } catch (error) {
        setGlobalError(chatErrorMessage(error))
      }
    },
    [notify, requestText, sessionStatesRef, setGlobalError, t, updateSessionSummary],
  )

  return {
    workspaceSession,
    setWorkspaceSession,
    switchSessionCwd,
    selectSessionWorkspace,
    renameSession,
    pauseGoal,
    setGoalBudget,
    compactSession,
    setCompactionThreshold,
    loadSessionThinkingLevel,
    switchSessionModel,
    switchSessionThinkingLevel,
    switchSessionExecutionMode,
    resolveToolApproval,
  }
}
