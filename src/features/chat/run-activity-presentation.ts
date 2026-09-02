// Agent 运行活动的共享文案与展示推导：工具/子代理活动标题映射、计划进度、
// 计划变更与压缩文案。从 AgentRunActivity 拆出的纯逻辑，不依赖 React。
// 注意：activityPresentation 与团队任务文案仍由源码守卫测试锁定在主文件内。
import { formatTokenCount } from '@/lib/format'
import { isPlanReadTool, isPlanWriteTool } from '@/lib/plan-protocol'
import type { I18nValues } from '@/app/i18n'
import type { EntityRecord } from '@/types/chat'

export type Translate = (message: string, values?: I18nValues) => string

export function toolActivityLabel(name: unknown, t: Translate) {
  if (name === 'read') return t('chat:agentRunActivity.readingFiles')
  if (name === 'grep') return t('chat:agentRunActivity.searchingContent')
  if (name === 'find') return t('chat:agentRunActivity.findingFiles')
  if (name === 'ls') return t('chat:agentRunActivity.browsingDirectories')
  if (name === 'edit') return t('chat:agentRunActivity.editingFiles')
  if (name === 'write') return t('chat:agentRunActivity.writingFiles')
  if (name === 'bash') return t('chat:agentRunActivity.runningCommands')
  if (name === 'memory_search') return t('chat:agentRunActivity.searchingMemory')
  if (name === 'memory_remember') return t('chat:agentRunActivity.savingMemory')
  if (name === 'spawn_agent') return t('chat:agentRunActivity.startingSubagent')
  if (name === 'list_agents') return t('chat:agentRunActivity.checkingSubagents')
  if (name === 'send_message') return t('chat:agentRunActivity.messagingSubagent')
  if (name === 'send_team_message') return t('chat:agentRunActivity.messagingTeamMember')
  if (name === 'list_team_members') return t('chat:agentRunActivity.checkingTeamMembers')
  if (name === 'run_team_workflow') return t('chat:agentRunActivity.runningTeamWorkflow')
  if (name === 'followup_task') return t('chat:agentRunActivity.addingSubagentTask')
  if (name === 'wait_agent') return t('chat:agentRunActivity.waitingForSubagent')
  if (name === 'interrupt_agent') return t('chat:agentRunActivity.interruptingSubagent')
  if (isPlanReadTool(name)) return t('chat:agentRunActivity.readingPlan')
  if (isPlanWriteTool(name)) return t('chat:agentRunActivity.updatingPlan')
  if (name === 'browser_automation') return t('chat:agentRunActivity.controllingBrowser')
  if (name === 'generate_visual') return t('chat:agentRunActivity.generatingVisualContent')
  if (name === 'discover_tools') return t('chat:agentRunActivity.discoveringTools')
  return t('chat:agentRunActivity.usingTool', {
    tool: String(name || t('chat:agentRunActivity.tools')),
  })
}

export function toolCompletedLabel(name: unknown, t: Translate) {
  if (name === 'read') return t('chat:agentRunActivity.filesRead')
  if (name === 'grep') return t('chat:agentRunActivity.searchCompleted')
  if (name === 'find') return t('chat:agentRunActivity.findCompleted')
  if (name === 'ls') return t('chat:agentRunActivity.directoriesBrowsed')
  if (name === 'edit') return t('chat:agentRunActivity.filesEdited')
  if (name === 'write') return t('chat:agentRunActivity.filesWritten')
  if (name === 'bash') return t('chat:agentRunActivity.commandCompleted')
  if (name === 'spawn_agent') return t('chat:agentRunActivity.subagentStarted')
  if (name === 'send_team_message') return t('chat:agentRunActivity.teamMessageSent')
  if (name === 'list_team_members') return t('chat:agentRunActivity.teamMembersListed')
  if (name === 'run_team_workflow') return t('chat:agentRunActivity.teamWorkflowStarted')
  if (name === 'wait_agent') return t('chat:agentRunActivity.subagentStatusUpdated')
  if (isPlanWriteTool(name)) return t('chat:agentRunActivity.planUpdated')
  if (name === 'discover_tools') return t('chat:agentRunActivity.toolsDiscovered')
  return t('chat:agentRunActivity.currentOperationCompleted')
}

export function agentActivityTitle(status: unknown, name: string, t: Translate) {
  if (status === 'queued') return t('chat:agentRunActivity.agentQueued', { name })
  if (status === 'starting') return t('chat:agentRunActivity.agentStarting', { name })
  if (status === 'running') return t('chat:agentRunActivity.agentRunning', { name })
  if (status === 'completed') return t('chat:agentRunActivity.agentCompleted', { name })
  if (status === 'interrupted') return t('chat:agentRunActivity.agentInterrupted', { name })
  if (status === 'failed') return t('chat:agentRunActivity.agentFailed', { name })
  return t('chat:agentRunActivity.agentStatusUpdated', { name })
}

export function planProgress(plan: EntityRecord | null | undefined, t: Translate) {
  const items = plan?.items || []
  const completed =
    plan?.counts?.completed ??
    items.filter((item: EntityRecord) => item.status === 'completed').length
  const active =
    plan?.counts?.inProgress ??
    items.filter((item: EntityRecord) => item.status === 'in_progress').length
  return (
    [
      items.length
        ? t('chat:agentRunActivity.completedTotalCompleted', { completed, total: items.length })
        : '',
      active ? t('chat:agentRunActivity.countInProgress', { count: active }) : '',
    ]
      .filter(Boolean)
      .join(' · ') || t('chat:agentRunActivity.planCleared')
  )
}

export function planChangeText(change: EntityRecord, t: Translate) {
  if (change.kind === 'removed')
    return t('chat:agentRunActivity.removedTitle', { title: change.title })
  if (change.status === 'completed')
    return t('chat:agentRunActivity.completedTitle', { title: change.title })
  if (change.status === 'in_progress')
    return t('chat:agentRunActivity.inProgressTitle', { title: change.title })
  if (change.status === 'blocked')
    return t('chat:agentRunActivity.blockedTitle', { title: change.title })
  if (change.kind === 'added') return t('chat:agentRunActivity.addedTitle', { title: change.title })
  return t('chat:agentRunActivity.pendingTitle', { title: change.title })
}

export function compactionText(compaction: EntityRecord | null | undefined, t: Translate) {
  if (compaction?.active) {
    return compaction.reason === 'overflow'
      ? t('chat:agentRunActivity.contextIsFullTheRequestWillRetryAfterCompaction')
      : compaction.reason === 'manual'
        ? t('chat:agentRunActivity.summarizingOlderMessagesAsRequested')
        : t('chat:agentRunActivity.contextIsNearingItsLimitSummarizingOlderMessages')
  }
  if (compaction?.tokensBefore != null && compaction?.estimatedTokensAfter != null) {
    return t('chat:agentRunActivity.contextCompactedBeforeAfterTokens', {
      before: formatTokenCount(compaction.tokensBefore),
      after: formatTokenCount(compaction.estimatedTokensAfter),
    })
  }
  return t('chat:agentRunActivity.summarizingOlderMessages')
}

export type ActivityPresentationContext = {
  t: Translate
  streaming?: boolean
  text?: string
  thinkingText?: string
  compaction?: EntityRecord | null
  error?: string
  stopped?: boolean
  notice?: string
  lastActivityAt?: string | null
  now: number
}

export type ActivityPresentation = {
  tone: string
  title: string
  detail: string
  output: string
  command: boolean
  startedAt: unknown
  changes: EntityRecord[]
}
