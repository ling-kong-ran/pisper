// 会话文件快照（/file-changes）的读取、批准与撤销：从 SessionFilesPane 抽出，供旧上下文面板与会话页检查器共用。
// 状态归调用它的面板所有：切换会话时整组状态随面板重新挂载；确认框可以跨面板存活，卸载即失效。
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { chatApi, type SessionFileChangesResponse } from './chat-api'
import { invalidateSessionChangeSummary } from './session-change-summary-api'

export type SessionFileAction = { kind: 'approve' | 'revert'; path?: string }

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export function useSessionFileChanges({
  sessionId,
  streaming,
  requestConfirm,
}: {
  sessionId: string
  streaming: boolean
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t } = useI18n()
  const [revision, setRevision] = useState(0)
  const [snapshotRevision, setSnapshotRevision] = useState(0)
  const [changes, setChanges] = useState<SessionFileChangesResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [actionError, setActionError] = useState('')
  const [notice, setNotice] = useState('')
  const [running, setRunning] = useState<SessionFileAction | null>(null)
  const [diff, setDiff] = useState<{ diff: string; truncated: boolean } | null>(null)
  const [diffLoading, setDiffLoading] = useState<string | null>(null)
  const activeRef = useRef(false)
  const requestSequence = useRef(0)
  const diffSequence = useRef(0)
  const loadingRef = useRef(true)
  const runningRef = useRef(false)
  const streamingRef = useRef(streaming)

  useLayoutEffect(() => {
    streamingRef.current = streaming
  }, [streaming])

  // 确认框可以跨面板存活，卸载提交阶段就失效，不能等后续异步清理。
  useLayoutEffect(() => {
    activeRef.current = true
    return () => {
      activeRef.current = false
      requestSequence.current += 1
      diffSequence.current += 1
    }
  }, [])

  useEffect(() => {
    const sequence = ++requestSequence.current
    diffSequence.current += 1
    setDiff(null)
    setDiffLoading(null)
    loadingRef.current = true
    setLoading(true)
    setError('')
    void chatApi
      .getSessionFileChanges(sessionId)
      .then(
        (data) => {
          if (activeRef.current && sequence === requestSequence.current) {
            setChanges(data)
            setSnapshotRevision((value) => value + 1)
          }
        },
        (caught: unknown) => {
          if (activeRef.current && sequence === requestSequence.current)
            setError(errorMessage(caught))
        },
      )
      .finally(() => {
        if (activeRef.current && sequence === requestSequence.current) {
          loadingRef.current = false
          setLoading(false)
        }
      })
    return () => {
      requestSequence.current += 1
    }
  }, [revision, sessionId, streaming])

  const viewDiff = async (path: string) => {
    if (runningRef.current || loadingRef.current) return
    const sequence = ++diffSequence.current
    setDiffLoading(path)
    setActionError('')
    try {
      const result = await chatApi.getSessionFileChangeDiff(sessionId, path)
      if (!activeRef.current || sequence !== diffSequence.current) return
      if (!result.diff?.trim()) {
        setActionError(t('chat:focusSession.fileChangesNoSnapshot'))
        return
      }
      setDiff({ diff: result.diff, truncated: Boolean(result.diffTruncated) })
    } catch (caught) {
      if (activeRef.current && sequence === diffSequence.current)
        setActionError(errorMessage(caught))
    } finally {
      if (activeRef.current && sequence === diffSequence.current) setDiffLoading(null)
    }
  }

  const runAction = async (action: SessionFileAction) => {
    if (
      !activeRef.current ||
      runningRef.current ||
      loadingRef.current ||
      streamingRef.current ||
      !changes ||
      error
    )
      return
    const eligibleFiles = changes.files.filter((file) =>
      action.kind === 'approve'
        ? file.pending && !file.approved && !file.reverted
        : file.canRevert && !file.reverted,
    )
    if (!eligibleFiles.some((file) => action.path === undefined || file.path === action.path))
      return

    // 同步锁同时覆盖确认和写入，防止双击、单文件与批量操作彼此交错。
    runningRef.current = true
    setRunning(action)
    setActionError('')
    setNotice('')
    const sequence = ++requestSequence.current
    diffSequence.current += 1
    setDiff(null)
    setDiffLoading(null)
    let attempted = false
    try {
      if (action.kind === 'revert') {
        const confirmed = await requestConfirm({
          title: action.path
            ? t('chat:focusSession.fileChangesRevertFile')
            : t('chat:focusSession.fileChangesRevertAll'),
          message: action.path
            ? t('chat:focusSession.fileChangesRevertFileDescription', { path: action.path })
            : t('chat:focusSession.fileChangesRevertAllDescription'),
          confirmLabel: t('chat:focusSession.fileChangesConfirmRevert'),
          tone: 'danger',
        })
        if (!confirmed) return
      }
      // 确认期间会话可能开始新一轮运行，不能再提交针对旧快照的撤销。
      if (
        !activeRef.current ||
        streamingRef.current ||
        loadingRef.current ||
        sequence !== requestSequence.current
      )
        return
      attempted = true
      const result =
        action.kind === 'approve'
          ? await chatApi.approveSessionFileChanges(sessionId, action.path)
          : await chatApi.revertSessionFileChanges(sessionId, action.path)
      // 写入属于会话本身；即使面板已卸载，也要让其他摘要观察者重新读取。
      void invalidateSessionChangeSummary(sessionId)
      if (!activeRef.current || sequence !== requestSequence.current) return
      setChanges(result)
      if (action.kind === 'approve') {
        setNotice(
          action.path
            ? t('chat:focusSession.fileChangesApproved')
            : t('chat:focusSession.fileChangesApproveDone'),
        )
      } else if (!result.reverted) {
        setNotice(t('chat:focusSession.fileChangesRevertUnavailable'))
      } else if (action.path === undefined && result.files.some((file) => file.pending)) {
        setNotice(t('chat:focusSession.fileChangesRevertPartial', { count: result.reverted }))
      } else {
        setNotice(t('chat:focusSession.fileChangesRevertDone'))
      }
    } catch (caught) {
      if (activeRef.current && sequence === requestSequence.current)
        setActionError(errorMessage(caught))
    } finally {
      runningRef.current = false
      if (activeRef.current) {
        setRunning(null)
        // 运行状态变化或网络错误时都重新取快照，旧读取不能覆盖写入结果。
        if (attempted) {
          loadingRef.current = true
          setLoading(true)
          setRevision((current) => current + 1)
        }
      }
    }
  }

  const writesDisabled = loading || streaming || Boolean(running) || Boolean(error) || !changes
  const pendingCount = changes?.summary.pending || 0
  const reversibleCount =
    changes?.files.filter((file) => file.canRevert && !file.reverted).length || 0
  const displayedError = actionError || error

  const refresh = () => {
    loadingRef.current = true
    setLoading(true)
    setActionError('')
    setRevision((current) => current + 1)
  }

  return {
    changes,
    loading,
    error,
    actionError,
    notice,
    running,
    diff,
    setDiff,
    diffLoading,
    snapshotRevision,
    writesDisabled,
    pendingCount,
    reversibleCount,
    displayedError,
    viewDiff,
    runAction,
    refresh,
  }
}
