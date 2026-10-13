// 聚焦会话输入区状态与提交逻辑：执行模式/预算状态、资源调用选择、
// 排队与手动压缩守卫，以及「发送/排队」提交流程。
// 从 FocusSession.tsx 拆出：行为与原先逐行一致，仅改变代码所在位置。
import { useEffect, useRef, useState, type FormEvent, type RefObject } from 'react'
import { useI18n } from '@/app/i18n/use-i18n'
import { chatErrorMessage } from '@/features/chat/model/chat-errors'
import type { ChatAttachment, EntityRecord, ResourceInvocation } from '@/types/chat'
import type { ComposerExecutionMode } from '@/features/chat/components/runtime/GoalModeControl'

type ComposerSelection = {
  attachments: ChatAttachment[]
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
  hasMessages,
  value,
  updateValue,
  selection,
  clearDraft,
  promptRef,
  onCompact,
  onGoalPause,
  onSend,
  onQueue,
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
  hasMessages: boolean
  value: string
  updateValue: (value: string) => void
  selection: ComposerSelection
  clearDraft: () => void
  promptRef: RefObject<HTMLTextAreaElement | null>
  onCompact?: () => Promise<boolean> | boolean
  onGoalPause?: () => Promise<void> | void
  onSend: (
    value: string,
    attachments: ChatAttachment[],
    goalMode: boolean,
    teamMode: boolean,
    goalTokenBudget: number | null,
    invocation?: ResourceInvocation | null,
  ) => Promise<void> | void
  onQueue?: (
    value: string,
    attachments: ChatAttachment[],
    behavior: string,
  ) => Promise<boolean> | boolean
  setToolsOpen: (open: boolean) => void
  requestTranscriptBottom: () => void
}) {
  const { t } = useI18n()
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
  const [compactingManually, setCompactingManually] = useState(false)
  const [commandError, setCommandError] = useState('')
  const compactOperationRef = useRef<symbol | null>(null)
  const currentDraftRef = useRef({ sessionId, value })
  currentDraftRef.current = { sessionId, value }
  const [invocation, setInvocation] = useState<ResourceInvocation | null>(null)

  useEffect(() => {
    setComposerExecutionMode(normalizeComposerRunMode(runModeRef.current))
    goalPausePromiseRef.current = null
    setGoalTokenBudget(null)
    setTeamTokenBudget(null)
    setQueueing(false)
    setCompactingManually(false)
    compactOperationRef.current = null
    setCommandError('')
    setInvocation(null)
  }, [sessionId])
  useEffect(
    () => () => {
      compactOperationRef.current = null
    },
    [],
  )
  useEffect(() => setCommandError(''), [value])
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
    if (
      !onCompact ||
      !hasMessages ||
      streaming ||
      compactOperationRef.current ||
      compaction?.active
    ) {
      setCommandError(t('chat:commands.compactUnavailable'))
      return false
    }
    const operation = Symbol('compact')
    compactOperationRef.current = operation
    setCompactingManually(true)
    setCommandError('')
    try {
      const success = await onCompact()
      if (compactOperationRef.current !== operation) return false
      if (!success) setCommandError(t('chat:commands.compactFailed'))
      return success
    } catch (error) {
      if (compactOperationRef.current === operation) setCommandError(chatErrorMessage(error))
      return false
    } finally {
      if (compactOperationRef.current === operation) {
        compactOperationRef.current = null
        setCompactingManually(false)
      }
    }
  }
  // 提交输入：空输入不发送；流式中走排队，否则发送目标模式/资源命令并清空草稿。
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    // 内置命令在排队/目标/普通消息路径前处理；无效调用也不能发送给模型。
    if (/^\/compact(?:\s|$)/.test(value.trim())) {
      if (value.trim() !== '/compact' || selection.attachments.length || invocation) {
        setCommandError(t('chat:commands.compactOnly'))
        return
      }
      const submittedValue = value
      if (await compactContext()) {
        if (currentDraftRef.current.sessionId !== sessionId) return
        // 压缩等待期间允许继续编辑；仅清掉本次命令文本，不清附件或资源。
        if (currentDraftRef.current.value === submittedValue) updateValue('')
        setToolsOpen(false)
      }
      return
    }
    if (!value.trim() && !selection.attachments.length && !invocation) return
    if (streaming) {
      if ((!value.trim() && !selection.attachments.length) || queueing) return
      setQueueing(true)
      const queued = await onQueue?.(value, selection.attachments, 'steer')
      setQueueing(false)
      if (!queued) return
      clearDraft()
      setComposerExecutionMode('plan')
      setToolsOpen(false)
      requestTranscriptBottom()
      if (promptRef.current) promptRef.current.style.height = 'auto'
      return
    }
    const pendingGoalPause = goalPausePromiseRef.current
    if (pendingGoalPause) {
      try {
        await pendingGoalPause
      } catch {
        goalPausePromiseRef.current = null
        return
      }
      goalPausePromiseRef.current = null
    } else if (
      goal?.status === 'active' &&
      (composerExecutionMode === 'plan' || composerExecutionMode !== goal.mode)
    )
      await onGoalPause?.()
    onSend(
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
    )
    requestTranscriptBottom()
    clearDraft()
    setComposerExecutionMode('plan')
    setToolsOpen(false)
    setInvocation(null)
    if (promptRef.current) promptRef.current.style.height = 'auto'
  }

  return {
    composerExecutionMode,
    setComposerExecutionMode,
    goalTokenBudget,
    setGoalTokenBudget,
    teamTokenBudget,
    setTeamTokenBudget,
    queueing,
    compactingManually,
    commandError,
    invocation,
    setInvocation,
    requestGoalPause,
    compactContext,
    submit,
  }
}
