// 工作流目录 hook：拉取/搜索/保存/删除工作流，维护列表状态。
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
  skills: [],
  cwd: '',
}

// 工作流错误归一化为可展示文案。
export function workflowErrorMessage(caught: unknown) {
  return caught instanceof Error ? caught.message : String(caught)
}

// 工作流目录 hook：加载/搜索/新建/重命名/删除工作流，
// 维护列表、筛选、加载状态与错误，供工作流列表页使用。
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
    async (workflow: Workflow, inputs: Record<string, unknown> = {}) => {
      setBusyId(workflow.id)
      setError('')
      try {
        await apiJson(`/api/workflows/${encodeURIComponent(workflow.id)}/run`, {
          method: 'POST',
          body: JSON.stringify({ inputs }),
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
        await apiJson(`/api/workflow-runs/${encodeURIComponent(run.id)}/stop`, {
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

  const retryRun = useCallback(
    async (run: WorkflowRun) => {
      setBusyId(run.id)
      try {
        await apiJson(`/api/workflow-runs/${encodeURIComponent(run.id)}/retry`, {
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

  const resolveApproval = useCallback(
    async (run: WorkflowRun, nodeId: string, approved: boolean) => {
      setBusyId(nodeId)
      try {
        await apiJson(
          `/api/workflow-runs/${encodeURIComponent(run.id)}/approvals/${encodeURIComponent(nodeId)}`,
          { method: 'POST', body: JSON.stringify({ approved }) },
        )
        await load()
      } catch (caught) {
        const message = workflowErrorMessage(caught)
        setError(message)
        notify(message, 'error')
      } finally {
        setBusyId('')
      }
    },
    [load, notify],
  )

  const duplicateWorkflow = useCallback(
    async (workflow: Workflow) => {
      setBusyId(workflow.id)
      try {
        await apiJson(`/api/workflows/${encodeURIComponent(workflow.id)}/duplicate`, {
          method: 'POST',
          body: '{}',
        })
        await load()
        notify(t('workflows:workflowsPage.workflowDuplicated'))
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

  const exportWorkflow = useCallback(
    async (workflow: Workflow) => {
      try {
        const exported = await apiJson<Record<string, unknown>>(
          `/api/workflows/${encodeURIComponent(workflow.id)}/export`,
        )
        const blob = new Blob([JSON.stringify(exported, null, 2)], { type: 'application/json' })
        const url = URL.createObjectURL(blob)
        const anchor = document.createElement('a')
        anchor.href = url
        anchor.download = `${workflow.name.replace(/[\\/:*?"<>|]/g, '-')}.pisper-workflow.json`
        anchor.click()
        URL.revokeObjectURL(url)
      } catch (caught) {
        notify(workflowErrorMessage(caught), 'error')
      }
    },
    [notify],
  )

  const importWorkflow = useCallback(
    async (value: unknown) => {
      setBusyId('import')
      try {
        await apiJson('/api/workflows/import', {
          method: 'POST',
          body: JSON.stringify(value),
        })
        await load()
        notify(t('workflows:workflowsPage.workflowImported'))
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
    retryRun,
    resolveApproval,
    duplicateWorkflow,
    exportWorkflow,
    importWorkflow,
    removeWorkflow,
  }
}
