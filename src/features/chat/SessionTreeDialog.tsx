// 会话树对话框（规格 v2 第 4.5 节）：分支 / 标记 / 全部标记三个页，合并了原来的分支与标签两个入口。
// 数据请求与状态留在本文件；渲染拆到 session-tree-* 子组件，纯算法在 session-tree-model。
// 会改变当前分支的操作（从此处继续、打开另一条分支上的标记）完成后提示可撤销；打开另一条分支上的标记先确认。
// “返回原对话”只打开父会话并定位到派生点，不再导航父会话的树（老问题 4：那样会把父会话退回派生点）。
import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from 'react'
import { GitFork, X } from 'lucide-react'
import type { Notify } from '@/app/route-context'
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
import { notifyTreeNavigation } from './session-tree-actions'
import { applySessionTreeResponse, invalidateSessionTreeSummary } from './session-tree-summary'
import { SessionTreeBrowser } from '@/features/chat/session-tree-browser'
import { SessionTreeInspector } from '@/features/chat/session-tree-inspector'
import {
  buildDisplayTree,
  createSegment,
  filterTree,
  flattenTree,
  nodeInView,
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
  initialView = 'branches',
  returnFocusRef,
  notify,
}: {
  open: boolean
  sessionId: string
  streaming: boolean
  // 默认打开“分支”页；手机标题菜单的“标签”一行直接打开“标记”页。
  initialView?: TreeView
  // 打开对话框的页头按钮（“分支”“标签”）；为空时退回到会话页头的“⋯”。
  returnFocusRef?: RefObject<HTMLElement | null>
  onClose: () => void
  onNavigated: (editorText: string | null) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
  notify?: Notify
}) {
  const { t, language } = useI18n()
  const mobileApp = useIsMobileApp()
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  const [data, setData] = useState<SessionTreeResponse | null>(null)
  const [sessions, setSessions] = useState<SessionSummary[]>([])
  const [view, setView] = useState<TreeView>('branches')
  // “分支”页默认只显示对话条目；勾选后连工具调用、设置等全部条目一起显示。
  const [showAllEntries, setShowAllEntries] = useState(false)
  const [mobilePane, setMobilePane] = useState<'list' | 'detail'>('list')
  const [query, setQuery] = useState('')
  const [selectedId, setSelectedId] = useState('')
  const [label, setLabel] = useState('')
  const [allMarks, setAllMarks] = useState<SessionTreeLabelMatch[]>([])
  const [marksLoaded, setMarksLoaded] = useState(false)
  const [marksLoading, setMarksLoading] = useState(false)
  const [openingMark, setOpeningMark] = useState<SessionTreeLabelMatch | null>(null)
  // 不在当前分支上的标记：先在列表里就地确认（老问题 2），确认后才切换分支。
  const [confirmingMark, setConfirmingMark] = useState<SessionTreeLabelMatch | null>(null)
  const [summarize, setSummarize] = useState(false)
  const [loading, setLoading] = useState(false)
  const [savingLabel, setSavingLabel] = useState(false)
  const [navigating, setNavigating] = useState(false)
  const [creatingChild, setCreatingChild] = useState(false)
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
    setConfirmingMark(null)
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

  // 打开标记：在当前分支上的直接打开并定位；在另一条分支上的先就地确认，确认后切换分支、
  // 打开并提示可撤销。防重入避免重复导航。
  const openMark = async (mark: SessionTreeLabelMatch, confirmed = false) => {
    if (openingMark) return
    if (!mark.active && !confirmed) {
      setConfirmingMark(mark)
      return
    }
    setConfirmingMark(null)
    setOpeningMark(mark)
    setError('')
    try {
      let navigation = null
      if (!mark.active) {
        navigation = await chatApi.navigateSessionTreeTarget(mark.sessionId, mark.entryId)
        if (navigation.cancelled) {
          setOpeningMark(null)
          return
        }
        void invalidateSessionTreeSummary(mark.sessionId)
      }
      requestSessionSelection(mark.sessionId, 'open', mark.entryId)
      onClose()
      if (navigation && notify) {
        notifyTreeNavigation(mark.sessionId, navigation, t('chat:sessionTree.switchedBranch'), {
          notify,
          t,
        })
      }
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
      if (!nodeInView(node, view, showAllEntries)) return false
      if (!needle) return true
      return `${node.label} ${node.text} ${typeLabel(node)}`
        .toLocaleLowerCase(language)
        .includes(needle)
    })
  }, [annotatedRoots, language, query, showAllEntries, typeLabel, view])
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
      const next = await chatApi.setSessionTreeLabel(sessionId, selected.id, label)
      setData(next)
      // 转录里的回合标记与顶栏读同一份摘要，保存后立刻显示（老问题 1）。
      applySessionTreeResponse(sessionId, next)
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
      const next = await chatApi.setSessionTreeLabel(sessionId, selected.id, '')
      setData(next)
      applySessionTreeResponse(sessionId, next)
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

  // 在谱系中打开父会话或其它独立对话。父会话只定位到派生点那一回合，不导航它的树：
  // 父会话派生后可能继续聊过，导航到派生点会把它的当前分支退回去（老问题 4，已复现）。
  const openRelatedSession = (target: SessionSummary, targetEntryId = '') => {
    if (!target.id) return
    requestSessionSelection(target.id, 'open', targetEntryId)
    onClose()
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
        // 分支数立刻更新（老问题 6），提示里可以撤销（老问题 3）。
        applySessionTreeResponse(sessionId, next)
        await onNavigated(next.editorText)
        onClose()
        if (notify) {
          notifyTreeNavigation(sessionId, next, t('chat:chatPage.branchedFromNode'), { notify, t })
        }
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
                <GitFork size={16} />
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
                setConfirmingMark(null)
                setMobilePane('list')
              }}
              showAllEntries={showAllEntries}
              onShowAllEntriesChange={setShowAllEntries}
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
              confirmingMark={confirmingMark}
              onConfirmMark={(mark) => void openMark(mark, true)}
              onCancelMark={() => setConfirmingMark(null)}
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
                openRelatedSession(target, targetEntryId ?? '')
              }
            />
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
