import { useCallback, useEffect, useMemo, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { Workflow, WorkflowRun, WorkflowsData } from './types'
import type { WorkflowFilter } from './workflow-templates'

export const EMPTY_WORKFLOWS_DATA: WorkflowsData = {
  workflows: [],
  runs: [],
  limits: { maxConcurrent: 4, running: 0 },
  notificationTargets: {
    browser: { enabled: false },
    feishu: { enabled: false },
    weixin: { enabled: false },
  },
  models: [],
  cwd: '',
}

export function workflowErrorMessage(caught: unknown) {
  return caught instanceof Error ? caught.message : String(caught)
}

export function useWorkflowCatalog({
  notify,
  requestConfirm,
  query,
}: {
  notify: Notify
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
  query: string
}) {
  const { t } = useI18n()
  const [data, setData] = useState<WorkflowsData>(EMPTY_WORKFLOWS_DATA)
  const [filter, setFilter] = useState<WorkflowFilter>('all')
  const [loading, setLoading] = useState(true)
  const [busyId, setBusyId] = useState('')
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      setData(await apiJson<WorkflowsData>('/api/workflows'))
      setError('')
    } catch (caught) {
      setError(workflowErrorMessage(caught))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  useEffect(() => {
    const timer = window.setInterval(
      () => {
        void load()
      },
      data.limits.running ? 1500 : 8000,
    )
    return () => window.clearInterval(timer)
  }, [data.limits.running, load])

  const latestRun = useCallback(
    (workflowId: string) =>
      data.runs
        .filter((run) => run.workflowId === workflowId)
        .sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt))[0],
    [data.runs],
  )

  const visibleWorkflows = useMemo(
    () =>
      data.workflows.filter((workflow) => {
        const run = latestRun(workflow.id)
        const matchesQuery = `${workflow.name} ${workflow.description}`
          .toLowerCase()
          .includes(query.toLowerCase())
        if (!matchesQuery) return false
        if (filter === 'running') return run?.status === 'running'
        if (filter === 'failed') return run?.status === 'failed'
        if (filter === 'draft') return workflow.status === 'draft'
        if (filter === 'presets') return false
        return true
      }),
    [data.workflows, filter, latestRun, query],
  )

  const runWorkflow = useCallback(
    async (workflow: Workflow) => {
      setBusyId(workflow.id)
      setError('')
      try {
        await apiJson(`/api/workflows/${encodeURIComponent(workflow.id)}/run`, {
          method: 'POST',
          body: '{}',
        })
        await load()
        notify(t('workflows:workflowsPage.workflowStarted'))
      } catch (caught) {
        const message = workflowErrorMessage(caught)
        setError(message)
        notify(message, 'error')
      } finally {
        setBusyId('')
      }
    },
    [load, notify, t],
  )

  const stopRun = useCallback(
    async (run: WorkflowRun) => {
      setBusyId(run.id)
      setError('')
      try {
        await apiJson(`/api/workflows/runs/${encodeURIComponent(run.id)}/stop`, {
          method: 'POST',
          body: '{}',
        })
        await load()
        notify(t('workflows:workflowsPage.stoppingWorkflow'), 'info')
      } catch (caught) {
        const message = workflowErrorMessage(caught)
        setError(message)
        notify(message, 'error')
      } finally {
        setBusyId('')
      }
    },
    [load, notify, t],
  )

  const removeWorkflow = useCallback(
    async (workflow: Workflow) => {
      const approved = await requestConfirm?.({
        title: t('workflows:workflowsPage.deleteWorkflow'),
        message: t('workflows:workflowsPage.deleteWorkflowNameAndItsRunHistory', {
          name: workflow.name,
        }),
        confirmLabel: t('workflows:workflowsPage.delete'),
        tone: 'danger',
      })
      if (!approved) return
      setBusyId(workflow.id)
      try {
        await apiJson(`/api/workflows/${encodeURIComponent(workflow.id)}`, { method: 'DELETE' })
        await load()
        notify(t('workflows:workflowsPage.workflowDeleted'))
      } catch (caught) {
        const message = workflowErrorMessage(caught)
        setError(message)
        notify(message, 'error')
      } finally {
        setBusyId('')
      }
    },
    [load, notify, requestConfirm, t],
  )

  return {
    data,
    filter,
    setFilter,
    loading,
    busyId,
    error,
    visibleWorkflows,
    latestRun,
    runWorkflow,
    stopRun,
    removeWorkflow,
  }
}
