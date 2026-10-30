// 会话树对话框：会话从属关系（父子）的树形浏览。
// 数据请求与状态留在本文件；渲染拆到 session-tree-* 子组件，纯算法在 session-tree-model。
import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from 'react'
import { TreePine, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { useIsMobileApp } from '@/stores/client-store'
import type { SessionSummary } from '@/types/chat'
import { chatErrorMessage } from './chat-errors'
import {
  chatApi,
  type SessionTreeLabelMatch,
  type SessionTreeNode,
  type SessionTreeResponse,
} from './chat-api'
import { requestSessionSelection } from './events'
import { SessionTreeBrowser } from '@/features/chat/session-tree-browser'
import { SessionTreeInspector } from '@/features/chat/session-tree-inspector'
import {
  buildDisplayTree,
  conversationKinds,
  createSegment,
  filterTree,
  flattenTree,
  nonDerivableKinds,
  type TreeView,
} from '@/features/chat/session-tree-model'

export function SessionTreeDialog({
  open,
  sessionId,
  streaming,
  onClose,
  onNavigated,
  onCreateChildSession,
  initialView = 'conversation',
  returnFocusRef,
}: {
  open: boolean
  sessionId: string
  streaming: boolean
  // 会话页头的“标签”入口直接打开“已标记”页签，“分支”入口打开对话树。
  initialView?: TreeView
  // 打开对话框的页头按钮（“分支”“标签”）；为空时退回到会话页头的“⋯”。
  returnFocusRef?: RefObject<HTMLElement | null>
  onClose: () => void
  onNavigated: (editorText: string | null) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
}) {
  const { t, language } = useI18n()
  const mobileApp = useIsMobileApp()
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  const [data, setData] = useState<SessionTreeResponse | null>(null)
  const [sessions, setSessions] = useState<SessionSummary[]>([])
  const [view, setView] = useState<TreeView>('conversation')
  const [mobilePane, setMobilePane] = useState<'list' | 'detail'>('list')
  const [query, setQuery] = useState('')
  const [selectedId, setSelectedId] = useState('')
  const [label, setLabel] = useState('')
  const [allMarks, setAllMarks] = useState<SessionTreeLabelMatch[]>([])
  const [marksLoaded, setMarksLoaded] = useState(false)
  const [marksLoading, setMarksLoading] = useState(false)
  const [openingMark, setOpeningMark] = useState<SessionTreeLabelMatch | null>(null)
  const [summarize, setSummarize] = useState(false)
  const [loading, setLoading] = useState(false)
  const [savingLabel, setSavingLabel] = useState(false)
  const [navigating, setNavigating] = useState(false)
  const [creatingChild, setCreatingChild] = useState(false)
  const [openingRelatedId, setOpeningRelatedId] = useState('')
  const [error, setError] = useState('')
  const viewportRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!open || !sessionId) return
    let cancelled = false
    setView(initialView)
    setMobilePane('list')
    setQuery('')
    setLoading(true)
    setError('')
    void Promise.all([chatApi.getSessionTree(sessionId), chatApi.listSessions()])
      .then(([next, sessionData]) => {
        if (cancelled) return
        setData(next)
        setSessions(sessionData.sessions || [])
        setSelectedId(next.leafId || next.nodes[0]?.id || '')
      })
      .catch((reason) => {
        if (!cancelled) setError(chatErrorMessage(reason))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
    // initialView 只在打开那一刻生效；打开期间用户切换页签不应被它拉回。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, sessionId])

  // 弹窗关闭后立即释放可能超过万条的树数据，避免长期占用前端内存。
  useEffect(() => {
    if (open) return
    setData(null)
    setSessions([])
    setSelectedId('')
    setMobilePane('list')
    setLabel('')
    setOpeningMark(null)
    setSummarize(false)
    setError('')
  }, [open])

  // 首次切到「全部标记」页签时拉取跨会话标记列表（搜索索引已持久化，代价很低）。
  // 注意：marksLoading 只作防重入守卫，不能进依赖数组——effect 内同步 setState
  // 会触发重跑并把在途请求的 active 置 false，导致永远停在加载态。
  useEffect(() => {
    if (!open || view !== 'marks' || marksLoaded) return
    let active = true
    setMarksLoading(true)
    setError('')
    void chatApi
      .listSessionTreeLabels(500)
      .then((result) => {
        if (!active) return
        setAllMarks(result.labels || [])
        setMarksLoaded(true)
      })
      .catch((reason) => active && setError(chatErrorMessage(reason)))
      .finally(() => active && setMarksLoading(false))
    return () => {
      active = false
    }
  }, [open, view, marksLoaded])

  const visibleMarks = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase(language)
    if (!needle) return allMarks
    return allMarks.filter((mark) =>
      `${mark.label} ${mark.sessionName}`.toLocaleLowerCase(language).includes(needle),
    )
  }, [allMarks, language, query])

  // 打开标签定位：非活动条目先导航到目标条目（激活分支），
  // 再请求打开会话；防重入避免重复导航。
  const openMark = async (mark: SessionTreeLabelMatch) => {
    if (openingMark) return
    setOpeningMark(mark)
    setError('')
    try {
      if (!mark.active) {
        await chatApi.navigateSessionTreeTarget(mark.sessionId, mark.entryId)
      }
      requestSessionSelection(mark.sessionId, 'open', mark.entryId)
      onClose()
    } catch (reason) {
      setError(chatErrorMessage(reason))
      setOpeningMark(null)
    }
  }

  const annotatedRoots = useMemo(() => buildDisplayTree(data?.nodes || []), [data])
  // 节点类型 → 本地化标签（未知类型回退原始 type 字段）。
  const typeLabel = useCallback(
    (node: SessionTreeNode) => {
      const known: Record<string, string> = {
        user: t('chat:sessionTree.userMessage'),
        assistant: t('chat:sessionTree.agentMessage'),
        tool: t('chat:sessionTree.toolResult'),
        'tool-call': t('chat:sessionTree.toolCall'),
        summary: t('chat:sessionTree.branchSummary'),
        compaction: t('chat:sessionTree.compactionSummary'),
        settings: t('chat:sessionTree.settingsChange'),
        label: t('chat:sessionTree.labelChange'),
        position: t('chat:sessionTree.branchPosition'),
        extension: t('chat:sessionTree.extensionEntry'),
        metadata: t('chat:sessionTree.metadataEntry'),
        message: t('chat:sessionTree.message'),
      }
      return known[node.kind] || node.type
    },
    [t],
  )
  const visibleRoots = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase(language)
    return filterTree(annotatedRoots, (node) => {
      const inView =
        view === 'conversation'
          ? conversationKinds.has(node.kind)
          : view === 'labeled'
            ? Boolean(node.label)
            : true
      if (!inView) return false
      if (!needle) return true
      return `${node.label} ${node.text} ${typeLabel(node)}`
        .toLocaleLowerCase(language)
        .includes(needle)
    })
  }, [annotatedRoots, language, query, typeLabel, view])
  const segments = useMemo(() => visibleRoots.map(createSegment), [visibleRoots])
  const visibleNodes = useMemo(() => flattenTree(visibleRoots), [visibleRoots])
  const allNodes = useMemo(() => flattenTree(annotatedRoots), [annotatedRoots])
  const selected = allNodes.find((node) => node.id === selectedId) || null
  const parentSession = data?.lineage?.parentSessionId
    ? sessions.find((session) => session.id === data.lineage?.parentSessionId)
    : null
  const childSessions = (data?.lineage?.childSessionIds || [])
    .map((childId) => sessions.find((session) => session.id === childId))
    .filter((session): session is SessionSummary => Boolean(session))
  const canLabelSelected = selected?.kind === 'assistant' && selected.status === 'completed'
  const canCreateChildSelected = canLabelSelected
  const canDeriveSelected = selected ? !nonDerivableKinds.has(selected.kind) : false

  useEffect(() => {
    setLabel(selected?.label || '')
    setSummarize(false)
  }, [selected?.id, selected?.label])

  useEffect(() => {
    if (selectedId && visibleNodes.some((node) => node.id === selectedId)) return
    const activeNode = [...visibleNodes].reverse().find((node) => node.active)
    setSelectedId(activeNode?.id || visibleNodes[0]?.id || '')
  }, [selectedId, visibleNodes])

  useEffect(() => {
    if (!open || !selectedId) return
    const frame = requestAnimationFrame(() => {
      document
        .querySelector<HTMLElement>('[data-testid="session-tree-list"] .session-tree-node.selected')
        ?.scrollIntoView({ block: 'center' })
    })
    return () => cancelAnimationFrame(frame)
  }, [open, query, selectedId, view, visibleNodes.length])

  // 刷新会话树：拉取树数据并把选中项重置到叶子节点（或第一个节点）。
  const refresh = async () => {
    setLoading(true)
    setError('')
    try {
      const next = await chatApi.getSessionTree(sessionId)
      setData(next)
      setSelectedId(next.leafId || next.nodes[0]?.id || '')
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setLoading(false)
    }
  }

  // 保存选中节点的标签；防重入（savingLabel）。
  const saveLabel = async () => {
    if (!selected || savingLabel) return
    setSavingLabel(true)
    setError('')
    try {
      setData(await chatApi.setSessionTreeLabel(sessionId, selected.id, label))
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setSavingLabel(false)
    }
  }

  // 移除选中节点的标签（置空）；防重入。
  const removeLabel = async () => {
    if (!selected || savingLabel) return
    setSavingLabel(true)
    setError('')
    try {
      setData(await chatApi.setSessionTreeLabel(sessionId, selected.id, ''))
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setSavingLabel(false)
    }
  }

  // 从选中的已完成回复创建独立对话；创建成功后由外层打开新 Dock。
  const createChild = async () => {
    if (!selected || !canCreateChildSelected || creatingChild) return
    setCreatingChild(true)
    setError('')
    try {
      await onCreateChildSession(selected.id)
      onClose()
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setCreatingChild(false)
    }
  }

  // 在谱系中打开父会话或其它独立对话；父会话需要先定位到创建边界。
  const openRelatedSession = async (target: SessionSummary, targetEntryId = '') => {
    if (!target.id || openingRelatedId) return
    setOpeningRelatedId(target.id)
    setError('')
    try {
      if (targetEntryId) await chatApi.navigateSessionTreeTarget(target.id, targetEntryId)
      requestSessionSelection(target.id, 'open', targetEntryId)
      onClose()
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setOpeningRelatedId('')
    }
  }

  // 导航到选中节点（非叶子且非流式中）：切换到该历史位置继续会话。
  const navigate = async () => {
    if (!selected || navigating || selected.leaf || streaming || data?.streaming) return
    setNavigating(true)
    setError('')
    try {
      const next = await chatApi.navigateSessionTree(sessionId, selected.id, summarize)
      setData(next)
      if (!next.cancelled) {
        await onNavigated(next.editorText)
        onClose()
      }
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setNavigating(false)
    }
  }

  const stateLabel = (node: SessionTreeNode) =>
    node.leaf
      ? t('chat:sessionTree.current')
      : node.branchPoint
        ? t('chat:sessionTree.fork')
        : node.active
          ? t('chat:sessionTree.active')
          : ''

  const selectNode = (id: string) => {
    setSelectedId(id)
    if (mobileLayout) setMobilePane('detail')
  }

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <DialogContent
        onCloseAutoFocus={(event) => {
          const opener = returnFocusRef?.current
          const trigger = opener?.isConnected
            ? opener
            : document.getElementById(`task-${sessionId}-more`)
          // 菜单项已经卸载；只恢复仍然处于活动任务中的稳定入口，避免抢走切换后的焦点。
          if (trigger?.closest('[data-task-workspace]')?.getAttribute('data-active') === 'true') {
            event.preventDefault()
            trigger.focus()
          }
        }}
        className={`chat-resource-dialog session-tree-dialog grid-rows-[auto_minmax(0,1fr)] gap-0 overflow-hidden p-0 ${mobileLayout ? '!h-[calc(100dvh_-_16px)] !w-[calc(100vw_-_16px)] !max-w-none' : '!h-[min(760px,calc(100dvh_-_32px))] !w-[min(1120px,calc(100vw_-_32px))] !max-w-[1120px]'}`}
        showCloseButton={false}
      >
        <div className="chat-resource-head flex items-start justify-between gap-4 border-b border-stroke-soft px-[18px] pt-4 pb-3.5 max-[650px]:p-3.5 [&_[data-slot='dialog-description']]:mt-0.5 [&_[data-slot='dialog-description']]:text-xs [&_[data-slot='dialog-description']]:text-content-muted [&_[data-slot='dialog-title']]:text-base [&_[data-slot='dialog-title']]:font-semibold">
          <div>
            <div className="session-tree-title flex items-center gap-2.5">
              <span className="grid size-[30px] place-items-center rounded-sm border border-key-border bg-key text-content-secondary shadow-key">
                <TreePine size={16} />
              </span>
              <DialogTitle>{t('chat:sessionTree.title')}</DialogTitle>
            </div>
            <DialogDescription>
              {t('chat:sessionTree.description', {
                nodes: data?.nodeCount || 0,
                branches: data?.branchCount || 0,
              })}
            </DialogDescription>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="max-[650px]:size-11"
            aria-label={t('common:ui.closeDialog')}
            title={t('common:ui.closeDialog')}
            onClick={onClose}
          >
            <X size={16} />
          </Button>
        </div>
        <div
          className={`chat-resource-body session-tree-layout grid min-h-0 overflow-hidden bg-surface-subtle ${mobileLayout ? 'grid-cols-1 grid-rows-[minmax(0,1fr)]' : '!grid-cols-[minmax(0,1fr)_292px]'}`}
        >
          {(!mobileLayout || mobilePane === 'list') && (
            <SessionTreeBrowser
              view={view}
              onViewChange={(nextView) => {
                setView(nextView)
                setMobilePane('list')
              }}
              query={query}
              onQueryChange={setQuery}
              loading={loading}
              error={error}
              mobileLayout={mobileLayout}
              viewportRef={viewportRef}
              segments={segments}
              visibleNodes={visibleNodes}
              selectedId={selectedId}
              typeLabel={typeLabel}
              stateLabel={stateLabel}
              onSelect={selectNode}
              visibleMarks={visibleMarks}
              marksLoading={marksLoading}
              openingMark={openingMark}
              onOpenMark={(mark) => void openMark(mark)}
            />
          )}
          {view !== 'marks' && (!mobileLayout || mobilePane === 'detail') && (
            <SessionTreeInspector
              mobileLayout={mobileLayout}
              selected={selected}
              loading={loading}
              error={error}
              streaming={streaming}
              dataStreaming={Boolean(data?.streaming)}
              parentSession={parentSession ?? null}
              parentEntryId={data?.lineage?.sourceEntryId || ''}
              childSessions={childSessions}
              canLabel={canLabelSelected}
              canDerive={canDeriveSelected}
              label={label}
              summarize={summarize}
              savingLabel={savingLabel}
              creatingChild={creatingChild}
              navigating={navigating}
              openingRelatedId={openingRelatedId}
              typeLabel={typeLabel}
              stateLabel={stateLabel}
              onBackToList={() => setMobilePane('list')}
              onLabelChange={setLabel}
              onSummarizeChange={setSummarize}
              onSaveLabel={() => void saveLabel()}
              onRemoveLabel={() => void removeLabel()}
              onCreateChild={() => void createChild()}
              onNavigate={() => void navigate()}
              onRefresh={() => void refresh()}
              onOpenRelated={(target, targetEntryId) =>
                void openRelatedSession(target, targetEntryId ?? '')
              }
            />
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
