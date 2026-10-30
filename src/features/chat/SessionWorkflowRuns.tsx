// 会话工作流运行记录：展示会话关联的工作流运行历史与状态。
import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { Check, ChevronRight, CircleX, Square, Workflow, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { OrbitSpinner } from '@/components/ui/spinner'
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

// empty：会话页“⋯ → 工作流运行记录”对话框里没有记录时的说明；输入框上方的内联列表不传，保持不占位。
export function SessionWorkflowRuns({
  sessionId,
  empty = null,
}: {
  sessionId: string
  empty?: ReactNode
}) {
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

  if (!runs.length) return empty
  // 与工具活动日志同一语言：一张细线卡片，运行逐行以发丝线分隔；状态方块同时带形状与颜色，
  // 运行中使用品牌轨道指示，计数与进度用等宽数字。
  const context =
    'session-workflow-context grid min-w-0 gap-1 border-b border-stroke-soft pb-1.5 [&_strong]:text-xs [&_strong]:font-medium [&_strong]:text-content-muted [&_p]:max-h-[84px] [&_p]:overflow-auto [&_p]:text-xs [&_p]:leading-relaxed [&_p]:whitespace-pre-wrap [&_p]:text-content-secondary [&_p]:[overflow-wrap:anywhere] [&_pre]:max-h-[84px] [&_pre]:overflow-auto [&_pre]:rounded-xs [&_pre]:bg-code [&_pre]:px-1.5 [&_pre]:py-1 [&_pre]:font-mono [&_pre]:text-xs [&_pre]:leading-relaxed [&_pre]:whitespace-pre-wrap [&_pre]:text-content-secondary [&_pre]:[overflow-wrap:anywhere]'
  return (
    <div className="flex max-h-[210px] flex-col overflow-auto rounded-md border border-stroke bg-panel">
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
              className={`session-workflow-run border-b border-stroke-soft last:border-b-0 ${run.status}`}
              key={run.id}
            >
              <button
                className={`session-workflow-summary grid w-full cursor-pointer grid-cols-[20px_minmax(120px,1fr)_minmax(80px,160px)_14px] items-center gap-2.5 border-0 px-3 py-2 text-left text-content transition-colors duration-(--d-fast) hover:bg-surface-subtle focus-visible:-outline-offset-2 max-[650px]:min-h-11 max-[650px]:grid-cols-[20px_minmax(0,1fr)_14px] max-[650px]:[&_[data-slot='progress']]:col-[2/4] [&_[data-slot='progress']]:h-1 [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_small]:truncate [&_small]:text-xs [&_small]:text-content-muted [&_small]:tabular-nums [&_strong]:truncate [&_strong]:text-sm [&_strong]:font-medium ${open ? 'bg-surface-subtle' : 'bg-transparent'}`}
                aria-expanded={open}
                onClick={() => setExpanded(open ? '' : run.id)}
              >
                <span
                  className={`grid size-5 place-items-center rounded-xs ${run.status === 'completed' ? 'bg-success-soft text-success' : run.status === 'failed' ? 'bg-danger-soft text-danger' : run.status === 'waiting_approval' ? 'bg-warning-soft text-warning-strong' : 'bg-surface-muted text-content-muted'}`}
                >
                  {run.status === 'running' ? (
                    <OrbitSpinner className="size-3.5" />
                  ) : run.status === 'completed' ? (
                    <Check size={12} />
                  ) : run.status === 'failed' ? (
                    <X size={12} />
                  ) : (
                    <Workflow size={12} />
                  )}
                </span>
                <span>
                  <strong>{run.workflowName}</strong>
                  <small>
                    {statusCopy(run.status, t)} · v{run.workflowRevision || 1}
                  </small>
                </span>
                <Progress value={run.status === 'completed' ? 100 : progress} />
                <ChevronRight
                  size={14}
                  className={`text-content-muted transition-transform duration-(--d-fast) ${open ? 'rotate-90' : ''}`}
                />
              </button>
              {open && (
                <div className="session-workflow-detail flex flex-col gap-1.5 border-t border-stroke-soft bg-surface-subtle py-2 pr-3 pl-[42px] max-[650px]:pl-3 [&_>_p]:text-xs [&_>_p]:text-content-muted">
                  {run.sourceMessage && (
                    <div className={context}>
                      <strong>{t('chat:workflowRun.request')}</strong>
                      <p>{run.sourceMessage}</p>
                    </div>
                  )}
                  {run.inputs && Object.keys(run.inputs).length > 0 && (
                    <div className={context}>
                      <strong>{t('chat:workflowRun.inputs')}</strong>
                      <pre>{JSON.stringify(run.inputs, null, 2)}</pre>
                    </div>
                  )}
                  {(run.nodes || []).map((node: EntityRecord) => (
                    <div
                      className={`session-workflow-node grid min-h-6 grid-cols-[15px_minmax(0,1fr)_auto] items-center gap-1.5 text-content-muted [&.completed]:text-success [&.failed]:text-danger [&_>_div]:flex [&_>_div]:gap-1 [&_>_span]:flex [&_>_span]:min-w-0 [&_>_span]:flex-col [&_small]:truncate [&_small]:text-xs [&_small]:text-content-muted [&_strong]:text-xs [&_strong]:font-medium [&_strong]:text-content ${node.status}`}
                      key={node.id}
                    >
                      {node.status === 'completed' ? (
                        <Check size={13} />
                      ) : node.status === 'failed' ? (
                        <CircleX size={13} />
                      ) : node.status === 'running' ? (
                        <OrbitSpinner className="size-3.5" />
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
                      className="self-start"
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
