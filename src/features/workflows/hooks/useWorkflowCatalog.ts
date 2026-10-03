// 工作流目录 hook：拉取/搜索/保存/删除工作流，维护列表状态。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useI18n, translateText } from '@/app/i18n/use-i18n'
import { ApiError } from '@/lib/http/http'
import { apiJson } from '@/lib/http/api'
import type { Notify } from '@/app/routes/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { Workflow, WorkflowRun, WorkflowsData } from '@/features/workflows/model/types'
import type { WorkflowFilter } from '@/features/workflows/model/workflow-templates'
import {
  exportWorkflowPackage,
  importWorkflowPackage,
  isWorkflowPackageTooLarge,
} from '@/features/workflows/api/workflow-transfer-api'

export const EMPTY_WORKFLOWS_DATA: WorkflowsData = {
  workflows: [],
  runs: [],
  limits: { maxConcurrent: 4, running: 0 },
  notificationTargets: {
    browser: { enabled: false },
    feishu: { enabled: false },
    weixin: { enabled: false },
    qq: { enabled: false },
    telegram: { enabled: false },
  },
  models: [],
  skills: [],
  cwd: '',
}

// 工作流错误归一化为可展示文案。
export function workflowErrorMessage(caught: unknown) {
  const code =
    caught instanceof ApiError
      ? caught.data?.code
      : typeof caught === 'string'
        ? caught
        : caught && typeof caught === 'object' && 'code' in caught
          ? caught.code
          : ''
  if (code === 'workflow_image_source_stale')
    return translateText('workflows:imageNodes.sourceStale')
  if (code === 'workflow_image_source_required')
    return translateText('workflows:imageNodes.sourceRequired')
  if (code === 'workflow_image_engine_missing')
    return translateText('workflows:imageNodes.engineMissing')
  if (code === 'workflow_image_generation_failed')
    return translateText('workflows:imageNodes.generationFailed')
  if (code === 'workflow_image_too_large') return translateText('workflows:imageNodes.tooLarge')
  if (code === 'workflow_image_timeout') return translateText('workflows:imageNodes.timeout')
  if (typeof code === 'string' && code.startsWith('workflow_image_'))
    return translateText('workflows:imageNodes.invalidOutput')
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
  const transfer = useRef<AbortController | null>(null)
  useEffect(() => () => transfer.current?.abort(), [])

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
        return true
      } catch (caught) {
        const message = workflowErrorMessage(caught)
        setError(message)
        notify(message, 'error')
        return false
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
      if (transfer.current) return
      const controller = new AbortController()
      transfer.current = controller
      setBusyId(workflow.id)
      try {
        const blob = await exportWorkflowPackage(workflow.id, controller.signal)
        if (controller.signal.aborted) return
        const url = URL.createObjectURL(blob)
        const anchor = document.createElement('a')
        anchor.href = url
        anchor.download = `${workflow.name.replace(/[\\/:*?"<>|]/g, '-')}.pisper-workflow.zip`
        anchor.click()
        window.setTimeout(() => URL.revokeObjectURL(url), 1000)
      } catch {
        if (!controller.signal.aborted) notify(t('workflows:workflowsPage.packageFailed'), 'error')
      } finally {
        transfer.current = null
        if (!controller.signal.aborted) setBusyId('')
      }
    },
    [notify, t],
  )

  const importWorkflow = useCallback(
    async (value: File) => {
      if (transfer.current) return
      const controller = new AbortController()
      transfer.current = controller
      setBusyId('import')
      try {
        await importWorkflowPackage(value, controller.signal)
        if (controller.signal.aborted) return
        await load()
        if (controller.signal.aborted) return
        notify(t('workflows:workflowsPage.workflowImported'))
      } catch (caught) {
        if (controller.signal.aborted) return
        const message = isWorkflowPackageTooLarge(caught)
          ? t('workflows:workflowsPage.packageTooLarge')
          : t('workflows:workflowsPage.packageFailed')
        setError(message)
        notify(message, 'error')
      } finally {
        transfer.current = null
        if (!controller.signal.aborted) setBusyId('')
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
