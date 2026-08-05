import { useCallback, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { workspaceName } from '@/lib/format'
import { ApiError } from '@/lib/http'
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

  const updateSessionSummary = useCallback(
    (sessionId: string, update: (session: SessionSummary) => SessionSummary) => {
      updateSessions((current) =>
        current.map((session) => (session.id === sessionId ? update(session) : session)),
      )
    },
    [updateSessions],
  )

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

  const switchSessionModel = useCallback(
    async (sessionId: string, nextModel: string) => {
      const selected = availableModels.find((item) => item.key === nextModel)
      if (!sessionId || !selected || sessionStatesRef.current[sessionId]?.streaming) return
      updateSessionState(sessionId, { switchingModel: true, error: '' })
      try {
        const updated = await chatApi.updateModel(sessionId, selected.provider, selected.modelId)
        updateSessionState(sessionId, {
          model: updated.model,
          contextUsage: updated.contextUsage ?? null,
          switchingModel: false,
        })
        updateSessionSummary(sessionId, (session) => ({ ...session, model: updated.model }))
        notify(t('chat:chatPage.switchedToModel', { model: selected.label }))
      } catch (error) {
        updateSessionState(sessionId, {
          switchingModel: false,
          error: chatErrorMessage(error),
        })
      }
    },
    [availableModels, notify, sessionStatesRef, t, updateSessionState, updateSessionSummary],
  )

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

  const renameSession = useCallback(
    async (session: SessionSummary) => {
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
    [notify, requestText, setGlobalError, t, updateSessionSummary],
  )

  return {
    workspaceSession,
    setWorkspaceSession,
    switchSessionCwd,
    renameSession,
    pauseGoal,
    setGoalBudget,
    compactSession,
    setCompactionThreshold,
    switchSessionModel,
    switchSessionExecutionMode,
    resolveToolApproval,
  }
}
