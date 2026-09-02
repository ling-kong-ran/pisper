// 聚焦会话输入区状态与提交逻辑：执行模式/预算状态、资源调用选择、
// 排队与手动压缩守卫，以及「发送/排队」提交流程。
// 从 FocusSession.tsx 拆出：行为与原先逐行一致，仅改变代码所在位置。
import { useEffect, useRef, useState, type FormEvent, type RefObject } from 'react'
import type { ChatAttachment, EntityRecord, ResourceInvocation } from '@/types/chat'

type ComposerSelection = {
  attachments: ChatAttachment[]
}

export function useFocusComposer({
  sessionId,
  goalsAvailable,
  teamAvailable,
  workflowsAvailable,
  goal,
  streaming,
  compaction,
  value,
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
  goalsAvailable: boolean
  teamAvailable: boolean
  workflowsAvailable: boolean
  goal?: EntityRecord | null
  streaming?: boolean
  compaction?: EntityRecord | null
  value: string
  selection: ComposerSelection
  clearDraft: () => void
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
  ) => Promise<void> | void
  onQueue?: (
    value: string,
    attachments: ChatAttachment[],
    behavior: string,
  ) => Promise<boolean> | boolean
  setToolsOpen: (open: boolean) => void
  requestTranscriptBottom: () => void
}) {
  const [composerExecutionMode, setComposerExecutionMode] = useState<'plan' | 'goal' | 'team'>(
    'plan',
  )
  const [goalTokenBudget, setGoalTokenBudget] = useState<number | null>(null)
  const [teamTokenBudget, setTeamTokenBudget] = useState<number | null>(null)
  const goalPausePromiseRef = useRef<Promise<void> | null>(null)
  const [queueing, setQueueing] = useState(false)
  const [compactingManually, setCompactingManually] = useState(false)
  const [invocation, setInvocation] = useState<ResourceInvocation | null>(null)

  useEffect(() => {
    setComposerExecutionMode('plan')
    goalPausePromiseRef.current = null
    setGoalTokenBudget(null)
    setTeamTokenBudget(null)
    setQueueing(false)
    setCompactingManually(false)
    setInvocation(null)
  }, [sessionId])
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
  // 提交输入：空输入不发送；流式中走排队，否则发送目标模式/资源命令并清空草稿。
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
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
    invocation,
    setInvocation,
    requestGoalPause,
    compactContext,
    submit,
  }
}
