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
    <div className="session-workflow-runs">
      {[...runs]
        .sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt))
        .slice(0, 5)
        .map((run) => {
          const open = expanded === run.id
          const progress = Math.round(
            ((Number(run.completedNodes) || 0) / Math.max(1, Number(run.totalNodes) || 1)) * 100,
          )
          return (
            <div className={`session-workflow-run ${run.status}`} key={run.id}>
              <button
                className="session-workflow-summary"
                onClick={() => setExpanded(open ? '' : run.id)}
              >
                <span className="list-icon">
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
                <div className="session-workflow-detail">
                  {run.sourceMessage && (
                    <div className="session-workflow-context">
                      <strong>{t('chat:workflowRun.request')}</strong>
                      <p>{run.sourceMessage}</p>
                    </div>
                  )}
                  {run.inputs && Object.keys(run.inputs).length > 0 && (
                    <div className="session-workflow-context">
                      <strong>{t('chat:workflowRun.inputs')}</strong>
                      <pre>{JSON.stringify(run.inputs, null, 2)}</pre>
                    </div>
                  )}
                  {(run.nodes || []).map((node: EntityRecord) => (
                    <div className={`session-workflow-node ${node.status}`} key={node.id}>
                      {node.status === 'completed' ? (
                        <Check size={13} />
                      ) : node.status === 'failed' ? (
                        <CircleX size={13} />
                      ) : node.status === 'running' ? (
                        <RefreshCw className="spin" size={13} />
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
