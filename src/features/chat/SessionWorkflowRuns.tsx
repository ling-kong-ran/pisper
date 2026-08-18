// 会话工作流运行记录：展示会话关联的工作流运行历史与状态。
import { useCallback, useEffect, useState } from 'react'
import {
  Check,
  ChevronDown,
  ChevronRight,
  CircleX,
  RefreshCw,
  Square,
  Workflow,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import type { EntityRecord } from '@/types/chat'
import { chatApi } from './chat-api'

function activeRun(run: EntityRecord) {
  return ['running', 'waiting_approval'].includes(String(run.status || ''))
}

function statusCopy(status: string, t: ReturnType<typeof useI18n>['t']) {
  if (status === 'running') return t('chat:workflowRun.running')
  if (status === 'waiting_approval') return t('chat:workflowRun.waitingApproval')
  if (status === 'completed') return t('chat:workflowRun.completed')
  if (status === 'failed') return t('chat:workflowRun.failed')
  if (status === 'cancelled') return t('chat:workflowRun.stopped')
  return t('chat:workflowRun.interrupted')
}

export function SessionWorkflowRuns({ sessionId }: { sessionId: string }) {
  const { t } = useI18n()
  const [runs, setRuns] = useState<EntityRecord[]>([])
  const [expanded, setExpanded] = useState('')

  const load = useCallback(async () => {
    try {
      const result = await chatApi.getSessionWorkflowRuns(sessionId)
      setRuns(result.runs || [])
    } catch {}
  }, [sessionId])

  useEffect(() => {
    void load()
    const timer = window.setInterval(load, 1500)
    return () => window.clearInterval(timer)
  }, [load])

  if (!runs.length) return null
  return (
    <div className="flex max-h-[210px] flex-col gap-[5px] overflow-auto">
      {[...runs]
        .sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt))
        .slice(0, 5)
        .map((run) => {
          const open = expanded === run.id
          const progress = Math.round(
            ((Number(run.completedNodes) || 0) / Math.max(1, Number(run.totalNodes) || 1)) * 100,
          )
          return (
            <div
              className={`session-workflow-run [&.failed]:border-[color-mix(in_srgb,var(--danger)_40%,var(--stroke-soft))] [&.waiting_approval]:border-[color-mix(in_srgb,var(--warning)_45%,var(--stroke-soft))] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--solid)] ${run.status}`}
              key={run.id}
            >
              <button
                className="session-workflow-summary [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[1px] [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[12px] [&_small]:text-[var(--text-muted)] [&_small]:text-[10px] [&_[data-slot='progress']]:h-[4px] max-[650px]:grid-cols-[28px_minmax(0,1fr)_16px] max-[650px]:[&_[data-slot='progress']]:[grid-column:2/4] grid w-full grid-cols-[28px_minmax(120px,1fr)_minmax(80px,160px)_16px] items-center gap-[9px] border-0 bg-transparent [padding:7px_9px] text-[var(--text)] text-left cursor-pointer"
                onClick={() => setExpanded(open ? '' : run.id)}
              >
                <span className="list-icon [.chat-resource-list_&]:grid [.chat-resource-list_&]:w-[28px] [.chat-resource-list_&]:h-[28px] [.chat-resource-list_&]:place-items-center [.chat-resource-list_&]:rounded-[var(--r-sm)] [.chat-resource-list_&]:bg-[var(--surface-subtle)] [.chat-resource-list_&]:text-[var(--star-strong)] [.session-workflow-summary_&]:grid [.session-workflow-summary_&]:w-[28px] [.session-workflow-summary_&]:h-[28px] [.session-workflow-summary_&]:place-items-center [.session-workflow-summary_&]:rounded-[var(--r-sm)] [.session-workflow-summary_&]:bg-[var(--surface-subtle)] [.session-workflow-summary_&]:text-[var(--star-strong)] grid w-[27px] h-[27px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)] [.workflow-template-gallery_&]:grid [.workflow-template-gallery_&]:w-[32px] [.workflow-template-gallery_&]:h-[32px] [.workflow-template-gallery_&]:place-items-center [.workflow-template-gallery_&]:rounded-[var(--r-sm)] [.workflow-template-gallery_&]:bg-[var(--surface-subtle)] [.workflow-template-gallery_&]:text-[var(--star-strong)]">
                  <Workflow size={14} />
                </span>
                <span>
                  <strong>{run.workflowName}</strong>
                  <small>
                    {statusCopy(run.status, t)} · v{run.workflowRevision || 1}
                  </small>
                </span>
                <Progress value={run.status === 'completed' ? 100 : progress} />
                {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
              </button>
              {open && (
                <div className="session-workflow-detail [&_>_p]:text-[var(--text-muted)] [&_>_p]:text-[11px] max-[650px]:pl-[12px] flex flex-col gap-[5px] [border-top:1px_solid_var(--stroke-soft)] [padding:7px_9px_9px_46px]">
                  {run.sourceMessage && (
                    <div className="session-workflow-context [&_strong]:text-[var(--text-muted)] [&_strong]:text-[10px] [&_strong]:[text-transform:uppercase] [&_p]:max-h-[84px] [&_p]:overflow-auto [&_p]:m-0 [&_p]:text-[var(--text-secondary)] [&_p]:text-[11px] [&_p]:leading-[1.45] [&_p]:whitespace-pre-wrap [&_p]:[overflow-wrap:anywhere] [&_pre]:max-h-[84px] [&_pre]:overflow-auto [&_pre]:m-0 [&_pre]:text-[var(--text-secondary)] [&_pre]:text-[11px] [&_pre]:leading-[1.45] [&_pre]:whitespace-pre-wrap [&_pre]:[overflow-wrap:anywhere] [&_pre]:rounded-[var(--r-xs)] [&_pre]:bg-[var(--surface-subtle)] [&_pre]:p-[5px_6px] [&_pre]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] grid min-w-0 gap-[3px] [border-bottom:1px_solid_var(--stroke-soft)] [padding-bottom:6px]">
                      <strong>{t('chat:workflowRun.request')}</strong>
                      <p>{run.sourceMessage}</p>
                    </div>
                  )}
                  {run.inputs && Object.keys(run.inputs).length > 0 && (
                    <div className="session-workflow-context [&_strong]:text-[var(--text-muted)] [&_strong]:text-[10px] [&_strong]:[text-transform:uppercase] [&_p]:max-h-[84px] [&_p]:overflow-auto [&_p]:m-0 [&_p]:text-[var(--text-secondary)] [&_p]:text-[11px] [&_p]:leading-[1.45] [&_p]:whitespace-pre-wrap [&_p]:[overflow-wrap:anywhere] [&_pre]:max-h-[84px] [&_pre]:overflow-auto [&_pre]:m-0 [&_pre]:text-[var(--text-secondary)] [&_pre]:text-[11px] [&_pre]:leading-[1.45] [&_pre]:whitespace-pre-wrap [&_pre]:[overflow-wrap:anywhere] [&_pre]:rounded-[var(--r-xs)] [&_pre]:bg-[var(--surface-subtle)] [&_pre]:p-[5px_6px] [&_pre]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] grid min-w-0 gap-[3px] [border-bottom:1px_solid_var(--stroke-soft)] [padding-bottom:6px]">
                      <strong>{t('chat:workflowRun.inputs')}</strong>
                      <pre>{JSON.stringify(run.inputs, null, 2)}</pre>
                    </div>
                  )}
                  {(run.nodes || []).map((node: EntityRecord) => (
                    <div
                      className={`session-workflow-node [&.completed]:text-[var(--success)] [&.failed]:text-[var(--danger)] [&_>_span]:flex [&_>_span]:min-w-0 [&_>_span]:flex-col [&_strong]:text-[var(--text)] [&_strong]:text-[11px] [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[10px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_>_div]:flex [&_>_div]:gap-[4px] grid grid-cols-[15px_minmax(0,1fr)_auto] items-center gap-[6px] min-h-[24px] text-[var(--text-muted)] ${node.status}`}
                      key={node.id}
                    >
                      {node.status === 'completed' ? (
                        <Check size={13} />
                      ) : node.status === 'failed' ? (
                        <CircleX size={13} />
                      ) : node.status === 'running' ? (
                        <RefreshCw className="animate-spin" size={13} />
                      ) : (
                        <Square size={11} />
                      )}
                      <span>
                        <strong>{node.label}</strong>
                        {(node.error || node.summary) && (
                          <small>{node.error || node.summary}</small>
                        )}
                      </span>
                      {node.status === 'waiting_approval' && (
                        <div>
                          <Button
                            size="xs"
                            onClick={async () => {
                              await chatApi.resolveWorkflowApproval(run.id, node.id, true)
                              await load()
                            }}
                          >
                            {t('chat:workflowRun.approve')}
                          </Button>
                          <Button
                            size="xs"
                            variant="destructive"
                            onClick={async () => {
                              await chatApi.resolveWorkflowApproval(run.id, node.id, false)
                              await load()
                            }}
                          >
                            {t('chat:workflowRun.reject')}
                          </Button>
                        </div>
                      )}
                    </div>
                  ))}
                  {(run.error || run.summary) && <p>{run.error || run.summary}</p>}
                  {activeRun(run) && (
                    <Button
                      size="xs"
                      variant="destructive"
                      onClick={async () => {
                        await chatApi.stopWorkflowRun(run.id)
                        await load()
                      }}
                    >
                      <Square data-icon="inline-start" />
                      {t('chat:workflowRun.stop')}
                    </Button>
                  )}
                </div>
              )}
            </div>
          )
        })}
    </div>
  )
}
