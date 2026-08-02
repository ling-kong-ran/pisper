import { useCallback, useEffect, useMemo, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { wouldCreateWorkflowCycle } from '@shared/workflow-graph.mjs'
import type { Notify } from '@/app/route-context'
import type {
  NodeKind,
  NotificationTarget,
  Workflow,
  WorkflowEdge,
  WorkflowMutationResult,
  WorkflowNode,
  WorkflowsData,
} from './types'
import { EMPTY_WORKFLOWS_DATA, workflowErrorMessage } from './useWorkflowCatalog'
import {
  blankWorkflow,
  createWorkflowNode,
  NODE_TYPE_NAMES,
  templateWorkflow,
  WORKFLOW_TEMPLATES,
} from './workflow-templates'

export function useWorkflowEditor({
  workflowId,
  templateId,
  notify,
  onCreated,
}: {
  workflowId: string
  templateId: string | null
  notify: Notify
  onCreated: (workflowId: string) => void
}) {
  const { t } = useI18n()
  const [catalog, setCatalog] = useState<WorkflowsData>(EMPTY_WORKFLOWS_DATA)
  const [draft, setDraft] = useState<Workflow | null>(null)
  const [selectedNodeId, setSelectedNodeId] = useState('')
  const [selectedEdgeId, setSelectedEdgeId] = useState('')
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      const result = await apiJson<WorkflowsData>('/api/workflows')
      setCatalog(result)
      const stored =
        workflowId !== 'new'
          ? result.workflows.find((workflow) => workflow.id === workflowId)
          : null
      const template = WORKFLOW_TEMPLATES.find((item) => item.id === templateId)
      const next = stored
        ? structuredClone(stored)
        : template
          ? templateWorkflow(template, result.cwd)
          : blankWorkflow(result.cwd)
      setDraft(next)
      setSelectedNodeId((current) =>
        next.nodes.some((item) => item.id === current) ? current : next.nodes[0]?.id || '',
      )
      setSelectedEdgeId('')
      setError(
        stored || workflowId === 'new'
          ? ''
          : t('workflows:workflowsPage.workflowNotFoundABlankEditorWasOpened'),
      )
    } catch (caught) {
      setError(workflowErrorMessage(caught))
    } finally {
      setLoading(false)
    }
  }, [t, templateId, workflowId])

  useEffect(() => {
    void load()
  }, [load])

  const currentRun = useMemo(
    () =>
      catalog.runs
        .filter((run) => run.workflowId === draft?.id)
        .sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt))[0],
    [catalog.runs, draft?.id],
  )
  const running = currentRun?.status === 'running'

  useEffect(() => {
    if (!running) return undefined
    const timer = window.setInterval(async () => {
      try {
        setCatalog(await apiJson<WorkflowsData>('/api/workflows'))
      } catch {}
    }, 1500)
    return () => window.clearInterval(timer)
  }, [running])

  const selectedNode = useMemo(
    () => draft?.nodes.find((item) => item.id === selectedNodeId) || null,
    [draft?.nodes, selectedNodeId],
  )
  const selectedEdge = useMemo(
    () => draft?.edges.find((edge) => edge.id === selectedEdgeId) || null,
    [draft?.edges, selectedEdgeId],
  )

  const updateDraft = useCallback((patch: Partial<Workflow>) => {
    setDraft((current) => (current ? { ...current, ...patch } : current))
  }, [])

  const updateNode = useCallback(
    (patch: Partial<WorkflowNode>) => {
      setDraft((current) =>
        current
          ? {
              ...current,
              nodes: current.nodes.map((item) =>
                item.id === selectedNodeId ? { ...item, ...patch } : item,
              ),
            }
          : current,
      )
    },
    [selectedNodeId],
  )

  const selectNode = useCallback((id: string) => {
    setSelectedNodeId(id)
    setSelectedEdgeId('')
  }, [])

  const selectEdge = useCallback((id: string) => {
    setSelectedEdgeId(id)
    setSelectedNodeId('')
  }, [])

  const clearSelection = useCallback(() => {
    setSelectedNodeId('')
    setSelectedEdgeId('')
  }, [])

  const addEdge = useCallback(
    (source: string, target: string, sourcePort = 'output') => {
      if (!draft || source === target) return
      const targetNode = draft.nodes.find((item) => item.id === target)
      if (!targetNode || targetNode.kind === 'trigger') {
        notify(t('workflows:workflowsPage.aTriggerCannotHaveAnUpstreamConnection'), 'error')
        return
      }
      if (
        draft.edges.some(
          (edge) =>
            edge.source === source && edge.target === target && edge.sourcePort === sourcePort,
        )
      ) {
        notify(t('workflows:workflowsPage.thisConnectionAlreadyExists'), 'info')
        return
      }
      if (wouldCreateWorkflowCycle(draft.nodes, draft.edges, source, target, sourcePort)) {
        notify(t('workflows:workflowsPage.aWorkflowCannotContainCyclicConnections'), 'error')
        return
      }
      const edge: WorkflowEdge = {
        id: crypto.randomUUID(),
        source,
        sourcePort,
        target,
        targetPort: 'input',
      }
      setDraft((current) => (current ? { ...current, edges: [...current.edges, edge] } : current))
      setSelectedEdgeId(edge.id)
      setSelectedNodeId('')
      notify(t('workflows:workflowsPage.connectionCreated'), 'info')
    },
    [draft, notify, t],
  )

  const removeEdges = useCallback(
    (edgeIds: string[]) => {
      if (!edgeIds.length) return
      const ids = new Set(edgeIds)
      setDraft((current) =>
        current
          ? { ...current, edges: current.edges.filter((edge) => !ids.has(edge.id)) }
          : current,
      )
      setSelectedEdgeId('')
      notify(t('workflows:workflowsPage.connectionDeleted'), 'info')
    },
    [notify, t],
  )

  const removeSelectedEdge = useCallback(() => {
    if (selectedEdgeId) removeEdges([selectedEdgeId])
  }, [removeEdges, selectedEdgeId])

  const removeNodes = useCallback(
    (nodeIds: string[]) => {
      if (!nodeIds.length) return
      const ids = new Set(nodeIds)
      setDraft((current) =>
        current
          ? {
              ...current,
              nodes: current.nodes.filter((item) => !ids.has(item.id)),
              edges: current.edges.filter((edge) => !ids.has(edge.source) && !ids.has(edge.target)),
            }
          : current,
      )
      clearSelection()
      notify(t('workflows:workflowsPage.nodeDeleted'), 'info')
    },
    [clearSelection, notify, t],
  )

  const addNode = useCallback(
    (kind: NodeKind, label: string, position: { x: number; y: number }) => {
      const id = crypto.randomUUID()
      setDraft((current) =>
        current
          ? {
              ...current,
              nodes: [
                ...current.nodes,
                createWorkflowNode(
                  id,
                  kind,
                  label || NODE_TYPE_NAMES[kind],
                  '',
                  position.x,
                  position.y,
                ),
              ],
            }
          : current,
      )
      setSelectedNodeId(id)
      setSelectedEdgeId('')
    },
    [],
  )

  const moveNode = useCallback((id: string, position: { x: number; y: number }) => {
    setDraft((current) =>
      current
        ? {
            ...current,
            nodes: current.nodes.map((item) =>
              item.id === id && (item.x !== position.x || item.y !== position.y)
                ? { ...item, x: position.x, y: position.y }
                : item,
            ),
          }
        : current,
    )
  }, [])

  const copyNode = useCallback(() => {
    if (!selectedNode) return
    const id = crypto.randomUUID()
    setDraft((current) =>
      current
        ? {
            ...current,
            nodes: [
              ...current.nodes,
              {
                ...selectedNode,
                id,
                label: `${selectedNode.label} 副本`,
                x: selectedNode.x + 25,
                y: selectedNode.y + 25,
              },
            ],
          }
        : current,
    )
    setSelectedNodeId(id)
    setSelectedEdgeId('')
    notify(t('workflows:workflowsPage.nodeDuplicated'), 'info')
  }, [notify, selectedNode, t])

  const deleteNode = useCallback(() => {
    if (selectedNode) removeNodes([selectedNode.id])
  }, [removeNodes, selectedNode])

  const toggleNotification = useCallback(
    (target: NotificationTarget) => {
      if (!draft) return
      updateDraft({
        notifications: draft.notifications.includes(target)
          ? draft.notifications.filter((item) => item !== target)
          : [...draft.notifications, target],
      })
    },
    [draft, updateDraft],
  )

  const saveWorkflow = useCallback(
    async (status: Workflow['status'] = 'draft', quiet = false) => {
      if (!draft) return null
      setBusy(true)
      setError('')
      try {
        const payload = { ...draft, status }
        const result = draft.id
          ? await apiJson<WorkflowMutationResult>(
              `/api/workflows/${encodeURIComponent(draft.id)}`,
              { method: 'PATCH', body: JSON.stringify(payload) },
            )
          : await apiJson<WorkflowMutationResult>('/api/workflows', {
              method: 'POST',
              body: JSON.stringify(payload),
            })
        setCatalog(result.state)
        setDraft(structuredClone(result.workflow))
        if (!draft.id) onCreated(result.workflow.id)
        if (!quiet) {
          notify(
            status === 'published'
              ? t('workflows:workflowsPage.workflowPublished')
              : t('workflows:workflowsPage.workflowDraftSaved'),
          )
        }
        return result.workflow
      } catch (caught) {
        const message = workflowErrorMessage(caught)
        setError(message)
        notify(message, 'error')
        return null
      } finally {
        setBusy(false)
      }
    },
    [draft, notify, onCreated, t],
  )

  const runWorkflow = useCallback(async () => {
    const workflow = await saveWorkflow(draft?.status || 'draft', true)
    if (!workflow) return
    setBusy(true)
    try {
      await apiJson(`/api/workflows/${encodeURIComponent(workflow.id)}/run`, {
        method: 'POST',
        body: '{}',
      })
      setCatalog(await apiJson<WorkflowsData>('/api/workflows'))
      notify(t('workflows:workflowsPage.workflowStarted'))
    } catch (caught) {
      const message = workflowErrorMessage(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }, [draft?.status, notify, saveWorkflow, t])

  const stopWorkflow = useCallback(async () => {
    if (!currentRun || currentRun.status !== 'running') return
    setBusy(true)
    try {
      await apiJson(`/api/workflows/runs/${encodeURIComponent(currentRun.id)}/stop`, {
        method: 'POST',
        body: '{}',
      })
      setCatalog(await apiJson<WorkflowsData>('/api/workflows'))
      notify(t('workflows:workflowsPage.stoppingWorkflow'), 'info')
    } catch (caught) {
      const message = workflowErrorMessage(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }, [currentRun, notify, t])

  const publishWorkflow = useCallback(() => saveWorkflow('published'), [saveWorkflow])

  return {
    catalog,
    draft,
    selectedNode,
    selectedEdge,
    selectedNodeId,
    selectedEdgeId,
    currentRun,
    loading,
    busy,
    running,
    error,
    updateDraft,
    updateNode,
    addEdge,
    removeEdges,
    removeSelectedEdge,
    removeNodes,
    addNode,
    moveNode,
    copyNode,
    deleteNode,
    toggleNotification,
    selectNode,
    selectEdge,
    clearSelection,
    saveWorkflow,
    runWorkflow,
    stopWorkflow,
    publishWorkflow,
  }
}
