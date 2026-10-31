// 星忆页（功能 › 会话与资料）：长期记忆的星域、星辰与待确认的草稿。
// 用功能页模板（规格 v2 第 10 节 A6 第 2 批，星忆保留星图）：“‹ 功能”· 标题 · 工具行（搜索、点亮星辰；星图 / 列表）·
// 星域分区（选中的星域行内有重命名 / 删除）· 当前星域的星辰（星图视图：深空舞台 + 类型图例 + 选中星辰的详情；
// 列表视图：每颗星辰一行，展开是同一份详情，可以看到星图放不下的第 25 颗以后）· 星忆草稿（自动确认阈值与逐条确认 / 忽略）。
import { useCallback, useEffect, useRef, useState } from 'react'
import type { FormEvent } from 'react'
import { Check, FileCode2, Info, Pencil, Plus, RefreshCw, Sparkles, Trash2, X } from 'lucide-react'
import { FEATURE_SEARCH_INPUT_ID } from '@/app/more-pages'
import type { I18nValues } from '@/app/i18n'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import {
  FeatureAlert,
  FeatureList,
  FeaturePage,
  FeatureRow,
  FeatureSearch,
  FeatureSection,
  FeatureToolbar,
} from '@/components/app/FeaturePage'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { AppCardHeader, AppError, SegmentedTabs as Segmented } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { apiJson } from '@/lib/api'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { GalaxyDot, MemoryGalaxy } from './MemoryGalaxy'
import { MEMORY_TYPES, type MemoryLink, type MemoryNode, type MemoryType } from './memory-types'

type Translate = (message: string, values?: I18nValues) => string

type MemorySpace = {
  id: string
  name: string
  kind: 'global' | 'custom' | string
  nodeCount: number
}
type MemoryCandidate = {
  id: string
  spaceId: string
  title: string
  content: string
  evidence?: string
  sourceType?: string
  confidence?: number
  topicKey?: string
  createdAt?: string
  expiresAt?: string
}
type MemoryData = {
  spaces: MemorySpace[]
  nodes: MemoryNode[]
  links: MemoryLink[]
  candidates: MemoryCandidate[]
  selectedSpaceId: string
}
type MemoryNodeModalState = Partial<MemoryNode> & { spaceId: string }
type MemoryView = 'galaxy' | 'list'
type MemoryPageProps = {
  notify: Notify
  // 从“功能”进入时由应用壳提供，渲染标题上方的“‹ 功能”。
  onBack?: () => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}
type MemoryNodeModalProps = {
  spaces: MemorySpace[]
  node: MemoryNode | null
  initialSpaceId: string
  onClose: () => void
  onSaved: (message: string) => Promise<void>
}
type MemorySpaceModalProps = {
  space: MemorySpace | null
  onClose: () => void
  onSaved: (space: MemorySpace, message: string) => Promise<void>
}

// 星忆弹窗沿用全局 .modal 浮层：纯色遮罩（无背景模糊）、浮层底色 + 环线投影。
const MODAL_BACKDROP =
  'modal-backdrop fixed inset-0 z-[70] grid place-items-center overflow-y-auto overscroll-contain bg-[var(--modal-overlay)] p-5 max-[650px]:p-2'
const MODAL_SURFACE =
  'modal grid max-h-[calc(100dvh_-_40px)] w-[min(430px,100%)] gap-0.5 overflow-y-auto overscroll-contain rounded-dialog bg-popover p-5 shadow-float max-[650px]:max-h-[calc(100dvh_-_16px)]'
// 只读键值行：左侧弱化标签，右侧值单行省略。
const KEY_VALUE =
  'key-value flex min-h-[34px] min-w-0 items-center justify-between gap-4 border-t border-stroke-soft text-sm [&>dt]:shrink-0 [&>dt]:text-content-muted [&>dd]:min-w-0 [&>dd]:truncate [&>dd]:text-right [&>dd]:text-content'

function memoryTypeLabel(type: MemoryType, t: Translate) {
  if (type === 'file') return t('memory:memoryPage.fileType')
  if (type === 'risk') return t('memory:memoryPage.riskType')
  if (type === 'preference') return t('memory:memoryPage.preferenceType')
  if (type === 'decision') return t('memory:memoryPage.decisionType')
  if (type === 'fact') return t('memory:memoryPage.factType')
  if (type === 'task') return t('memory:memoryPage.taskType')
  return t('memory:memoryPage.conceptType')
}

function formatMemoryTime(value: string | undefined, locale = 'zh-CN') {
  if (!value) return '—'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return new Intl.DateTimeFormat(locale, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }).format(date)
}

