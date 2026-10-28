// 聚焦会话输入区状态与提交逻辑：执行模式/预算状态、资源调用选择、
// 排队与手动压缩守卫，以及「发送/排队」提交流程。
// 接收确认与会话身份共同约束草稿清理，拒绝时保留原输入。
import { useEffect, useRef, useState, type FormEvent, type RefObject } from 'react'
import type { ChatAttachment, EntityRecord, ResourceInvocation } from '@/types/chat'
import type { ComposerSubmission } from './composer-drafts'
import type { ComposerExecutionMode } from './GoalModeControl'

type ComposerSelection = {
  attachments: ChatAttachment[]
  preparingAttachments?: { id: string; name: string }[]
}

const COMPOSER_RUN_MODES: ComposerExecutionMode[] = ['plan', 'goal', 'team']

// 会话级持久化的运行模式快照可能缺失或非法，统一归一到 plan 默认值。
function normalizeComposerRunMode(value: string | null | undefined): ComposerExecutionMode {
  return COMPOSER_RUN_MODES.includes(value as ComposerExecutionMode)
    ? (value as ComposerExecutionMode)
    : 'plan'
}

export function useFocusComposer({
  sessionId,
  runMode,
  goalsAvailable,
  teamAvailable,
  workflowsAvailable,
  goal,
  streaming,
  compaction,
  value,
  selection,
  clearDraft,
  captureSubmission,
  promptRef,
  onCompact,
  onGoalPause,
  onSend,
  onQueue,
  onAbort,
  setToolsOpen,
  requestTranscriptBottom,
}: {
  sessionId: string
  runMode?: string | null
  goalsAvailable: boolean
  teamAvailable: boolean
  workflowsAvailable: boolean
  goal?: EntityRecord | null
  streaming?: boolean
  compaction?: EntityRecord | null
  value: string
  selection: ComposerSelection
  clearDraft: () => void
  captureSubmission?: () => ComposerSubmission
  promptRef: RefObject<HTMLTextAreaElement | null>
  onCompact?: () => Promise<void> | void
  onGoalPause?: () => Promise<void> | void
  onSend: (
    value: string,
    attachments: ChatAttachment[],
    goalMode: boolean,
    teamMode: boolean,
    goalTokenBudget: number | null,
    invocation?: ResourceInvocation | null,
    onAccepted?: () => void,
  ) => Promise<boolean | void> | boolean | void
  onQueue?: (
    value: string,
    attachments: ChatAttachment[],
    behavior: string,
    onAccepted?: () => void,
  ) => Promise<boolean> | boolean
  onAbort?: () => Promise<void> | void
  setToolsOpen: (open: boolean) => void
  requestTranscriptBottom: () => void
}) {
  // 运行模式是会话级持久化偏好：初始化与切换会话时都从快照恢复，而不是固定回 plan。
  const [composerExecutionMode, setComposerExecutionMode] = useState<ComposerExecutionMode>(() =>
    normalizeComposerRunMode(runMode),
  )
  const runModeRef = useRef(runMode)
  runModeRef.current = runMode
  const [goalTokenBudget, setGoalTokenBudget] = useState<number | null>(null)
  const [teamTokenBudget, setTeamTokenBudget] = useState<number | null>(null)
  const goalPausePromiseRef = useRef<Promise<void> | null>(null)
  const [queueing, setQueueing] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [stopping, setStopping] = useState(false)
  const [submitError, setSubmitError] = useState('')
  const pendingSubmissionRef = useRef<object | null>(null)
  const activeSessionRef = useRef(sessionId)
  activeSessionRef.current = sessionId
  const [compactingManually, setCompactingManually] = useState(false)
  const [invocation, setInvocation] = useState<ResourceInvocation | null>(null)
  const invocationRef = useRef(invocation)
  invocationRef.current = invocation

  useEffect(() => {
    setComposerExecutionMode(normalizeComposerRunMode(runModeRef.current))
    goalPausePromiseRef.current = null
    setGoalTokenBudget(null)
    setTeamTokenBudget(null)
    pendingSubmissionRef.current = null
    setSubmitting(false)
    setStopping(false)
    setSubmitError('')
    setQueueing(false)
    setCompactingManually(false)
    setInvocation(null)
  }, [sessionId])
  // 会话快照异步到达后同步持久化的运行模式（刷新页面首次渲染时可能尚未加载）。
  useEffect(() => {
    setComposerExecutionMode(normalizeComposerRunMode(runMode))
  }, [sessionId, runMode])
  useEffect(() => {
    if (!goalsAvailable) setComposerExecutionMode('plan')
    if (!teamAvailable && composerExecutionMode === 'team') setComposerExecutionMode('goal')
    if (!workflowsAvailable && invocation?.kind === 'workflow') setInvocation(null)
  }, [composerExecutionMode, goalsAvailable, invocation?.kind, teamAvailable, workflowsAvailable])
  useEffect(() => {
    if (goal?.status === 'active')
      setComposerExecutionMode(goal.mode === 'team' && teamAvailable ? 'team' : 'goal')
  }, [goal?.id, goal?.mode, goal?.status, teamAvailable])
  useEffect(() => {
    if (!goal?.id) {
      setGoalTokenBudget(null)
      return
    }
    const rawBudget = goal.mode === 'team' ? goal.teamTokenBudget : goal.tokenBudget
    const savedBudget = rawBudget == null ? null : Number(rawBudget)
    const nextBudget =
      savedBudget !== null && Number.isFinite(savedBudget) && savedBudget > 0 ? savedBudget : null
    if (goal.mode === 'team') setTeamTokenBudget(nextBudget)
    else setGoalTokenBudget(nextBudget)
  }, [goal?.id, goal?.mode, goal?.teamTokenBudget, goal?.tokenBudget])

  const requestGoalPause = () => {
    if (goalPausePromiseRef.current) return goalPausePromiseRef.current
    const pending = Promise.resolve().then(() => onGoalPause?.())
    goalPausePromiseRef.current = pending
    void pending.catch(() => {})
    return pending
  }
  const compactContext = async () => {
    if (!onCompact || streaming || compactingManually || compaction?.active) return
    setCompactingManually(true)
    try {
      await onCompact()
    } finally {
      setCompactingManually(false)
    }
  }
  const stop = async () => {
    if (stopping || !onAbort) return
    const originSession = sessionId
    setStopping(true)
    try {
      await onAbort()
    } catch (error) {
      if (activeSessionRef.current === originSession)
        setSubmitError(error instanceof Error ? error.message : String(error))
    } finally {
      if (activeSessionRef.current === originSession) setStopping(false)
    }
  }

  // 草稿在服务端确认接收时才清理；请求拒绝前保持完整文本与附件，避免恢复覆盖新输入。
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (pendingSubmissionRef.current || stopping || selection.preparingAttachments?.length) return
    if (!value.trim() && !selection.attachments.length && !invocation) return
    if (streaming && ((!value.trim() && !selection.attachments.length) || !onQueue)) return
    const identity = {}
    const originSession = sessionId
    pendingSubmissionRef.current = identity
    const ownsRequest = () =>
      activeSessionRef.current === originSession && pendingSubmissionRef.current === identity
    const snapshot = captureSubmission?.()
    let accepted = false
    const accept = () => {
      if (accepted) return
      accepted = true
      if (snapshot) snapshot.accept()
      else clearDraft()
      if (!ownsRequest()) return
      setSubmitting(false)
      setQueueing(false)
      setComposerExecutionMode('plan')
      setToolsOpen(false)
      if (invocationRef.current === invocation) setInvocation(null)
      requestTranscriptBottom()
      if (promptRef.current) promptRef.current.style.height = 'auto'
      pendingSubmissionRef.current = null
    }
    setSubmitError('')
    if (streaming) setQueueing(true)
    else setSubmitting(true)
    try {
      if (streaming) {
        if (await onQueue?.(value, selection.attachments, 'steer', accept)) accept()
        return
      }
      const pendingGoalPause = goalPausePromiseRef.current
      if (pendingGoalPause) {
        await pendingGoalPause
        goalPausePromiseRef.current = null
      } else if (
        goal?.status === 'active' &&
        (composerExecutionMode === 'plan' || composerExecutionMode !== goal.mode)
      )
        await onGoalPause?.()
      if (!ownsRequest()) return
      const result = await onSend(
        value,
        selection.attachments,
        composerExecutionMode === 'goal' || composerExecutionMode === 'team',
        composerExecutionMode === 'team',
        composerExecutionMode === 'team'
          ? teamTokenBudget
          : composerExecutionMode === 'goal'
            ? goalTokenBudget
            : null,
        invocation,
        accept,
      )
      // 旧调用方以完成 Promise 表示接收，新调用方通过回调在 SSE 开始时确认。
      if (!accepted && result !== false) accept()
    } catch (error) {
      if (ownsRequest()) {
        goalPausePromiseRef.current = null
        setSubmitError(error instanceof Error ? error.message : String(error))
      }
    } finally {
      if (ownsRequest()) {
        pendingSubmissionRef.current = null
        setSubmitting(false)
        setQueueing(false)
      }
    }
  }

  return {
    composerExecutionMode,
    setComposerExecutionMode,
    goalTokenBudget,
    setGoalTokenBudget,
    teamTokenBudget,
    setTeamTokenBudget,
    queueing,
    submitting,
    stopping,
    submitError,
    stop,
    compactingManually,
    invocation,
    setInvocation,
    requestGoalPause,
    compactContext,
    submit,
  }
}
