import { PageFrame } from '@/components/app/PageFrame'
import { ListDetailLayout } from '@/components/app/ListDetailLayout'
// 记忆页：查看/搜索/删除 Agent 会话记忆条目，按时间线浏览。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Check,
  ChevronRight,
  FileCode2,
  Info,
  Pencil,
  Plus,
  RefreshCw,
  Trash2,
  X,
  ZoomIn,
  ZoomOut,
} from 'lucide-react'
import { AppCard as Panel, AppCardHeader, AppError } from '@/components/ui/app-primitives'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { StarOrbit } from '@/components/StarOrbit'
import { apiJson } from '@/lib/api'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { useAnimationVisibility } from '@/hooks/use-animation-visibility'
import type { CSSProperties, FormEvent, PointerEvent as ReactPointerEvent, ReactNode } from 'react'
import type { Notify } from '@/app/route-context'
import type { I18nValues } from '@/app/i18n'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'

type Translate = (message: string, values?: I18nValues) => string
type MemoryType = 'concept' | 'file' | 'risk' | 'preference' | 'decision' | 'fact' | 'task'
type MemorySpace = {
  id: string
  name: string
  kind: 'global' | 'custom' | string
  nodeCount: number
}
type MemoryNode = {
  id: string
  spaceId: string
  title: string
  content: string
  type: MemoryType
  sourceType: string
  sourcePath?: string
  cwd?: string
  evidence?: string
  importance?: number
  authority?: number
  createdAt?: string
}
type MemoryLink = { id: string; sourceId: string; targetId: string }
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
type GalaxyStar = {
  node: MemoryNode
  x: number
  y: number
  twinkle: number
}
type GalaxyPoint = Pick<GalaxyStar, 'x' | 'y'>
type MemoryNodeModalState = Partial<MemoryNode> & { spaceId: string }
type MemoryPageProps = {
  notify: Notify
  query?: string
  registerPrimaryAction: (action: () => void) => () => void
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
type GalaxyStarStyle = CSSProperties & {
  '--star-size': string
  '--twinkle-delay': string
  '--enter-delay': string
  '--depth': number
  '--g-star-color': string
  '--g-star-glow': string
}

const GALAXY_VIEW = { width: 600, height: 420, cx: 300, cy: 206 }
const MAX_STARS = 24
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5))

const MEMORY_TYPES: MemoryType[] = [
  'concept',
  'file',
  'risk',
  'preference',
  'decision',
  'fact',
  'task',
]

// 卡片：1px 描边 + r-md，无投影；卡片头位于次级表面，13/500 标题 + 等宽计数。
const CARD = 'min-w-0 overflow-hidden rounded-md border border-stroke bg-panel'
const CARD_HEAD =
  'flex min-h-11 min-w-0 items-center justify-between gap-3 border-b border-stroke-soft bg-surface-subtle px-3.5 py-1.5'
const SELECTED_TICK = 'absolute inset-y-2 left-0 w-[3px] rounded-r-[3px] bg-star'
// 只读键值行：左侧弱化标签，右侧值单行省略。
const KEY_VALUE =
  'key-value flex min-h-[34px] min-w-0 items-center justify-between gap-4 border-t border-stroke-soft text-sm [&>dt]:shrink-0 [&>dt]:text-content-muted [&>dd]:min-w-0 [&>dd]:truncate [&>dd]:text-right [&>dd]:text-content'
// 星忆弹窗沿用全局 .modal 浮层：纯色遮罩（无背景模糊）、浮层底色 + 环线投影。
const MODAL_BACKDROP =
  'modal-backdrop fixed inset-0 z-[70] grid place-items-center overflow-y-auto overscroll-contain bg-[var(--modal-overlay)] p-5 max-[650px]:p-2'
const MODAL_SURFACE =
  'modal grid max-h-[calc(100dvh_-_40px)] w-[min(430px,100%)] gap-0.5 overflow-y-auto overscroll-contain rounded-dialog bg-popover p-5 shadow-float max-[650px]:max-h-[calc(100dvh_-_16px)]'

function CardTitle({ children, count }: { children: ReactNode; count?: ReactNode }) {
  return (
    <h2 className="flex min-w-0 items-baseline gap-2 text-sm font-medium text-content">
      {children}
      {count !== undefined && (
        <span className="font-normal text-content-muted tabular-nums">{count}</span>
      )}
    </h2>
  )
}

// 星图类型色点：沿用星系配色 token，外加 1px 细环保证浅色底上的浅色类型可见。
function GalaxyDot({ type }: { type: MemoryType }) {
  return (
    <i
      aria-hidden="true"
      className={`g-dot g-${type} inline-block size-2 shrink-0 rounded-full ring-1 ring-stroke`}
      style={{ background: STAR_COLORS[type] }}
    />
  )
}

function memoryTypeLabel(type: MemoryType, t: Translate) {
  if (type === 'file') return t('memory:memoryPage.fileType')
  if (type === 'risk') return t('memory:memoryPage.riskType')
  if (type === 'preference') return t('memory:memoryPage.preferenceType')
  if (type === 'decision') return t('memory:memoryPage.decisionType')
  if (type === 'fact') return t('memory:memoryPage.factType')
  if (type === 'task') return t('memory:memoryPage.taskType')
  return t('memory:memoryPage.conceptType')
}

const STAR_COLORS: Record<MemoryType, string> = {
  concept: 'var(--g-concept)',
  file: 'var(--g-file)',
  risk: 'var(--g-risk)',
  preference: 'var(--g-preference)',
  decision: 'var(--g-decision)',
  fact: 'var(--g-fact)',
  task: 'var(--g-task)',
}
const STAR_GLOWS: Record<MemoryType, string> = {
  concept: 'var(--g-concept-glow)',
  file: 'var(--g-file-glow)',
  risk: 'var(--g-risk-glow)',
  preference: 'var(--g-preference-glow)',
  decision: 'var(--g-decision-glow)',
  fact: 'var(--g-fact-glow)',
  task: 'var(--g-task-glow)',
}

function hashSeed(text: string) {
  let hash = 0
  for (let index = 0; index < text.length; index += 1)
    hash = (hash * 31 + text.charCodeAt(index)) >>> 0
  return hash
}