function spaceLabel(space: MemorySpace | null | undefined, t: Translate = (value) => value) {
  return space?.kind === 'global' ? t('memory:memoryPage.globalMemorySpace') : space?.name || ''
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export function MemoryPage({ notify, onBack, requestConfirm }: MemoryPageProps) {
  const searchShortcut = useShortcutLabel('focusSearch')
  // 搜索词归页面自己（换页即清空，与原共享页头一致）；外壳的“/”按固定 id 聚焦这里的搜索框。
  const [query, setQuery] = useState('')
  const { t, language } = useI18n()
  const [data, setData] = useState<MemoryData>({
    spaces: [],
    nodes: [],
    links: [],
    candidates: [],
    selectedSpaceId: '',
  })
  const [spaceId, setSpaceId] = useState('')
  const [selectedId, setSelectedId] = useState('')
  // 列表视图里展开的星辰（与星图的选中分开：列表默认全部收起）。
  const [expandedId, setExpandedId] = useState('')
  const [view, setView] = useState<MemoryView>('galaxy')
  const detailRef = useRef<HTMLDivElement | null>(null)
  const loadRevision = useRef(0)
  const [loading, setLoading] = useState(true)
  // 至少成功读到过一次：之前的失败是“读不出来”（整页错误 + 重试），之后的失败只是一条提示、数据保留。
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState('')
  const [nodeModal, setNodeModal] = useState<MemoryNodeModalState | null>(null)
  const [spaceModal, setSpaceModal] = useState<Partial<MemorySpace> | null>(null)
  const [resolvingCandidateId, setResolvingCandidateId] = useState('')
  const [autoApproveConfidence, setAutoApproveConfidence] = useState<string | null>(null)
  const [thresholdError, setThresholdError] = useState('')
  const [thresholdErrorAction, setThresholdErrorAction] = useState<'load' | 'save'>('load')
  const [loadingThreshold, setLoadingThreshold] = useState(true)
  const savedThreshold = useRef<number | null>(null)
  const thresholdEditRevision = useRef(0)
  const thresholdReadRevision = useRef(0)
  const thresholdSavedRevision = useRef(0)
  const thresholdRead = useRef<AbortController | null>(null)
  const thresholdWrite = useRef<AbortController | null>(null)
  const thresholdWriteUncertain = useRef(false)
  const thresholdQueuedSubmission = useRef<{ value: number; revision: number } | null>(null)
  const thresholdActive = useRef(true)
  const [savingThreshold, setSavingThreshold] = useState(false)

  // 加载记忆数据：按空间/搜索词拉取节点与候选；选中项失效时回退首节点。
  const load = useCallback(
    async (requestedSpaceId = '') => {
      const revision = ++loadRevision.current
      setLoading(true)
      setError('')
      try {
        const params = new URLSearchParams()
        if (requestedSpaceId) params.set('spaceId', requestedSpaceId)
        if (query.trim()) params.set('query', query.trim())
        const result = await apiJson<MemoryData>(`/api/memory?${params}`)
        if (revision !== loadRevision.current) return
        setData(result)
        setLoaded(true)
        setSpaceId(result.selectedSpaceId || '')
        setSelectedId((current) =>
          result.nodes.some((node) => node.id === current) ? current : result.nodes[0]?.id || '',
        )
      } catch (loadError) {
        if (revision === loadRevision.current) setError(errorMessage(loadError))
      } finally {
        if (revision === loadRevision.current) setLoading(false)
      }
    },
    [query],
  )

  useEffect(() => {
    void load(spaceId)
    return () => {
      loadRevision.current += 1
    }
  }, [load, spaceId])

  const loadAutoApproveConfidence = useCallback(async () => {
    thresholdRead.current?.abort()
    const controller = new AbortController()
    thresholdRead.current = controller
    const requestRevision = ++thresholdReadRevision.current
    const editRevision = thresholdEditRevision.current
    const draftIsSaved = editRevision === thresholdSavedRevision.current
    setLoadingThreshold(true)
    setThresholdError('')
    try {
      const preference = await apiJson<{ autoApproveConfidence: unknown }>('/api/settings/memory', {
        signal: controller.signal,
      })
      if (
        controller.signal.aborted ||
        !thresholdActive.current ||
        requestRevision !== thresholdReadRevision.current
      )
        return
      const value = preference.autoApproveConfidence
      if (typeof value !== 'number' || !Number.isInteger(value) || value < 0 || value > 100) {
        throw new Error(t('memory:memoryPage.invalidAutoApproveThreshold'))
      }
      // 读取只能初始化尚未编辑的草稿，晚到结果不能覆盖用户当前输入。
      if (draftIsSaved && thresholdEditRevision.current === editRevision) {
        savedThreshold.current = value
        setAutoApproveConfidence(String(value))
      }
    } catch (loadError) {
      if (
        !controller.signal.aborted &&
        thresholdActive.current &&
        requestRevision === thresholdReadRevision.current
      ) {
        setThresholdErrorAction('load')
        setThresholdError(errorMessage(loadError))
      }
    } finally {
      if (
        !controller.signal.aborted &&
        thresholdActive.current &&
        requestRevision === thresholdReadRevision.current
      )
        setLoadingThreshold(false)
    }
  }, [t])

  useEffect(() => {
    thresholdActive.current = true
    void loadAutoApproveConfidence()
    return () => {
      thresholdActive.current = false
      thresholdRead.current?.abort()
      thresholdWrite.current?.abort()
      thresholdQueuedSubmission.current = null
    }
  }, [loadAutoApproveConfidence])

  const saveAutoApproveConfidence = async () => {
    if (
      autoApproveConfidence === null ||
      !autoApproveConfidence.trim() ||
      savedThreshold.current === null
    )
      return
    const value = Number(autoApproveConfidence)
    if (!Number.isFinite(value)) return
    const normalized = Math.min(100, Math.max(0, Math.round(value)))
    if (
      !thresholdWrite.current &&
      !thresholdWriteUncertain.current &&
      normalized === savedThreshold.current
    ) {
      thresholdQueuedSubmission.current = null
      thresholdSavedRevision.current = thresholdEditRevision.current
      setAutoApproveConfidence(String(normalized))
      setThresholdError('')
      return
    }
    // blur/Enter 是明确的提交意图。写入期间只合并最新意图，不能把它当成普通编辑丢弃。
    thresholdQueuedSubmission.current = {
      value: normalized,
      revision: thresholdEditRevision.current,
    }
    if (thresholdWrite.current) return
    const controller = new AbortController()
    thresholdWrite.current = controller
    setSavingThreshold(true)
    setThresholdError('')
    try {
      while (
        thresholdQueuedSubmission.current &&
        thresholdActive.current &&
        !controller.signal.aborted
      ) {
        const submission = thresholdQueuedSubmission.current
        thresholdQueuedSubmission.current = null
        if (submission.value !== savedThreshold.current || thresholdWriteUncertain.current) {
          try {
            await apiJson('/api/settings/memory', {
              method: 'PATCH',
              body: { autoApproveConfidence: submission.value },
              signal: controller.signal,
            })
          } catch (saveError) {
            if (!thresholdActive.current || controller.signal.aborted) return
            // 失败时保留较新的提交意图与当前草稿，由用户重试，避免无提示地继续覆盖。
            thresholdQueuedSubmission.current ||= submission
            thresholdWriteUncertain.current = true
            setThresholdErrorAction('save')
            setThresholdError(errorMessage(saveError))
            return
          }
          if (!thresholdActive.current || controller.signal.aborted) return
          savedThreshold.current = submission.value
          thresholdWriteUncertain.current = false
        }
        thresholdSavedRevision.current = submission.revision
        if (thresholdEditRevision.current === submission.revision) {
          setAutoApproveConfidence(String(submission.value))
          if (!thresholdQueuedSubmission.current) notify(t('memory:memoryPage.autoApproveSaved'))
        }
      }
    } finally {
      if (thresholdWrite.current === controller) {
        thresholdWrite.current = null
        if (thresholdActive.current) setSavingThreshold(false)
      }
    }
  }

  const selected = data.nodes.find((node) => node.id === selectedId) || null
  // 关联星辰与关联文件：星图里按选中的星辰算，列表里按展开的那一行算。
  const relatedOf = (id: string) => {
    const related = new Set(
      data.links.flatMap((link) => {
        if (link.sourceId === id) return [link.targetId]
        if (link.targetId === id) return [link.sourceId]
        return []
      }),
    )
    return {
      count: related.size,
      files: data.nodes.filter(
        (node) => node.type === 'file' && (related.has(node.id) || node.id === id),
      ),
    }
  }
  const selectedSpace = data.spaces.find((space) => space.id === spaceId)

  // 删除记忆节点（确认后），成功后刷新。
  const deleteNode = async (node: MemoryNode) => {
    const approved = await requestConfirm({
      title: t('memory:memoryPage.deleteMemory'),
      message: t('memory:memoryPage.deleteMemoryName', { name: node.title }),
      confirmLabel: t('memory:memoryPage.delete'),
    })
    if (!approved) return
    try {
      await apiJson(`/api/memory/nodes/${encodeURIComponent(node.id)}`, { method: 'DELETE' })
      notify(t('memory:memoryPage.memoryDeleted'))
      await load(spaceId)
    } catch (deleteError) {
      setError(errorMessage(deleteError))
    }
  }

  // 处理记忆候选：接受/忽略（POST 动作）后刷新；防重入。
  const resolveCandidate = async (candidate: MemoryCandidate, action: 'accept' | 'reject') => {
    if (resolvingCandidateId) return
    setResolvingCandidateId(candidate.id)
    setError('')
    try {
      await apiJson(`/api/memory/candidates/${encodeURIComponent(candidate.id)}/${action}`, {
        method: 'POST',
        body: '{}',
      })
      notify(
        action === 'accept'
          ? t('memory:memoryPage.memoryCandidateConfirmed')
          : t('memory:memoryPage.memoryCandidateIgnored'),
      )
      await load(spaceId)
    } catch (candidateError) {
      setError(errorMessage(candidateError))
    } finally {
      setResolvingCandidateId('')
    }
  }

  // 忽略全部候选（确认后批量处理）。
  const ignoreAllCandidates = async () => {
    if (resolvingCandidateId || !data.candidates?.length) return
    setResolvingCandidateId('__all__')
    setError('')
    try {
      const result = await apiJson<{ rejected: number }>('/api/memory/candidates/reject-all', {
        method: 'POST',
        body: '{}',
      })
      notify(t('memory:memoryPage.ignoredCountMemoryDrafts', { count: result.rejected || 0 }))
      await load(spaceId)
    } catch (candidateError) {
      setError(errorMessage(candidateError))
    } finally {
      setResolvingCandidateId('')
    }
  }

  const deleteSpace = async () => {
    if (!selectedSpace) return
    const approved = await requestConfirm({
      title: t('memory:memoryPage.deleteMemorySpace'),
      message: t('memory:memoryPage.deleteMemorySpaceNameAndAllMemoriesInIt', {
        name: spaceLabel(selectedSpace, t),
      }),
      confirmLabel: t('memory:memoryPage.delete'),
    })
    if (!approved) return
    try {
      await apiJson(`/api/memory/spaces/${encodeURIComponent(selectedSpace.id)}`, {
        method: 'DELETE',
      })
      setSpaceId('')
      setSelectedId('')
      notify(t('memory:memoryPage.memorySpaceDeleted'))
    } catch (deleteError) {
      setError(errorMessage(deleteError))
    }
  }
  const title = t('navigation:navigation.memory')
  const spaceName = spaceLabel(selectedSpace, t)
  // 星图里点了一颗星：详情在舞台下方，滚到能看见（不抢焦点，键盘仍停在那颗星上）。
  const selectFromGalaxy = (id: string) => {
    setSelectedId(id)
    requestAnimationFrame(() => detailRef.current?.scrollIntoView({ block: 'nearest' }))
  }
  // 选中 / 展开星辰的详情：类型、来源、可信度、证据、时间、关联星辰，编辑与删除，下面是关联文件。
  // 星图视图里带标题（星辰名与内容），列表视图里行本身已经写了名称与内容。
  const renderDetail = (node: MemoryNode, withHeading: boolean) => {
    const related = relatedOf(node.id)
    return (
      <div className="flex min-w-0 flex-col gap-3">
        {withHeading && (
          <header className="flex min-w-0 flex-col gap-1">
            <h3 className="text-[15px] leading-6 font-medium break-words text-content">
              {node.title}
            </h3>
            <p className="text-[13px] leading-normal break-words text-content-secondary">
              {node.content}
            </p>
          </header>
        )}
        <dl>
          <div className={KEY_VALUE}>
            <dt>{t('memory:memoryPage.memoryType')}</dt>
            <dd className="inline-flex items-center justify-end gap-2">
              <GalaxyDot type={node.type} />
              {memoryTypeLabel(node.type, t)}
            </dd>
          </div>
          <div className={KEY_VALUE}>
            <dt>{t('memory:memoryPage.source')}</dt>
            <dd>
              {node.sourceType === 'conversation_confirmed'
                ? t('memory:memoryPage.userConfirmedConversationCandidate')
                : node.sourceType === 'user_confirmed'
                  ? t('memory:memoryPage.explicitUserRequest')
                  : node.sourceType === 'agent'
                    ? t('memory:memoryPage.createdByAgent')
                    : t('memory:memoryPage.addedManually')}
            </dd>
          </div>
          <div className={KEY_VALUE}>
            <dt>{t('memory:memoryPage.authority')}</dt>
            <dd className="tabular-nums">{node.authority ?? 0}/100</dd>
          </div>
          {node.evidence && (
            <div className={KEY_VALUE}>
              <dt>{t('memory:memoryPage.evidence')}</dt>
              <dd title={node.evidence}>{node.evidence}</dd>
            </div>
          )}
          <div className={KEY_VALUE}>
            <dt>{t('memory:memoryPage.created')}</dt>
            <dd className="tabular-nums">{formatMemoryTime(node.createdAt, language)}</dd>
          </div>
          <div className={KEY_VALUE}>
            <dt>{t('memory:memoryPage.relatedMemories')}</dt>
            <dd className="tabular-nums">
              {t('memory:memoryPage.count', { count: related.count })}
            </dd>
          </div>
        </dl>
        <div className="flex flex-wrap justify-end gap-2">
          <Button
            variant="ghost"
            size="sm"
            className="text-danger hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
            onClick={() => void deleteNode(node)}
          >
            <Trash2 />
            {t('memory:memoryPage.delete')}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            className="max-[650px]:h-11"
            onClick={() => setNodeModal(node)}
          >
            <Pencil />
            {t('memory:memoryPage.edit')}
          </Button>
        </div>
        <section className="min-w-0" aria-label={t('memory:memoryPage.relatedFiles')}>
          <h4 className="mb-1 text-xs font-medium text-content-muted tabular-nums">
            {t('memory:memoryPage.relatedFiles')} · {related.files.length}
          </h4>
          {related.files.length ? (
            <ul className="divide-y divide-stroke-soft border-t border-stroke-soft">
              {related.files.map((file) => (
                <li
                  className="file-row grid min-h-11 min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 py-1"
                  key={file.id}
                >
                  <FileCode2 className="size-4 text-content-muted" aria-hidden="true" />
                  <span className="flex min-w-0 flex-col gap-0.5">
                    <strong className="truncate text-sm font-medium text-content">
                      {file.title}
                    </strong>
                    <small className="truncate font-mono text-xs text-content-muted">
                      {file.sourcePath || file.cwd || t('memory:memoryPage.localMemory')}
                    </small>
                  </span>
                  <span className="flex items-center">
                    {file.id !== node.id && (
                      <Button
                        variant="ghost"
                        size="sm"
                        className="max-[650px]:h-11"
                        onClick={() =>
                          view === 'galaxy' ? setSelectedId(file.id) : setExpandedId(file.id)
                        }
                      >
                        {t('memory:memoryPage.view')}
                      </Button>
                    )}
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="max-[650px]:size-11"
                      aria-label={`${t('memory:memoryPage.edit')}: ${file.title}`}
                      onClick={() => setNodeModal(file)}
                    >
                      <Pencil />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="text-content-muted hover:bg-danger-soft hover:text-danger max-[650px]:size-11"
                      aria-label={`${t('memory:memoryPage.delete')}: ${file.title}`}
                      onClick={() => void deleteNode(file)}
                    >
                      <Trash2 />
                    </Button>
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-xs text-content-muted">
              {t('memory:memoryPage.thisMemoryHasNoRelatedFiles')}
            </p>
          )}
        </section>
      </div>
    )
  }

  return (
    <FeaturePage
      title={title}
      description={t('memory:memoryPage.lede')}
      back={
        onBack && {
          label: t('navigation:sidebar.features'),
          ariaLabel: t('navigation:shell.backToFeaturesLabel'),
          onBack,
        }
      }
      toolbar={
        <FeatureToolbar
          search={
            <FeatureSearch
              id={FEATURE_SEARCH_INPUT_ID}
              value={query}
              onValueChange={setQuery}
              placeholder={t('navigation:pageHeader.searchMemoriesOrFiles')}
              shortcut={searchShortcut}
              clearLabel={t('navigation:more.clearSearch')}
            />
          }
          actions={
            // 主操作（原共享页头的“点亮星辰”）：在当前星域里新建一颗星辰。
            <Button
              type="button"
              className="h-9"
              disabled={!loaded}
              onClick={() => setNodeModal({ spaceId: spaceId || data.selectedSpaceId })}
            >
              <Plus />
              {t('memory:memoryPage.addMemory')}
            </Button>
          }
          filters={
            // 星图保留为页内的一个视图（规格 2.7）；列表能看到星图放不下的星辰。
            <Segmented
              options={[t('memory:memoryPage.galaxyView'), t('memory:memoryPage.listView')]}
              value={
                view === 'galaxy'
                  ? t('memory:memoryPage.galaxyView')
                  : t('memory:memoryPage.listView')
              }
              onChange={(label) =>
                setView(label === t('memory:memoryPage.listView') ? 'list' : 'galaxy')
              }
            />
          }
        />
      }
    >
      {!loaded && loading ? (
        <PageState kind="loading" title={t('common:pageTemplate.loading')} />
      ) : !loaded ? (
        // 一次都没读到：写明错误并给重试，不显示成空星域。
        <PageState
          kind="error"
          title={t('memory:memoryPage.loadFailed')}
          description={error}
          action={
            <Button
              variant="secondary"
              className="max-[650px]:h-11"
              onClick={() => void load(spaceId)}
            >
              {t('common:pageTemplate.retry')}
            </Button>
          }
        />
      ) : (
        <>
          {/* 读到之后的失败（刷新、删除、处理草稿）：写在最上面，数据保留。 */}
          {error && <FeatureAlert>{error}</FeatureAlert>}
          <FeatureSection
            title={t('memory:memoryPage.memorySpaces')}
            count={data.spaces.length}
            actions={
              <Button variant="ghost" size="sm" onClick={() => setSpaceModal({})}>
                <Plus />
                {t('memory:memoryPage.newMemorySpace')}
              </Button>
            }
          >
            <FeatureList>
              {data.spaces.map((space) => {
                const active = space.id === spaceId
                return (
                  <FeatureRow
                    key={space.id}
                    data-memory-space={space.id}
                    aria-current={active || undefined}
                    // 选中的星域：名称前 3px 石墨刻度（不用强调色，规格 6.1）。
                    className={cn(
                      'relative',
                      active &&
                        'after:absolute after:top-3.5 after:bottom-3.5 after:-left-3 after:w-[3px] after:rounded-r-[3px] after:bg-star max-[650px]:after:-left-2.5',
                    )}
                    icon={<Sparkles />}
                    onOpen={() => {
                      setSpaceId(space.id)
                      setSelectedId('')
                      setExpandedId('')
                    }}
                    name={spaceLabel(space, t)}
                    status={t('memory:memoryPage.countMemories', { count: space.nodeCount })}
                    actions={
                      active && (
                        <div className="memory-space-actions flex items-center gap-1">
                          <Button variant="ghost" size="sm" onClick={() => setSpaceModal(space)}>
                            <Pencil />
                            {t('memory:memoryPage.rename')}
                          </Button>
                          {space.kind !== 'global' && (
                            <Button
                              variant="ghost"
                              size="sm"
                              className="text-danger hover:bg-danger-soft hover:text-danger"
                              onClick={() => void deleteSpace()}
                            >
                              <Trash2 />
                              {t('memory:memoryPage.delete')}
                            </Button>
                          )}
                        </div>
                      )
                    }
                  />
                )
              })}
            </FeatureList>
          </FeatureSection>
          <FeatureSection
            data-memory-stars
            title={spaceName || t('memory:memoryPage.memorySpaces')}
            count={data.nodes.length}
            actions={
              <Button
                variant="ghost"
                size="icon-sm"
                className="text-content-muted max-[650px]:size-11 pointer-coarse:size-11"
                loading={loading}
                disabled={loading}
                aria-label={t('memory:memoryPage.refresh')}
                title={t('memory:memoryPage.refresh')}
                onClick={() => void load(spaceId)}
              >
                <RefreshCw />
              </Button>
            }
          >
            {view === 'galaxy' ? (
              <div className="flex min-w-0 flex-col gap-3">
                <MemoryGalaxy
                  nodes={data.nodes}
                  links={data.links}
                  spaceId={spaceId}
                  selectedId={selectedId}
                  loading={loading}
                  onSelect={selectFromGalaxy}
                  onAdd={() => setNodeModal({ spaceId })}
                />
                <div
                  role="group"
                  aria-label={t('memory:memoryPage.memoryMapTypes')}
                  className="galaxy-legend flex flex-wrap gap-x-4 gap-y-1.5"
                >
                  {MEMORY_TYPES.map((type) => (
                    <span
                      className="flex items-center gap-1.5 text-xs text-content-secondary"
                      key={type}
                    >
                      <GalaxyDot type={type} />
                      {memoryTypeLabel(type, t)}
                    </span>
                  ))}
                </div>
                <div
                  ref={detailRef}
                  role="region"
                  data-memory-detail
                  aria-label={t('memory:memoryPage.selectedMemory')}
                  className="min-w-0 scroll-mb-4 rounded-[var(--r-md)] border border-stroke px-4 py-3"
                >
                  {selected ? (
                    renderDetail(selected, true)
                  ) : (
                    <p className="text-xs leading-normal text-content-muted">
                      {t('memory:memoryPage.selectAMemoryInTheMapToViewItsDetails')}
                    </p>
                  )}
                </div>
              </div>
            ) : data.nodes.length ? (
              <FeatureList>
                {data.nodes.map((node) => (
                  <FeatureRow
                    key={node.id}
                    data-memory-node={node.id}
                    icon={<GalaxyDot type={node.type} />}
                    name={node.title}
                    nameMeta={memoryTypeLabel(node.type, t)}
                    description={node.content}
                    expanded={expandedId === node.id}
                    onExpandedChange={(open) => setExpandedId(open ? node.id : '')}
                    toggleLabel={node.title}
                    detail={renderDetail(node, false)}
                  />
                ))}
              </FeatureList>
            ) : (
              <InlineEmptyState
                title={
                  query.trim()
                    ? t('memory:memoryPage.noMatchingMemories')
                    : t('memory:memoryPage.noMemoriesInSpace')
                }
              />
            )}
          </FeatureSection>
          <FeatureSection
            title={t('memory:memoryPage.memoryDrafts')}
            count={data.candidates?.length || 0}
            actions={
              Boolean(data.candidates?.length) && (
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={Boolean(resolvingCandidateId)}
                  onClick={() => void ignoreAllCandidates()}
                >
                  <X />
                  {t('memory:memoryPage.ignoreAll')}
                </Button>
              )
            }
          >
            {loadingThreshold && (
              <p role="status" className="flex items-center gap-2 py-2 text-xs text-content-muted">
                <OrbitSpinner className="size-3.5" />
                {t('memory:memoryPage.loadingAutoApproveThreshold')}
              </p>
            )}
            {thresholdError && (
              <div className="flex flex-wrap items-center gap-2 py-2">
                <AppError className="my-0">
                  {t('memory:memoryPage.autoApproveThresholdUnavailable')} {thresholdError}
                </AppError>
                <Button
                  variant="secondary"
                  size="sm"
                  className="max-[650px]:h-11"
                  disabled={loadingThreshold || savingThreshold}
                  onClick={() =>
                    void (thresholdErrorAction === 'save'
                      ? saveAutoApproveConfidence()
                      : loadAutoApproveConfidence())
                  }
                >
                  {t('memory:memoryPage.retryAutoApproveThreshold')}
                </Button>
              </div>
            )}
            {autoApproveConfidence !== null && (
              <div className="memory-auto-approve-row flex min-h-12 items-center justify-between gap-3 border-t border-stroke py-1.5">
                <div className="flex min-w-0 items-center gap-1.5 text-sm font-medium text-content">
                  <span>{t('memory:memoryPage.autoApproveThreshold')}</span>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Info className="size-3.5 shrink-0 text-content-muted" />
                    </TooltipTrigger>
                    <TooltipContent align="center" side="top">
                      {t('memory:memoryPage.autoApproveThresholdHint')}
                    </TooltipContent>
                  </Tooltip>
                </div>
                <span className="memory-auto-approve-input flex flex-none items-center gap-1.5 text-xs text-content-muted">
                  <Input
                    type="number"
                    min={0}
                    max={100}
                    step={5}
                    className="h-7 w-16 text-right tabular-nums max-[650px]:h-11"
                    value={autoApproveConfidence}
                    aria-busy={savingThreshold}
                    aria-label={t('memory:memoryPage.autoApproveThreshold')}
                    onChange={(event) => {
                      thresholdEditRevision.current += 1
                      setAutoApproveConfidence(event.target.value)
                    }}
                    onBlur={() => void saveAutoApproveConfidence()}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter') {
                        event.preventDefault()
                        void saveAutoApproveConfidence()
                      }
                    }}
                  />
                  %{savingThreshold && <OrbitSpinner className="size-3.5" />}
                </span>
              </div>
            )}
            {data.candidates?.length ? (
              <FeatureList>
                {data.candidates.map((candidate) => (
                  <FeatureRow
                    key={candidate.id}
                    className="memory-candidate"
                    name={candidate.title}
                    description={candidate.content}
                    meta={
                      <span className="truncate tabular-nums">
                        {spaceLabel(
                          data.spaces.find((space) => space.id === candidate.spaceId),
                          t,
                        )}
                        {' · '}
                        {candidate.sourceType === 'agent'
                          ? t('memory:memoryPage.agentProposed')
                          : t('memory:memoryPage.conversationExtracted')}
                        {' · '}
                        {t('memory:memoryPage.confidencePercent', {
                          value: Math.round((candidate.confidence || 0) * 100),
                        })}
                      </span>
                    }
                    actions={
                      <>
                        <Button
                          variant="secondary"
                          size="sm"
                          disabled={Boolean(resolvingCandidateId)}
                          onClick={() => void resolveCandidate(candidate, 'accept')}
                        >
                          <Check />
                          {t('memory:memoryPage.confirm')}
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          disabled={Boolean(resolvingCandidateId)}
                          onClick={() => void resolveCandidate(candidate, 'reject')}
                        >
                          <X />
                          {t('memory:memoryPage.ignore')}
                        </Button>
                      </>
                    }
                    detail={
                      <div className="flex flex-col gap-0.5 text-xs leading-[1.5] text-content-muted">
                        {candidate.topicKey && (
                          <span>
                            {t('memory:memoryPage.topic')}：
                            <code className="font-mono">{candidate.topicKey}</code>
                          </span>
                        )}
                        {candidate.evidence && (
                          <span className="[overflow-wrap:anywhere]">
                            {t('memory:memoryPage.evidence')}：{candidate.evidence}
                          </span>
                        )}
                        <span className="tabular-nums">
                          {t('memory:memoryPage.created')}：
                          {formatMemoryTime(candidate.createdAt, language)}
                          {candidate.expiresAt
                            ? ` · ${t('memory:memoryPage.expires')}：${formatMemoryTime(candidate.expiresAt, language)}`
                            : ''}
                        </span>
                      </div>
                    }
                  />
                ))}
              </FeatureList>
            ) : (
              <p className="border-t border-stroke py-3 text-xs text-content-muted">
                {t('memory:memoryPage.thereAreNoPendingMemoryCandidates')}
              </p>
            )}
          </FeatureSection>
        </>
      )}
      {nodeModal && (
        <MemoryNodeModal
          spaces={data.spaces}
          node={nodeModal.id ? (nodeModal as MemoryNode) : null}
          initialSpaceId={nodeModal.spaceId || spaceId}
          onClose={() => setNodeModal(null)}
          onSaved={async (message) => {
            setNodeModal(null)
            notify(message)
            await load(spaceId)
          }}
        />
      )}
      {spaceModal && (
        <MemorySpaceModal
          space={spaceModal.id ? (spaceModal as MemorySpace) : null}
          onClose={() => setSpaceModal(null)}
          onSaved={async (space, message) => {
            setSpaceModal(null)
            setSpaceId(space.id)
            notify(message)
          }}
        />
      )}
    </FeaturePage>
  )
}

function MemoryNodeModal({ spaces, node, initialSpaceId, onClose, onSaved }: MemoryNodeModalProps) {
  const { t } = useI18n()
  const [draft, setDraft] = useState({
    spaceId: node?.spaceId || initialSpaceId || spaces[0]?.id || '',
    title: node?.title || '',
    content: node?.content || '',
    type: node?.type || 'concept',
    sourcePath: node?.sourcePath || '',
    importance: node?.importance ?? 0.5,
  })
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (saving) return
    setSaving(true)
    setError('')
    try {
      await apiJson(
        node ? `/api/memory/nodes/${encodeURIComponent(node.id)}` : '/api/memory/nodes',
        {
          method: node ? 'PATCH' : 'POST',
          body: JSON.stringify(draft),
        },
      )
      await onSaved(
        node ? t('memory:memoryPage.memoryUpdated') : t('memory:memoryPage.memoryCreated'),
      )
    } catch (saveError) {
      setError(errorMessage(saveError))
    } finally {
      setSaving(false)
    }
  }
  return (
    <div
      className={MODAL_BACKDROP}
      onMouseDown={(event) => !saving && event.target === event.currentTarget && onClose()}
    >
      <form
        role="dialog"
        aria-modal="true"
        aria-label={node ? t('memory:memoryPage.editMemory') : t('memory:memoryPage.addMemory')}
        className={MODAL_SURFACE}
        onSubmit={submit}
      >
        <AppCardHeader className="[&_h2]:text-[15px] [&_h2]:font-medium">
          <div>
            <h2>{node ? t('memory:memoryPage.editMemory') : t('memory:memoryPage.addMemory')}</h2>
            <p>{t('memory:memoryPage.lightUpIdeasWorthKeepingAsMemoriesYouCanReturnToLater')}</p>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="-mt-1 -mr-1 max-[650px]:size-11"
            aria-label={t('memory:memoryPage.closeDialog')}
            disabled={saving}
            onClick={onClose}
          >
            <X />
          </Button>
        </AppCardHeader>
        <FieldLabel variant="control">
          {t('memory:memoryPage.memorySpace')}
          <AppSelect
            value={draft.spaceId}
            onChange={(event) => setDraft({ ...draft, spaceId: event.target.value })}
          >
            {spaces.map((space) => (
              <option value={space.id} key={space.id}>
                {spaceLabel(space, t)}
              </option>
            ))}
          </AppSelect>
        </FieldLabel>
        <FieldLabel variant="control">
          {t('memory:memoryPage.memoryTitle')}
          <input
            value={draft.title}
            onChange={(event) => setDraft({ ...draft, title: event.target.value })}
            placeholder={t('memory:memoryPage.forExampleProjectUIConstraints')}
          />
        </FieldLabel>
        <FieldLabel variant="control">
          {t('memory:memoryPage.memoryContent')}
          <textarea
            value={draft.content}
            onChange={(event) => setDraft({ ...draft, content: event.target.value })}
            placeholder={t('memory:memoryPage.recordStandaloneReusableMemoryForFutureChats')}
          />
        </FieldLabel>
        <div className="form-grid grid gap-x-2.5 sm:grid-cols-2">
          <FieldLabel variant="control">
            {t('memory:memoryPage.memoryType')}
            <AppSelect
              value={draft.type}
              onChange={(event) => setDraft({ ...draft, type: event.target.value as MemoryType })}
            >
              {MEMORY_TYPES.map((value) => (
                <option value={value} key={value}>
                  {memoryTypeLabel(value, t)}
                </option>
              ))}
            </AppSelect>
          </FieldLabel>
          <FieldLabel variant="control">
            {t('memory:memoryPage.importance')}
            <AppSelect
              value={draft.importance}
              onChange={(event) => setDraft({ ...draft, importance: Number(event.target.value) })}
            >
              <option value="0.3">{t('memory:memoryPage.normal')}</option>
              <option value="0.5">{t('memory:memoryPage.common')}</option>
              <option value="0.8">{t('memory:memoryPage.important')}</option>
              <option value="1">{t('memory:memoryPage.strict')}</option>
            </AppSelect>
          </FieldLabel>
        </div>
        <FieldLabel variant="control">
          {t('memory:memoryPage.relatedFilePath')}
          <input
            value={draft.sourcePath}
            onChange={(event) => setDraft({ ...draft, sourcePath: event.target.value })}
            placeholder={t('memory:memoryPage.optionalForExampleECodeProjectREADMEMd')}
          />
        </FieldLabel>
        {error && <AppError>{error}</AppError>}
        <div className="mt-4 flex justify-end gap-2">
          <Button type="button" variant="secondary" size="dialogAction" onClick={onClose}>
            {t('memory:memoryPage.cancel')}
          </Button>
          <Button
            type="submit"
            size="dialogAction"
            disabled={saving || !draft.spaceId || !draft.title.trim() || !draft.content.trim()}
          >
            {saving ? <OrbitSpinner className="text-current" /> : <Pencil />}
            {saving ? t('memory:memoryPage.saving') : t('memory:memoryPage.save')}
          </Button>
        </div>
      </form>
    </div>
  )
}

