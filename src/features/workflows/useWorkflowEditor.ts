// 工作流编辑器核心 hook：节点的增删改查、连线与撤销式编辑，
// 负责把编辑状态序列化保存到运行时。
import { useCallback, useEffect, useMemo, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import {
  getBrowserNotificationPermission,
  prepareBrowserNotifications,
  requestBrowserNotificationPermission,
} from '@/lib/browser-notifications'
import { wouldCreateWorkflowCycle } from '@shared/workflow-graph.mjs'
import type { Notify } from '@/app/route-context'
import type { NotificationSettingsData } from '@/types/notifications'
import type { DesktopNotificationPermission } from '@/types/update'
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

// 工作流编辑器 hook：节点/连线编辑、保存、运行，
// 同步编辑状态到画布与检查器，并向壳层注册工作流动作。
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
  const desktopNotifications = Boolean(window.pisperDesktop?.showNotification)
  const [systemNotificationPermission, setSystemNotificationPermission] =
    useState<DesktopNotificationPermission>(() =>
      desktopNotifications ? 'checking' : getBrowserNotificationPermission(),
    )

  // 刷新系统通知权限（桌面桥接或浏览器），供触发器节点配置使用。
  const refreshSystemNotificationPermission = useCallback(async () => {
    if (!desktopNotifications) {
      setSystemNotificationPermission(getBrowserNotificationPermission())
      return
    }
    const getStatus = window.pisperDesktop?.getNotificationStatus
    if (!getStatus) {
      setSystemNotificationPermission('granted')
      return
    }
    try {
      const result = await getStatus()
      const permission = result?.permission
      setSystemNotificationPermission(
        permission === 'default' ||
          permission === 'denied' ||
          permission === 'granted' ||
          permission === 'checking' ||
          permission === 'unsupported'
          ? permission
          : result?.supported === false
            ? 'unsupported'
            : 'granted',
      )
    } catch {
      setSystemNotificationPermission('unsupported')
    }
  }, [desktopNotifications])

  // 加载工作流：按 id 取已存工作流，或按模板/空白新建草稿，
  // 并恢复选中节点与系统通知权限。
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

  useEffect(() => {
    void refreshSystemNotificationPermission()
    const refresh = () => void refreshSystemNotificationPermission()
    window.addEventListener('focus', refresh)
    return () => window.removeEventListener('focus', refresh)
  }, [refreshSystemNotificationPermission])

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

  // 更新草稿元数据（名称/描述等）。
  const updateDraft = useCallback((patch: Partial<Workflow>) => {
    setDraft((current) => (current ? { ...current, ...patch } : current))
  }, [])

  // 更新选中节点的字段。
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

  // 选中节点（互斥清除连线选中）。
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

  // 新增连线：拦截自环/重复连线/成环（wouldCreateWorkflowCycle），
  // 触发器不可作为目标，合法则加入草稿并选中新边。
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

  // 批量删除连线（并清空选中）。
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

  // 批量删除节点：同时移除关联的入/出边。
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

  // 新增节点：生成唯一 id 并加入草稿，随后选中新节点。
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

  // 移动节点（仅位置变化的项写回，避免无谓更新）。
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

  // 复制选中节点（偏移位置 + “副本”后缀），复制后选中新节点。
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

  // 删除选中节点。
  const deleteNode = useCallback(() => {
    if (selectedNode) removeNodes([selectedNode.id])
  }, [removeNodes, selectedNode])

  // 切换通知目标的启停：非浏览器目标直接增删；浏览器目标开启前先
  // 获取/刷新权限，未授权提示，首次启用时顺带开启系统通知设置。
  const toggleNotification = useCallback(
    async (target: NotificationTarget) => {
      if (!selectedNode || selectedNode.kind !== 'notification') return
      const targets = selectedNode.notificationTargets
      if (target !== 'browser') {
        if (targets.includes(target)) {
          updateNode({ notificationTargets: targets.filter((item) => item !== target) })
          return
        }
        if (!catalog.notificationTargets[target]?.enabled) return
        updateNode({ notificationTargets: [...targets, target] })
        return
      }
      if (
        targets.includes(target) &&
        catalog.notificationTargets.browser.enabled &&
        systemNotificationPermission === 'granted'
      ) {
        updateNode({ notificationTargets: targets.filter((item) => item !== target) })
        return
      }

      let permission = systemNotificationPermission
      if (desktopNotifications) {
        await refreshSystemNotificationPermission()
        const status = await window.pisperDesktop?.getNotificationStatus?.()
        permission = status?.permission || (status?.supported === false ? 'unsupported' : 'granted')
      } else {
        permission = await requestBrowserNotificationPermission()
        setSystemNotificationPermission(permission)
        if (permission === 'granted') {
          try {
            await prepareBrowserNotifications()
          } catch (caught) {
            notify(workflowErrorMessage(caught), 'error')
            return
          }
        }
      }
      if (permission !== 'granted') {
        notify(
          permission === 'unsupported'
            ? t('workflows:workflowsPage.systemNotificationsUnsupported')
            : t('workflows:workflowsPage.systemNotificationPermissionRequired'),
          'error',
        )
        return
      }

      try {
        const settings = await apiJson<NotificationSettingsData>(
          '/api/settings/notifications/browser',
          { method: 'PATCH', body: JSON.stringify({ enabled: true }) },
        )
        setCatalog((current) => ({
          ...current,
          notificationTargets: {
            ...current.notificationTargets,
            browser: { enabled: settings.browser.enabled },
          },
        }))
        updateNode({
          notificationTargets: targets.includes(target) ? targets : [...targets, target],
        })
        notify(t('workflows:workflowsPage.systemNotificationsEnabled'))
      } catch (caught) {
        notify(workflowErrorMessage(caught), 'error')
      }
    },
    [
      catalog.notificationTargets,
      desktopNotifications,
      notify,
      refreshSystemNotificationPermission,
      selectedNode,
      systemNotificationPermission,
      t,
      updateNode,
    ],
  )

  // 保存工作流：过滤未启用的通知目标，新建走 POST、已有走 PATCH，
  // 保存后回写草稿（避免保存期间编辑被覆盖），新建时回调 created。
  const saveWorkflow = useCallback(
    async (status: Workflow['status'] = 'draft', quiet = false) => {
      if (!draft) return null
      setBusy(true)
      setError('')
      try {
        const enabledTargets = new Set(
          Object.entries(catalog.notificationTargets)
            .filter(
              ([id, target]) =>
                target.enabled &&
                (id !== 'browser' ||
                  systemNotificationPermission === 'granted' ||
                  systemNotificationPermission === 'checking'),
            )
            .map(([id]) => id),
        )
        const payload = {
          ...draft,
          status,
          notifications: draft.notifications.filter((target) => enabledTargets.has(target)),
          nodes: draft.nodes.map((node) => ({
            ...node,
            notificationTargets: node.notificationTargets.filter((target) =>
              enabledTargets.has(target),
            ),
          })),
        }
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
    [catalog.notificationTargets, draft, notify, onCreated, systemNotificationPermission, t],
  )

  // 运行工作流：先静默保存（确保最新状态），再触发运行并刷新目录。
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

  // 停止运行中的工作流（仅当前运行且状态为 running 时）。
  const stopWorkflow = useCallback(async () => {
    if (!currentRun || currentRun.status !== 'running') return
    setBusy(true)
    try {
      await apiJson(`/api/workflow-runs/${encodeURIComponent(currentRun.id)}/stop`, {
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
    systemNotificationPermission,
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