// 黄金角螺旋星系布局：服务端按重要度降序返回星辰，越重要越靠近星系核心；同一星域布局稳定
function galaxyLayout(nodes: MemoryNode[], spaceId: string): GalaxyStar[] {
  const stars = nodes.slice(0, MAX_STARS)
  if (!stars.length) return []
  if (stars.length === 1)
    return [{ node: stars[0], x: GALAXY_VIEW.cx, y: GALAXY_VIEW.cy, twinkle: 0 }]
  const seed = (hashSeed(spaceId || 'galaxy') % 628) / 100
  const base = 172 / Math.sqrt(stars.length)
  return stars.map((node, index) => {
    const jitter = hashSeed(node.id)
    const angle = index * GOLDEN_ANGLE + seed
    const radius = base * Math.sqrt(index + 0.55) + (jitter % 11) - 5
    return {
      node,
      x: GALAXY_VIEW.cx + Math.cos(angle) * radius * 1.38,
      y: GALAXY_VIEW.cy + Math.sin(angle) * radius * 0.74,
      twinkle: (jitter % 50) / 10,
    }
  })
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

// 关联线使用贝塞尔曲线：沿垂直方向轻微弯曲，相邻线交错方向，看起来更柔和
function linkCurve(source: GalaxyPoint, target: GalaxyPoint, seed: number) {
  const dx = target.x - source.x
  const dy = target.y - source.y
  const length = Math.hypot(dx, dy) || 1
  const bend = Math.min(24, length * 0.16) * (seed % 2 === 0 ? 1 : -1)
  const cx = (source.x + target.x) / 2 - (dy / length) * bend
  const cy = (source.y + target.y) / 2 + (dx / length) * bend
  return `M ${source.x.toFixed(1)} ${source.y.toFixed(1)} Q ${cx.toFixed(1)} ${cy.toFixed(1)} ${target.x.toFixed(1)} ${target.y.toFixed(1)}`
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export function MemoryPage({
  notify,
  query = '',
  registerPrimaryAction,
  requestConfirm,
}: MemoryPageProps) {
  const [detailOpen, setDetailOpen] = useState(false)

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
  const loadRevision = useRef(0)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [zoom, setZoom] = useState(1)
  const [nodeModal, setNodeModal] = useState<MemoryNodeModalState | null>(null)
  const [spaceModal, setSpaceModal] = useState<Partial<MemorySpace> | null>(null)
  const [hoveredId, setHoveredId] = useState('')
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
  const stageRef = useRef<HTMLDivElement | null>(null)
  const parallaxFrame = useRef(0)
  // 星系面板可见性：文档隐藏或面板离屏时暂停装饰动画与视差写入，降低后台功耗
  const { ref: observeGalaxy, playing: galaxyPlaying } = useAnimationVisibility<HTMLDivElement>()
  // 合并视差舞台 ref 与可见性观察 ref；引用稳定，避免每次渲染重建观察器
  const attachGalaxyStage = useCallback(
    (node: HTMLDivElement | null) => {
      stageRef.current = node
      observeGalaxy(node)
    },
    [observeGalaxy],
  )
  // 不可见时暂停面板内所有 CSS 无限动画（含 before/after 伪元素上的星云漂移与星点闪烁）
  const galaxyPauseClass = galaxyPlaying
    ? ''
    : '[&:before]:[animation-play-state:paused]! [&:after]:[animation-play-state:paused]! [&_*]:[animation-play-state:paused]! [&_*:before]:[animation-play-state:paused]! [&_*:after]:[animation-play-state:paused]!'
  usePagePrimaryAction(registerPrimaryAction, () =>
    setNodeModal({ spaceId: spaceId || data.selectedSpaceId }),
  )

  // 鼠标视差：星辰与连线按深度分层缓动跟随，rAF 节流避免高频写入
  const handleParallax = (event: ReactPointerEvent<HTMLDivElement>) => {
    // 不可见时不再调度视差写入；恢复可见后由 pointermove 自然恢复
    if (!galaxyPlaying) return
    const stage = stageRef.current
    if (!stage) return
    const rect = stage.getBoundingClientRect()
    const px = ((event.clientX - rect.left) / rect.width - 0.5) * 2
    const py = ((event.clientY - rect.top) / rect.height - 0.5) * 2
    cancelAnimationFrame(parallaxFrame.current)
    parallaxFrame.current = requestAnimationFrame(() => {
      stage.style.setProperty('--px', px.toFixed(3))
      stage.style.setProperty('--py', py.toFixed(3))
    })
  }
  const resetParallax = () => {
    cancelAnimationFrame(parallaxFrame.current)
    stageRef.current?.style.setProperty('--px', '0')
    stageRef.current?.style.setProperty('--py', '0')
  }

  // 切到不可见时取消尚未执行的视差帧，避免后台仍产生样式写入
  useEffect(() => {
    if (galaxyPlaying) return
    cancelAnimationFrame(parallaxFrame.current)
    parallaxFrame.current = 0
  }, [galaxyPlaying])

  // 不可见时同步暂停舞台 SVG 内的 SMIL 动画（聚焦连线上的流光点）；
  // pauseAnimations 暂停的是整个 SVG 时间轴，此后新插入的动画元素同样保持暂停
  useEffect(() => {
    const svg = stageRef.current?.querySelector('svg')
    if (!svg) return
    if (galaxyPlaying) svg.unpauseAnimations()
    else svg.pauseAnimations()
  }, [galaxyPlaying])

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
  const stars = useMemo(() => galaxyLayout(data.nodes, spaceId), [data.nodes, spaceId])
  const starById = useMemo(() => new Map(stars.map((star) => [star.node.id, star])), [stars])
  const visibleLinks = useMemo(() => {
    const visibleIds = new Set(stars.map((star) => star.node.id))
    return data.links.filter(
      (link) => visibleIds.has(link.sourceId) && visibleIds.has(link.targetId),
    )
  }, [stars, data.links])
  const relatedNodeIds = new Set(
    data.links.flatMap((link) => {
      if (link.sourceId === selectedId) return [link.targetId]
      if (link.targetId === selectedId) return [link.sourceId]
      return []
    }),
  )
  // 聚焦模式：悬停优先于选中，聚焦星的关系网保持明亮，其余星辰淡出
  const focusId = hoveredId || selectedId
  const focusRelatedIds = new Set(
    data.links.flatMap((link) => {
      if (link.sourceId === focusId) return [link.targetId]
      if (link.targetId === focusId) return [link.sourceId]
      return []
    }),
  )
  const relatedFiles = data.nodes.filter(
    (node) => node.type === 'file' && (relatedNodeIds.has(node.id) || node.id === selectedId),
  )
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

  return (
    <PageFrame variant="list-detail">
      <div className="flex h-full min-h-0 flex-col gap-3">
        <>
          <ListDetailLayout
            selected={detailOpen}
            onBack={() => setDetailOpen(false)}
            backLabel={t('common:pageTemplate.backToList')}
            list={
              <>
                <Button
                  variant="secondary"
                  className="mb-3 h-11 w-full @min-[760px]/list-detail:hidden"
                  onClick={() => setDetailOpen(true)}
                >
                  {t('common:pageTemplate.openDetail')}
                </Button>
                <div className="flex min-w-0 flex-col gap-4">
                  <section className={cn('wiki-panel', CARD)}>
                    <header className={CARD_HEAD}>
                      <CardTitle count={data.spaces.length}>
                        {t('memory:memoryPage.memorySpaces')}
                      </CardTitle>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="-mr-1.5 max-[650px]:size-11"
                        title={t('memory:memoryPage.newMemorySpace')}
                        aria-label={t('memory:memoryPage.newMemorySpace')}
                        onClick={() => setSpaceModal({})}
                      >
                        <Plus />
                      </Button>
                    </header>
                    <ul className="flex flex-col gap-0.5 p-1.5">
                      {data.spaces.map((space) => {
                        const active = space.id === spaceId
                        return (
                          <li
                            className={cn(
                              'relative rounded-md transition-colors duration-(--d-fast) hover:bg-surface-subtle',
                              active && 'bg-surface-muted hover:bg-surface-muted',
                            )}
                            key={space.id}
                          >
                            {/* 选中星域：次级表面 + 3px 强调色刻度。 */}
                            {active && <i aria-hidden="true" className={SELECTED_TICK} />}
                            <button
                              type="button"
                              aria-current={active || undefined}
                              className={cn(
                                'grid min-h-10 w-full min-w-0 grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-2 py-1.5 pr-2 pl-3 text-left text-sm focus-visible:-outline-offset-2 max-[650px]:min-h-11',
                                active && 'active',
                              )}
                              onClick={() => {
                                setSpaceId(space.id)
                                setSelectedId('')
                              }}
                            >
                              <span
                                className={cn(
                                  'truncate',
                                  active ? 'font-medium text-content' : 'text-content-soft',
                                )}
                              >
                                {spaceLabel(space, t)}
                              </span>
                              <small className="text-xs text-content-muted tabular-nums">
                                {t('memory:memoryPage.countMemories', { count: space.nodeCount })}
                              </small>
                              <ChevronRight className="size-3.5 text-content-faint" />
                            </button>
                          </li>
                        )
                      })}
                    </ul>
                    {selectedSpace && (
                      <div className="memory-space-actions flex gap-1 border-t border-stroke-soft px-1.5 py-1.5">
                        <Button
                          variant="ghost"
                          size="sm"
                          className="max-[650px]:h-11"
                          onClick={() => setSpaceModal(selectedSpace)}
                        >
                          <Pencil />
                          {t('memory:memoryPage.rename')}
                        </Button>
                        {selectedSpace.kind !== 'global' && (
                          <Button
                            variant="ghost"
                            size="sm"
                            className="danger text-danger hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
                            onClick={deleteSpace}
                          >
                            <Trash2 />
                            {t('memory:memoryPage.delete')}
                          </Button>
                        )}
                      </div>
                    )}
                  </section>
                  <section className={cn('memory-legend-panel', CARD)}>
                    <header className={CARD_HEAD}>
                      <CardTitle>{t('memory:memoryPage.memoryMapTypes')}</CardTitle>
                    </header>
                    <div className="galaxy-legend grid grid-cols-2 gap-x-3 gap-y-2 px-3.5 py-3">
                      {MEMORY_TYPES.map((type) => (
                        <span
                          className="flex items-center gap-2 text-xs text-content-secondary"
                          key={type}
                        >
                          <GalaxyDot type={type} />
                          {memoryTypeLabel(type, t)}
                        </span>
                      ))}
                    </div>
                  </section>
                  <section className={CARD}>
                    <header className={CARD_HEAD}>
                      <CardTitle count={data.candidates?.length || 0}>
                        {t('memory:memoryPage.memoryDrafts')}
                      </CardTitle>
                      {Boolean(data.candidates?.length) && (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="-mr-1.5 max-[650px]:h-11"
                          disabled={Boolean(resolvingCandidateId)}
                          onClick={ignoreAllCandidates}
                        >
                          <X />
                          {t('memory:memoryPage.ignoreAll')}
                        </Button>
                      )}
                    </header>
                    {loadingThreshold && (
                      <p
                        role="status"
                        className="flex items-center gap-2 px-3.5 pt-3 text-xs text-content-muted"
                      >
                        <OrbitSpinner className="size-3.5" />
                        {t('memory:memoryPage.loadingAutoApproveThreshold')}
                      </p>
                    )}
                    {thresholdError && (
                      <div className="flex flex-wrap items-center gap-2 px-3.5 pt-3">
                        <AppError className="my-0">
                          {t('memory:memoryPage.autoApproveThresholdUnavailable')} {thresholdError}
                        </AppError>
                        <Button
                          variant="secondary"
                          size="sm"
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
                      <div className="memory-auto-approve-row flex min-h-11 items-center justify-between gap-3 border-b border-stroke-soft px-3.5 py-1.5">
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
                    <div className="flex max-h-[320px] flex-col gap-2 overflow-auto p-3">
                      {(data.candidates || []).map((candidate) => (
                        <article
                          className="memory-candidate flex flex-col gap-1 rounded-md border border-stroke-soft bg-surface-subtle p-2.5 text-xs leading-[1.5] text-content-muted"
                          key={candidate.id}
                        >
                          <strong className="text-sm font-medium text-content">
                            {candidate.title}
                          </strong>
                          <small className="text-xs tabular-nums">
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
                          </small>
                          <span className="text-content-secondary">{candidate.content}</span>
                          {candidate.topicKey && (
                            <small className="text-xs">
                              {t('memory:memoryPage.topic')}：
                              <code className="font-mono">{candidate.topicKey}</code>
                            </small>
                          )}
                          {candidate.evidence && (
                            <small className="text-xs">
                              {t('memory:memoryPage.evidence')}：{candidate.evidence}
                            </small>
                          )}
                          <small className="text-xs tabular-nums">
                            {t('memory:memoryPage.created')}：
                            {formatMemoryTime(candidate.createdAt, language)}
                            {candidate.expiresAt
                              ? ` · ${t('memory:memoryPage.expires')}：${formatMemoryTime(candidate.expiresAt, language)}`
                              : ''}
                          </small>
                          <div className="mt-1 flex gap-1.5">
                            <Button
                              variant="secondary"
                              size="sm"
                              className="max-[650px]:h-11"
                              disabled={Boolean(resolvingCandidateId)}
                              onClick={() => resolveCandidate(candidate, 'accept')}
                            >
                              <Check />
                              {t('memory:memoryPage.confirm')}
                            </Button>
                            <Button
                              variant="ghost"
                              size="sm"
                              className="danger text-danger hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
                              disabled={Boolean(resolvingCandidateId)}
                              onClick={() => resolveCandidate(candidate, 'reject')}
                            >
                              <X />
                              {t('memory:memoryPage.ignore')}
                            </Button>
                          </div>
                        </article>
                      ))}
                      {!data.candidates?.length && (
                        <small className="text-xs text-content-muted">
                          {t('memory:memoryPage.thereAreNoPendingMemoryCandidates')}
                        </small>
                      )}
                    </div>
                  </section>
                </div>
              </>
            }
            detail={
              <Panel
                className={`graph-panel before:[content:''] before:absolute before:z-[0] before:inset-0 before:pointer-events-none before:bg-[url('/memory-galaxy-background.webp')] before:[background-position:center_48%] before:[background-size:cover] before:bg-no-repeat before:[filter:saturate(.62)_contrast(1.08)] before:[mix-blend-mode:screen] before:opacity-[.42] after:[content:''] after:absolute after:z-[0] after:inset-0 after:pointer-events-none after:[background-image:radial-gradient(var(--galaxy-dot)_.9px,transparent_1.2px),_radial-gradient(var(--galaxy-dot-dim)_.8px,transparent_1.2px),_radial-gradient(var(--galaxy-dot)_1.2px,transparent_1.6px)] after:[background-position:0_0,_37px_53px,_71px_19px] after:[background-size:96px_96px,_152px_152px,_236px_236px] after:[animation:galaxy-field-twinkle_6.4s_ease-in-out_infinite] after:opacity-[.7] before:inset-[-8%] before:[animation:galaxy-background-drift_34s_ease-in-out_infinite_alternate] before:[will-change:transform] after:[animation:galaxy-field-drift_15s_ease-in-out_infinite_alternate] after:[will-change:background-position,opacity] max-[650px]:min-h-[430px] relative min-h-[500px] overflow-hidden [border-color:var(--galaxy-border)] bg-[var(--galaxy-bg)]! [background-image:radial-gradient(ellipse_46%_40%_at_50%_48%,_var(--galaxy-core),_transparent_72%),_radial-gradient(ellipse_62%_48%_at_16%_6%,_var(--galaxy-nebula-a),_transparent_70%),_radial-gradient(ellipse_56%_52%_at_88%_98%,_var(--galaxy-nebula-b),_transparent_72%)] p-0 backdrop-blur-none! galaxy-panel ${galaxyPauseClass}`}
                onPointerMove={handleParallax}
                onPointerLeave={resetParallax}
              >
                {/* 星图始终是深空场景：工具条用星系专属 token，实色底不做背景模糊。 */}
                <div className="graph-toolbar absolute top-2.5 left-2.5 z-[4] flex gap-0.5 rounded-sm border border-[var(--galaxy-toolbar-border)] bg-[var(--galaxy-toolbar-bg)] p-0.5 text-[var(--galaxy-toolbar-text)] [&_button]:grid [&_button]:size-7 [&_button]:place-items-center [&_button]:rounded-xs [&_button]:transition-colors [&_button]:duration-(--d-fast) [&_button:hover]:bg-[var(--galaxy-toolbar-hover)] max-[650px]:[&_button]:size-11 [&_svg]:size-3.5">
                  <button
                    type="button"
                    title={t('memory:memoryPage.addMemory')}
                    aria-label={t('memory:memoryPage.addMemory')}
                    onClick={() => setNodeModal({ spaceId })}
                  >
                    <Plus />
                  </button>
                  <button
                    type="button"
                    title={t('memory:memoryPage.zoomIn')}
                    aria-label={t('memory:memoryPage.zoomIn')}
                    onClick={() =>
                      setZoom((value) => Math.min(1.6, Number((value + 0.15).toFixed(2))))
                    }
                  >
                    <ZoomIn />
                  </button>
                  <button
                    type="button"
                    title={t('memory:memoryPage.zoomOut')}
                    aria-label={t('memory:memoryPage.zoomOut')}
                    onClick={() =>
                      setZoom((value) => Math.max(0.7, Number((value - 0.15).toFixed(2))))
                    }
                  >
                    <ZoomOut />
                  </button>
                  <button
                    type="button"
                    title={t('memory:memoryPage.refresh')}
                    aria-label={t('memory:memoryPage.refresh')}
                    onClick={() => load(spaceId)}
                  >
                    {loading ? <OrbitSpinner className="text-current" /> : <RefreshCw />}
                  </button>
                </div>
                <div
                  className={`galaxy-stage [&_>_svg]:absolute [&_>_svg]:inset-0 [&_>_svg]:w-full [&_>_svg]:h-full [&_>_svg]:overflow-visible [&_>_svg]:z-[1] [&_>_svg]:[transform:translate3d(calc(var(--px,0)_*_9px),calc(var(--py,0)_*_6px),0)] [&_>_svg]:[transition:transform_.9s_var(--ease-out)] absolute z-[1] inset-0 [transform-origin:50%_50%] [transition:transform_var(--d2)_var(--ease-out)] ${focusId ? 'has-focus' : ''}`}
                  ref={attachGalaxyStage}
                  style={{ transform: `scale(${zoom})` }}
                >
                  <i
                    className="galaxy-spiral absolute z-[0] block pointer-events-none [left:9%] [top:13%] w-[82%] h-[74%] rounded-[50%] bg-[conic-gradient(from_-18deg,transparent_0deg,var(--galaxy-nebula-a)_28deg,transparent_68deg,var(--galaxy-nebula-b)_118deg,transparent_170deg,var(--galaxy-nebula-a)_224deg,transparent_278deg,var(--galaxy-nebula-b)_322deg,transparent_360deg)] [filter:blur(13px)] [mask-image:radial-gradient(ellipse,transparent_0%,transparent_15%,var(--g-star-core)_38%,transparent_74%)] [-webkit-mask-image:radial-gradient(ellipse,transparent_0%,transparent_15%,var(--g-star-core)_38%,transparent_74%)] [mix-blend-mode:screen] opacity-[.38] [animation:galaxy-spiral-turn_42s_linear_infinite] [will-change:transform]"
                    aria-hidden="true"
                  />
                  <i
                    className="galaxy-core absolute z-[0] block pointer-events-none before:[content:''] before:absolute before:left-[50%] before:top-[50%] before:w-[46px] before:h-[46px] before:rounded-[50%] before:bg-[radial-gradient(circle,rgba(255,255,255,.82),rgba(191,219,254,.3)_55%,transparent_76%)] before:[filter:blur(6px)] before:[transform:translate(-50%,-50%)] [left:50%] [top:49%] w-[34%] h-[40%] rounded-[50%] bg-[radial-gradient(ellipse,rgba(219,232,255,.3)_0%,rgba(126,156,255,.15)_36%,transparent_70%)] [filter:blur(9px)] [mix-blend-mode:screen] [transform:translate(-50%,-50%)] [animation:galaxy-core-breathe_7.5s_ease-in-out_infinite] [will-change:transform,opacity]"
                    aria-hidden="true"
                  />
                  <i
                    className="galaxy-aurora absolute z-[0] block pointer-events-none w-[46%] h-[31%] rounded-[50%] [filter:blur(21px)] [mix-blend-mode:screen] opacity-[.42] [will-change:transform,opacity] [left:8%] [top:20%] bg-[radial-gradient(ellipse,var(--galaxy-nebula-a),transparent_70%)] [animation:galaxy-aurora-drift_18s_ease-in-out_infinite_alternate]"
                    aria-hidden="true"
                  />
                  <i
                    className="galaxy-aurora absolute z-[0] block pointer-events-none w-[46%] h-[31%] rounded-[50%] [filter:blur(21px)] [mix-blend-mode:screen] opacity-[.42] [will-change:transform,opacity] [right:5%] [bottom:14%] bg-[radial-gradient(ellipse,var(--galaxy-nebula-b),transparent_70%)] [animation:galaxy-aurora-drift_23s_ease-in-out_-9s_infinite_alternate-reverse]"
                    aria-hidden="true"
                  />
                  <i
                    className="galaxy-aurora absolute z-[0] block pointer-events-none w-[46%] h-[31%] rounded-[50%] [filter:blur(21px)] [mix-blend-mode:screen] opacity-[.42] [will-change:transform,opacity] galaxy-aurora-three left-[30%] top-[60%] !w-[40%] !h-[27%] bg-[radial-gradient(ellipse,var(--galaxy-nebula-c),transparent_70%)] [animation:galaxy-aurora-drift_27s_ease-in-out_-14s_infinite_alternate]"
                    aria-hidden="true"
                  />
                  <svg viewBox="0 0 600 420" preserveAspectRatio="none" aria-hidden="true">
                    <g className="galaxy-orbits [&_ellipse]:[fill:none] [&_ellipse]:[stroke:var(--galaxy-line)] [&_ellipse]:[stroke-width:.72] [&_ellipse]:[stroke-dasharray:3_7] [&_ellipse]:opacity-[.52] [transform-box:fill-box] [transform-origin:center] [animation:galaxy-orbit-drift_90s_linear_infinite]">
                      <ellipse cx="300" cy="206" rx="205" ry="122" transform="rotate(-8 300 206)" />
                      <ellipse cx="300" cy="206" rx="154" ry="88" transform="rotate(18 300 206)" />
                      <ellipse cx="300" cy="206" rx="95" ry="54" transform="rotate(-24 300 206)" />
                    </g>
                    <defs>
                      {visibleLinks.map((link, index) => {
                        const source = starById.get(link.sourceId)
                        const target = starById.get(link.targetId)
                        if (!source || !target) return null
                        return (
                          <linearGradient
                            id={`lg-${index}`}
                            gradientUnits="userSpaceOnUse"
                            x1={source.x}
                            y1={source.y}
                            x2={target.x}
                            y2={target.y}
                            key={link.id}
                          >
                            <stop
                              offset="0"
                              stopColor={
                                STAR_COLORS[source.node.type] || 'var(--galaxy-line-active)'
                              }
                            />
                            <stop
                              offset="1"
                              stopColor={
                                STAR_COLORS[target.node.type] || 'var(--galaxy-line-active)'
                              }
                            />
                          </linearGradient>
                        )
                      })}
                    </defs>
                    {visibleLinks.map((link, index) => {
                      const source = starById.get(link.sourceId)
                      const target = starById.get(link.targetId)
                      if (!source || !target) return null
                      const active = link.sourceId === focusId || link.targetId === focusId
                      const path = linkCurve(source, target, hashSeed(link.id))
                      return (
                        <g key={link.id}>
                          <path
                            className={`link-path [.galaxy-stage_>_svg_&]:[fill:none] [.galaxy-stage_>_svg_&]:[stroke-width:1.05] [.galaxy-stage_>_svg_&]:[stroke-dasharray:3_5] [.galaxy-stage_>_svg_&]:opacity-[.5] [.galaxy-stage_>_svg_&]:[animation:galaxy-link-flow_30s_linear_infinite] [.galaxy-stage_>_svg_&]:[transition:opacity_var(--d2)_var(--ease-out)] [.galaxy-stage_>_svg_&.active]:[stroke-width:1.55] [.galaxy-stage_>_svg_&.active]:[stroke-dasharray:none] [.galaxy-stage_>_svg_&.active]:opacity-100 [.galaxy-stage_>_svg_&.active]:[filter:drop-shadow(0_0_3.5px_var(--galaxy-line-active))] [.galaxy-stage.has-focus_&]:opacity-[.14] [.galaxy-stage.has-focus_&.active]:opacity-100 ${active ? 'active' : ''}`}
                            d={path}
                            stroke={`url(#lg-${index})`}
                          />
                          {active && (
                            <circle
                              className="link-pulse [.galaxy-stage_>_svg_&]:[fill:var(--g-star-core)] [.galaxy-stage_>_svg_&]:[filter:drop-shadow(0_0_4px_var(--galaxy-line-active))] [.galaxy-stage_>_svg_&]:pointer-events-none"
                              r="2.1"
                            >
                              <animateMotion dur="2.8s" repeatCount="indefinite" path={path} />
                            </circle>
                          )}
                        </g>
                      )
                    })}
                  </svg>
                  <i
                    className="galaxy-meteor before:[content:''] before:absolute before:top-[50%] before:left-[1px] before:w-[72px] before:h-[1px] before:bg-[linear-gradient(90deg,transparent,var(--galaxy-dot))] before:[transform:translateY(-50%)] absolute [top:14%] [left:80%] w-[2.4px] h-[2.4px] rounded-[50%] bg-[var(--galaxy-label-active)] shadow-[0_0_7px_1px_var(--galaxy-dot)] opacity-0 pointer-events-none [animation:galaxy-meteor_12s_linear_infinite] z-[3]"
                    aria-hidden="true"
                  />
                  <i
                    className="galaxy-meteor before:[content:''] before:absolute before:top-[50%] before:left-[1px] before:w-[72px] before:h-[1px] before:bg-[linear-gradient(90deg,transparent,var(--galaxy-dot))] before:[transform:translateY(-50%)] absolute [top:14%] [left:80%] w-[2.4px] h-[2.4px] rounded-[50%] bg-[var(--galaxy-label-active)] shadow-[0_0_7px_1px_var(--galaxy-dot)] opacity-0 pointer-events-none [animation:galaxy-meteor_12s_linear_infinite] z-[3] meteor-two [.galaxy-meteor&]:top-[63%] [.galaxy-meteor&]:left-[89%] [.galaxy-meteor&]:[animation:galaxy-meteor-b_17s_linear_4.5s_infinite]"
                    aria-hidden="true"
                  />
                  <i
                    className="galaxy-meteor before:[content:''] before:absolute before:top-[50%] before:left-[1px] before:w-[72px] before:h-[1px] before:bg-[linear-gradient(90deg,transparent,var(--galaxy-dot))] before:[transform:translateY(-50%)] absolute [top:14%] [left:80%] w-[2.4px] h-[2.4px] rounded-[50%] bg-[var(--galaxy-label-active)] shadow-[0_0_7px_1px_var(--galaxy-dot)] opacity-0 pointer-events-none [animation:galaxy-meteor_12s_linear_infinite] z-[3] meteor-three [.galaxy-meteor&]:top-[7%] [.galaxy-meteor&]:left-[36%] [.galaxy-meteor&]:[animation:galaxy-meteor-c_23s_linear_11s_infinite]"
                    aria-hidden="true"
                  />
                  {stars.map(({ node, x, y, twinkle }, index) => (
                    <button
                      className={`galaxy-star focus-visible:rounded-[50%] focus-visible:[outline-color:var(--galaxy-line-active)] before:[content:''] before:absolute before:w-[calc(var(--star-size)_+_26px)] before:h-[calc(var(--star-size)_+_26px)] before:rounded-[50%] before:bg-[radial-gradient(circle,var(--g-star-glow),transparent_68%)] before:[filter:blur(3px)] before:opacity-[.76] before:pointer-events-none [&.active::after]:[content:''] [&.active::after]:absolute [&.active::after]:w-[calc(var(--star-size)_+_31px)] [&.active::after]:h-[calc(var(--star-size)_+_31px)] [&.active::after]:[border:1px_solid_var(--g-star-glow)] [&.active::after]:rounded-[50%] [&.active::after]:pointer-events-none [&.active::after]:[animation:galaxy-star-selection-pulse_2.3s_var(--ease-out)_infinite] [&:hover::before]:opacity-100 [&.active::before]:opacity-100 absolute z-[2] grid w-[52px] h-[52px] min-w-0 min-h-[52px] place-items-center [transform:translate(-50%,-50%)_translate3d(calc(var(--px,0)_*_(9px_+_var(--depth,0)_*_7px)),calc(var(--py,0)_*_(6px_+_var(--depth,0)_*_5px)),0)] border-0 rounded-[50%] bg-transparent p-0 cursor-pointer [transition:transform_.9s_var(--ease-out),opacity_.45s_var(--ease-out),filter_.45s_var(--ease-out)] [animation:galaxy-star-enter_.72s_var(--ease-out)_var(--enter-delay,0ms)_backwards] ${selectedId === node.id ? 'active' : ''}    ${focusId && node.id !== focusId && !focusRelatedIds.has(node.id) ? 'dimmed [.galaxy-stage.has-focus_.galaxy-star&]:opacity-[.32] [.galaxy-stage.has-focus_.galaxy-star&]:[filter:saturate(.4)_brightness(.82)]' : ''}`}
                      onClick={() => {
                        setDetailOpen(true)
                        setSelectedId(node.id)
                      }}
                      onMouseEnter={() => setHoveredId(node.id)}
                      onMouseLeave={() => setHoveredId('')}
                      style={
                        {
                          left: `${(x / GALAXY_VIEW.width) * 100}%`,
                          top: `${(y / GALAXY_VIEW.height) * 100}%`,
                          '--star-size': `${12 + Math.round((node.importance || 0.5) * 9)}px`,
                          '--twinkle-delay': `${twinkle}s`,
                          '--enter-delay': `${index * 55}ms`,
                          '--depth': (hashSeed(node.id) % 100) / 100,
                          '--g-star-color': STAR_COLORS[node.type],
                          '--g-star-glow': STAR_GLOWS[node.type],
                        } as GalaxyStarStyle
                      }
                      title={node.content}
                      key={node.id}
                    >
                      {selectedId === node.id && (
                        <i
                          className="orbit-ring [.galaxy-star_&]:absolute [.galaxy-star_&]:left-[50%] [.galaxy-star_&]:top-[50%] [.galaxy-star_&]:w-[calc(var(--star-size)_+_36px)] [.galaxy-star_&]:h-[calc(var(--star-size)_+_36px)] [.galaxy-star_&]:[border:1px_solid_var(--galaxy-line-active)] [.galaxy-star_&]:rounded-[50%] [.galaxy-star_&]:pointer-events-none [.galaxy-star_&]:[transform:translate(-50%,-50%)_rotate(-22deg)_scaleY(.44)] [.galaxy-star_&]:[animation:galaxy-selected-orbit_6s_linear_infinite] [.galaxy-star_&]:opacity-[.8] [.galaxy-star_&::before]:[content:''] [.galaxy-star_&::before]:absolute [.galaxy-star_&::before]:top-[-2.2px] [.galaxy-star_&::before]:left-[50%] [.galaxy-star_&::before]:w-[4.4px] [.galaxy-star_&::before]:h-[4.4px] [.galaxy-star_&::before]:ml-[-2.2px] [.galaxy-star_&::before]:rounded-[50%] [.galaxy-star_&::before]:bg-[var(--g-star-color)] [.galaxy-star_&::before]:shadow-[0_0_6px_1px_var(--g-star-glow)]"
                          aria-hidden="true"
                        />
                      )}
                      {selectedId === node.id && (
                        <i
                          className="dash-ring [.galaxy-star_&]:absolute [.galaxy-star_&]:left-[50%] [.galaxy-star_&]:top-[50%] [.galaxy-star_&]:w-[calc(var(--star-size)_+_23px)] [.galaxy-star_&]:h-[calc(var(--star-size)_+_23px)] [.galaxy-star_&]:[border:1px_dashed_var(--g-star-glow)] [.galaxy-star_&]:rounded-[50%] [.galaxy-star_&]:opacity-[.9] [.galaxy-star_&]:pointer-events-none [.galaxy-star_&]:[transform:translate(-50%,-50%)] [.galaxy-star_&]:[animation:galaxy-dash-spin_16s_linear_infinite]"
                          aria-hidden="true"
                        />
                      )}
                      <svg
                        className="star-core [.galaxy-star_&]:relative [.galaxy-star_&]:block [.galaxy-star_&]:w-[var(--star-size)] [.galaxy-star_&]:h-[var(--star-size)] [.galaxy-star_&]:[transition:transform_var(--d2)_var(--ease-spring)] [.galaxy-star_&]:[animation:galaxy-star-twinkle_3.8s_ease-in-out_var(--twinkle-delay)_infinite] [.galaxy-star_&]:overflow-visible [.galaxy-star_&]:rounded-[0] [.galaxy-star_&]:bg-transparent [.galaxy-star_&]:shadow-[none] [.galaxy-star_&]:[filter:drop-shadow(0_0_3px_var(--g-star-glow))_drop-shadow(0_0_8px_var(--g-star-glow))] [.galaxy-star_&]:origin-[center] [.galaxy-star:hover_&]:[transform:scale(1.28)] [.galaxy-star.active_&]:[transform:scale(1.32)] [.galaxy-star.active_&]:[animation:galaxy-star-twinkle_2.1s_ease-in-out_var(--twinkle-delay)_infinite]"
                        viewBox="0 0 32 32"
                        aria-hidden="true"
                      >
                        <path
                          className="star-ray [.galaxy-star_.star-core_&]:[fill:var(--g-star-glow)] [.galaxy-star_.star-core_&]:opacity-[.58] [.galaxy-star_.star-core_&]:[transform-box:fill-box] [.galaxy-star_.star-core_&]:origin-[center] [.galaxy-star_.star-core_&]:[transform:scale(1.45)] [.galaxy-star.active_.star-core_&]:opacity-[.88] [.galaxy-star.active_.star-core_&]:[transform:scale(1.82)]"
                          d="M16 0 L19.7 12.3 L32 16 L19.7 19.7 L16 32 L12.3 19.7 L0 16 L12.3 12.3 Z"
                        />
                        <path
                          className="star-shape [.galaxy-star_.star-core_&]:[fill:var(--g-star-color)]"
                          d="M16 2 L18.8 13.2 L30 16 L18.8 18.8 L16 30 L13.2 18.8 L2 16 L13.2 13.2 Z"
                        />
                        <circle
                          className="star-heart [.galaxy-star_.star-core_&]:[fill:var(--g-star-core)]"
                          cx="16"
                          cy="16"
                          r="3.2"
                        />
                      </svg>
                      <span className="star-label [.galaxy-stage.has-focus_.galaxy-star.dimmed_&]:opacity-[.3] [.galaxy-stage.has-focus_.galaxy-star.dimmed_&]:[transition:opacity_.45s_var(--ease-out)] [.galaxy-star_&]:absolute [.galaxy-star_&]:top-[calc(50%_+_17px)] [.galaxy-star_&]:left-[50%] [.galaxy-star_&]:max-w-[120px] [.galaxy-star_&]:overflow-hidden [.galaxy-star_&]:text-[var(--galaxy-label)] [.galaxy-star_&]:text-[11px] [.galaxy-star_&]:leading-[1.35] [.galaxy-star_&]:pointer-events-none [.galaxy-star_&]:text-ellipsis [.galaxy-star_&]:[text-shadow:0_1px_7px_var(--galaxy-bg)] [.galaxy-star_&]:[transform:translateX(-50%)] [.galaxy-star_&]:[transition:color_var(--d1)_var(--ease-out)] [.galaxy-star_&]:whitespace-nowrap [.galaxy-star:hover_&]:text-[var(--galaxy-label-active)] [.galaxy-star.active_&]:text-[var(--galaxy-label-active)] [.galaxy-star.active_&]:font-[700]">
                        {node.title}
                      </span>
                    </button>
                  ))}
                </div>
                {!!stars.length && (
                  <span className="pointer-events-none absolute right-3 bottom-2.5 z-[3] rounded-xs border border-[var(--galaxy-toolbar-border)] bg-[var(--galaxy-toolbar-bg)] px-2 text-xs leading-5 text-[var(--galaxy-label)] tabular-nums">
                    ✦{' '}
                    {t('memory:memoryPage.visibleTotalMemories', {
                      visible: stars.length,
                      total: data.nodes.length,
                    })}
                  </span>
                )}
                {!loading && !stars.length && (
                  <div className="memory-empty [.galaxy-panel_&]:text-[var(--galaxy-label)] [.galaxy-panel_&]:[--stroke:var(--galaxy-toolbar-border)] [.galaxy-panel_&]:[--control-muted:var(--galaxy-label)] [.galaxy-panel_&]:[--star:var(--g-decision)] [.galaxy-panel_&]:[--text-muted:var(--galaxy-label)] absolute z-[3] inset-0 grid [align-content:center] justify-items-center gap-[8px] text-[var(--text-muted)] text-[12px] pointer-events-none">
                    <StarOrbit size={44} />
                    <span>
                      {t(
                        'memory:memoryPage.thisMemorySpaceIsStillUnlitClickInTheTopLeftToPlantTheFirstStar',
                      )}
                    </span>
                  </div>
                )}
              </Panel>
            }
            inspector={
              <div className="flex min-w-0 flex-col gap-4">
                <section className={CARD}>
                  <header className="flex min-w-0 flex-col gap-1 px-3.5 pt-3 pb-2.5">
                    <small className="text-xs font-medium tracking-[.02em] text-content-muted">
                      {t('memory:memoryPage.selectedMemory')}
                    </small>
                    {selected ? (
                      <>
                        <h2 className="text-[15px] leading-6 font-medium break-words text-content">
                          {selected.title}
                        </h2>
                        <p className="text-xs leading-normal break-words text-content-secondary">
                          {selected.content}
                        </p>
                      </>
                    ) : (
                      <p className="text-xs leading-normal text-content-muted">
                        {t('memory:memoryPage.selectAMemoryInTheMapToViewItsDetails')}
                      </p>
                    )}
                  </header>
                  {selected && (
                    <>
                      <dl className="px-3.5">
                        <div className={KEY_VALUE}>
                          <dt>{t('memory:memoryPage.memoryType')}</dt>
                          <dd className="inline-flex items-center justify-end gap-2">
                            <GalaxyDot type={selected.type} />
                            {memoryTypeLabel(selected.type, t)}
                          </dd>
                        </div>
                        <div className={KEY_VALUE}>
                          <dt>{t('memory:memoryPage.source')}</dt>
                          <dd>
                            {selected.sourceType === 'conversation_confirmed'
                              ? t('memory:memoryPage.userConfirmedConversationCandidate')
                              : selected.sourceType === 'user_confirmed'
                                ? t('memory:memoryPage.explicitUserRequest')
                                : selected.sourceType === 'agent'
                                  ? t('memory:memoryPage.createdByAgent')
                                  : t('memory:memoryPage.addedManually')}
                          </dd>
                        </div>
                        <div className={KEY_VALUE}>
                          <dt>{t('memory:memoryPage.authority')}</dt>
                          <dd className="tabular-nums">{selected.authority ?? 0}/100</dd>
                        </div>
                        {selected.evidence && (
                          <div className={KEY_VALUE}>
                            <dt>{t('memory:memoryPage.evidence')}</dt>
                            <dd title={selected.evidence}>{selected.evidence}</dd>
                          </div>
                        )}
                        <div className={KEY_VALUE}>
                          <dt>{t('memory:memoryPage.created')}</dt>
                          <dd className="tabular-nums">
                            {formatMemoryTime(selected.createdAt, language)}
                          </dd>
                        </div>
                        <div className={KEY_VALUE}>
                          <dt>{t('memory:memoryPage.relatedMemories')}</dt>
                          <dd className="tabular-nums">
                            {t('memory:memoryPage.count', { count: relatedNodeIds.size })}
                          </dd>
                        </div>
                      </dl>
                      <footer className="flex flex-wrap justify-end gap-2 border-t border-stroke-soft bg-surface-subtle px-3.5 py-2.5">
                        <Button
                          variant="ghost"
                          className="text-danger hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
                          onClick={() => deleteNode(selected)}
                        >
                          <Trash2 />
                          {t('memory:memoryPage.delete')}
                        </Button>
                        <Button
                          variant="secondary"
                          className="max-[650px]:h-11"
                          onClick={() => setNodeModal(selected)}
                        >
                          <Pencil />
                          {t('memory:memoryPage.edit')}
                        </Button>
                      </footer>
                    </>
                  )}
                  {error && <AppError className="mx-3.5 mb-3">{error}</AppError>}
                </section>
                <section className={CARD}>
                  <header className={CARD_HEAD}>
                    <CardTitle count={relatedFiles.length}>
                      {t('memory:memoryPage.relatedFiles')}
                    </CardTitle>
                  </header>
                  {relatedFiles.length ? (
                    <ul className="divide-y divide-stroke-soft">
                      {relatedFiles.map((file) => (
                        <li
                          className="file-row grid min-h-12 min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 py-1.5 pr-1.5 pl-3.5"
                          key={file.id}
                        >
                          <FileCode2 className="size-4 text-content-muted" />
                          <span className="flex min-w-0 flex-col gap-0.5">
                            <strong className="truncate text-sm font-medium text-content">
                              {file.title}
                            </strong>
                            <small className="truncate font-mono text-xs text-content-muted">
                              {file.sourcePath || file.cwd || t('memory:memoryPage.localMemory')}
                            </small>
                          </span>
                          <span className="flex items-center">
                            <Button
                              variant="ghost"
                              size="sm"
                              className="max-[650px]:h-11"
                              onClick={() => {
                                setDetailOpen(true)
                                setSelectedId(file.id)
                              }}
                            >
                              {t('memory:memoryPage.view')}
                            </Button>
                            <Button
                              variant="ghost"
                              size="icon"
                              className="max-[650px]:size-11"
                              aria-label={`${t('memory:memoryPage.edit')}: ${file.title}`}
                              onClick={() => setNodeModal(file)}
                            >
                              <Pencil />
                            </Button>
                            <Button
                              variant="ghost"
                              size="icon"
                              className="danger text-content-muted hover:bg-danger-soft hover:text-danger max-[650px]:size-11"
                              aria-label={`${t('memory:memoryPage.delete')}: ${file.title}`}
                              onClick={() => deleteNode(file)}
                            >
                              <Trash2 />
                            </Button>
                          </span>
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <p className="px-3.5 py-3 text-xs text-content-muted">
                      {t('memory:memoryPage.thisMemoryHasNoRelatedFiles')}
                    </p>
                  )}
                </section>
              </div>
            }
          />
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
        </>
      </div>
    </PageFrame>
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