function MemorySpaceModal({ space, onClose, onSaved }: MemorySpaceModalProps) {
  const { t } = useI18n()
  const [name, setName] = useState(space?.name || '')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (saving) return
    setSaving(true)
    setError('')
    try {
      const result = await apiJson<MemorySpace>(
        space ? `/api/memory/spaces/${encodeURIComponent(space.id)}` : '/api/memory/spaces',
        {
          method: space ? 'PATCH' : 'POST',
          body: JSON.stringify({ name, kind: 'custom' }),
        },
      )
      await onSaved(
        result,
        space
          ? t('memory:memoryPage.memorySpaceRenamed')
          : t('memory:memoryPage.memorySpaceCreated'),
      )
    } catch (saveError) {
      setError(errorMessage(saveError))
    } finally {
      setSaving(false)
    }
  }
  return (
    <div
      className={MODAL_BACKDROP}
      onMouseDown={(event) => !saving && event.target === event.currentTarget && onClose()}
    >
      <form
        role="dialog"
        aria-modal="true"
        aria-label={
          space ? t('memory:memoryPage.renameMemorySpace') : t('memory:memoryPage.newMemorySpace')
        }
        className={MODAL_SURFACE}
        onSubmit={submit}
      >
        <AppCardHeader className="[&_h2]:text-[15px] [&_h2]:font-medium">
          <div>
            <h2>
              {space
                ? t('memory:memoryPage.renameMemorySpace')
                : t('memory:memoryPage.newMemorySpace')}
            </h2>
            <p>
              {t(
                'memory:memoryPage.giveEachThemeOrProjectItsOwnMemorySpaceSoDurableMemoriesHaveSomewhereToBelong',
              )}
            </p>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="-mt-1 -mr-1 max-[650px]:size-11"
            aria-label={t('memory:memoryPage.closeDialog')}
            disabled={saving}
            onClick={onClose}
          >
            <X />
          </Button>
        </AppCardHeader>
        <FieldLabel variant="control">
          {t('memory:memoryPage.memorySpaceName')}
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={t('memory:memoryPage.forExampleProductDesignGuidelines')}
            autoFocus
          />
        </FieldLabel>
        {error && <AppError>{error}</AppError>}
        <div className="mt-4 flex justify-end gap-2">
          <Button type="button" variant="secondary" size="dialogAction" onClick={onClose}>
            {t('memory:memoryPage.cancel')}
          </Button>
          <Button type="submit" size="dialogAction" disabled={saving || !name.trim()}>
            {saving ? <OrbitSpinner className="text-current" /> : <Plus />}
            {saving ? t('memory:memoryPage.saving') : t('memory:memoryPage.save')}
          </Button>
        </div>
      </form>
    </div>
  )
}
