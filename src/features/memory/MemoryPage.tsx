// 记忆页：查看/搜索/删除 Agent 会话记忆条目，按时间线浏览。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Check,
  ChevronRight,
  FileCode2,
  LoaderCircle,
  Pencil,
  Plus,
  RefreshCw,
  Trash2,
  X,
  ZoomIn,
  ZoomOut,
} from 'lucide-react'
import {
  AppCard as Panel,
  AppSectionTitle as SectionTitle,
  AppCardHeader,
  AppError,
} from '@/components/ui/app-primitives'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { StarOrbit } from '@/components/StarOrbit'
import { apiJson } from '@/lib/api'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import type { CSSProperties, FormEvent, PointerEvent as ReactPointerEvent } from 'react'
import type { Notify } from '@/app/route-context'
import type { I18nValues } from '@/app/i18n'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

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
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [zoom, setZoom] = useState(1)
  const [nodeModal, setNodeModal] = useState<MemoryNodeModalState | null>(null)
  const [spaceModal, setSpaceModal] = useState<Partial<MemorySpace> | null>(null)
  const [hoveredId, setHoveredId] = useState('')
  const [resolvingCandidateId, setResolvingCandidateId] = useState('')
  const [autoApproveConfidence, setAutoApproveConfidence] = useState<number | null>(null)
  const [savingThreshold, setSavingThreshold] = useState(false)
  const stageRef = useRef<HTMLDivElement | null>(null)
  const parallaxFrame = useRef(0)
  usePagePrimaryAction(registerPrimaryAction, () =>
    setNodeModal({ spaceId: spaceId || data.selectedSpaceId }),
  )

  // 鼠标视差：星辰与连线按深度分层缓动跟随，rAF 节流避免高频写入
  const handleParallax = (event: ReactPointerEvent<HTMLDivElement>) => {
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

  // 加载记忆数据：按空间/搜索词拉取节点与候选；选中项失效时回退首节点。
  const load = useCallback(
    async (requestedSpaceId = '') => {
      setLoading(true)
      setError('')
      try {
        const params = new URLSearchParams()
        if (requestedSpaceId) params.set('spaceId', requestedSpaceId)
        if (query.trim()) params.set('query', query.trim())
        const result = await apiJson<MemoryData>(`/api/memory?${params}`)
        setData(result)
        setSpaceId(result.selectedSpaceId || '')
        setSelectedId((current) =>
          result.nodes.some((node) => node.id === current) ? current : result.nodes[0]?.id || '',
        )
      } catch (loadError) {
        setError(errorMessage(loadError))
      } finally {
        setLoading(false)
      }
    },
    [query],
  )

  useEffect(() => {
    load(spaceId)
  }, [load, spaceId])

  useEffect(() => {
    let active = true
    apiJson<{ autoApproveConfidence: number }>('/api/settings/memory')
      .then((preference) => active && setAutoApproveConfidence(preference.autoApproveConfidence))
      .catch(() => active && setAutoApproveConfidence(60))
    return () => {
      active = false
    }
  }, [])

  const saveAutoApproveConfidence = async () => {
    const value = autoApproveConfidence
    if (value === null || savingThreshold) return
    const normalized = Math.min(100, Math.max(0, Math.round(value)))
    if (normalized !== value) setAutoApproveConfidence(normalized)
    setSavingThreshold(true)
    try {
      await apiJson('/api/settings/memory', {
        method: 'PATCH',
        body: { autoApproveConfidence: normalized },
      })
      notify(t('memory:memoryPage.autoApproveSaved'))
    } catch (thresholdError) {
      setError(errorMessage(thresholdError))
    } finally {
      setSavingThreshold(false)
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
    <div className="memory-layout max-[1150px]:grid-cols-[220px_minmax(340px,1fr)] max-[650px]:grid-cols-[1fr] grid min-h-[100%] grid-cols-[230px_minmax(360px,1fr)_290px] gap-[12px]">
      <div className="flex min-w-0 flex-col gap-[12px]">
        <Panel className="wiki-panel [&_>_button]:grid [&_>_button]:w-full [&_>_button]:grid-cols-[minmax(0,1fr)_auto_auto] [&_>_button]:items-center [&_>_button]:gap-[6px] [&_>_button]:border-0 [&_>_button]:[border-top:1px_solid_var(--stroke-soft)] [&_>_button]:bg-transparent [&_>_button]:p-[9px_5px] [&_>_button]:text-left [&_>_button]:text-[12px] [&_>_button.active]:rounded-[var(--r-xs)] [&_>_button.active]:bg-[var(--accent-soft)] [&_>_button_small]:text-[var(--text-muted)] [&_>_button_small]:text-[13px]">
          <AppCardHeader>
            <SectionTitle title={t('memory:memoryPage.memorySpaces')} />
            <Button
              variant="ghost"
              size="icon"
              title={t('memory:memoryPage.newMemorySpace')}
              onClick={() => setSpaceModal({})}
            >
              <Plus size={13} />
            </Button>
          </AppCardHeader>
          {data.spaces.map((space) => (
            <button
              className={space.id === spaceId ? 'active' : ''}
              onClick={() => {
                setSpaceId(space.id)
                setSelectedId('')
              }}
              key={space.id}
            >
              <span>{spaceLabel(space, t)}</span>
              <small>{t('memory:memoryPage.countMemories', { count: space.nodeCount })}</small>
              <ChevronRight size={13} />
            </button>
          ))}
          {selectedSpace && (
            <div className="memory-space-actions [&_button]:inline-flex [&_button]:items-center [&_button]:gap-[4px] [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[var(--text-soft)] [&_button]:text-[13px] [&_button.danger]:text-[var(--danger)] flex gap-[6px] [margin-top:10px] [border-top:1px_solid_var(--stroke-soft)] [padding-top:9px]">
              <button onClick={() => setSpaceModal(selectedSpace)}>
                <Pencil size={12} />
                {t('memory:memoryPage.rename')}
              </button>
              {selectedSpace.kind !== 'global' && (
                <button className="danger" onClick={deleteSpace}>
                  <Trash2 size={12} />
                  {t('memory:memoryPage.delete')}
                </button>
              )}
            </div>
          )}
        </Panel>
        <Panel className="memory-legend-panel">
          <SectionTitle title={t('memory:memoryPage.memoryMapTypes')} />
          <div className="galaxy-legend [&_span]:flex [&_span]:items-center [&_span]:gap-[7px] [&_span]:text-[var(--text-soft)] [&_span]:text-[12px] grid grid-cols-[repeat(2,minmax(0,1fr))] gap-[8px_10px] [margin-top:11px]">
            {MEMORY_TYPES.map((type) => (
              <span key={type}>
                <i
                  className={`g-dot [&.g-concept]:bg-[var(--g-concept)] [&.g-concept]:shadow-[0_0_5px_var(--g-concept-glow)] [&.g-file]:bg-[var(--g-file)] [&.g-file]:shadow-[0_0_5px_var(--g-file-glow)] [&.g-risk]:bg-[var(--g-risk)] [&.g-risk]:shadow-[0_0_5px_var(--g-risk-glow)] [&.g-preference]:bg-[var(--g-preference)] [&.g-preference]:shadow-[0_0_5px_var(--g-preference-glow)] [&.g-decision]:bg-[var(--g-decision)] [&.g-decision]:shadow-[0_0_5px_var(--g-decision-glow)] [&.g-fact]:bg-[var(--g-fact)] [&.g-fact]:shadow-[0_0_5px_var(--g-fact-glow)] [&.g-task]:bg-[var(--g-task)] [&.g-task]:shadow-[0_0_5px_var(--g-task-glow)] inline-block w-[7px] h-[7px] flex-none rounded-[50%] g-${type}`}
                />
                {memoryTypeLabel(type, t)}
              </span>
            ))}
          </div>
        </Panel>
        <Panel className="min-h-0">
          <AppCardHeader>
            <SectionTitle
              title={`${t('memory:memoryPage.memoryDrafts')} · ${data.candidates?.length || 0}`}
            />
            {Boolean(data.candidates?.length) && (
              <Button
                variant="outline"
                className="bg-surface-subtle"
                disabled={Boolean(resolvingCandidateId)}
                onClick={ignoreAllCandidates}
              >
                <X size={12} />
                {t('memory:memoryPage.ignoreAll')}
              </Button>
            )}
          </AppCardHeader>
          {autoApproveConfidence !== null && (
            <div className="memory-auto-approve-row [&_label]:flex [&_label]:min-w-0 [&_label]:flex-col [&_label]:gap-[2px] [&_label_span]:text-[12px] [&_label_span]:font-[600] [&_label_small]:text-[var(--text-muted)] [&_label_small]:text-[10px] [&_label_small]:leading-[1.4] flex items-center justify-between gap-[10px] [border-bottom:1px_solid_var(--stroke-soft)] [padding:8px_0_10px]">
              <label>
                <span>{t('memory:memoryPage.autoApproveThreshold')}</span>
                <small>{t('memory:memoryPage.autoApproveThresholdHint')}</small>
              </label>
              <span className="memory-auto-approve-input [&_input]:w-[64px] [&_input]:[border:1px_solid_var(--stroke-soft)] [&_input]:rounded-[6px] [&_input]:bg-[var(--surface-muted)] [&_input]:p-[4px_6px] [&_input]:text-[var(--text)] [&_input]:text-[12px] [&_input]:text-right [&_em]:text-[var(--text-muted)] [&_em]:text-[11px] [&_em]:[font-style:normal] flex flex-none items-center gap-[5px]">
                <input
                  type="number"
                  min={0}
                  max={100}
                  step={5}
                  value={autoApproveConfidence}
                  disabled={savingThreshold}
                  aria-label={t('memory:memoryPage.autoApproveThreshold')}
                  onChange={(event) =>
                    setAutoApproveConfidence(event.target.value ? Number(event.target.value) : 0)
                  }
                  onBlur={() => void saveAutoApproveConfidence()}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') {
                      event.preventDefault()
                      void saveAutoApproveConfidence()
                    }
                  }}
                />
                <em>%</em>
                {savingThreshold && <LoaderCircle className="animate-spin" size={12} />}
              </span>
            </div>
          )}
          <div className="flex max-h-[300px] flex-col gap-[8px] overflow-auto">
            {(data.candidates || []).map((candidate) => (
              <div
                className="memory-candidate [&_>_strong]:text-[12px] [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[11px] [&_>_span]:leading-[1.45] [&_>_small]:text-[var(--text-muted)] [&_>_small]:text-[11px] [&_>_small]:leading-[1.45] [&_>_div]:flex [&_>_div]:gap-[6px] [&_button]:inline-flex [&_button]:items-center [&_button]:gap-[4px] [&_button]:[border:1px_solid_var(--stroke-soft)] [&_button]:rounded-[6px] [&_button]:bg-transparent [&_button]:p-[4px_7px] [&_button]:text-[var(--text)] [&_button]:text-[11px] [&_button.danger]:text-[var(--danger)] flex flex-col gap-[5px] [border:1px_solid_var(--stroke-soft)] rounded-[8px] bg-[var(--surface-muted)] [padding:8px]"
                key={candidate.id}
              >
                <strong>{candidate.title}</strong>
                <small>
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
                <span>{candidate.content}</span>
                {candidate.topicKey && (
                  <small>
                    {t('memory:memoryPage.topic')}：{candidate.topicKey}
                  </small>
                )}
                {candidate.evidence && (
                  <small>
                    {t('memory:memoryPage.evidence')}：{candidate.evidence}
                  </small>
                )}
                <small>
                  {t('memory:memoryPage.created')}：
                  {formatMemoryTime(candidate.createdAt, language)}
                  {candidate.expiresAt
                    ? ` · ${t('memory:memoryPage.expires')}：${formatMemoryTime(candidate.expiresAt, language)}`
                    : ''}
                </small>
                <div>
                  <button
                    disabled={Boolean(resolvingCandidateId)}
                    title={t('memory:memoryPage.confirm')}
                    onClick={() => resolveCandidate(candidate, 'accept')}
                  >
                    <Check size={12} />
                    {t('memory:memoryPage.confirm')}
                  </button>
                  <button
                    disabled={Boolean(resolvingCandidateId)}
                    className="danger"
                    title={t('memory:memoryPage.ignore')}
                    onClick={() => resolveCandidate(candidate, 'reject')}
                  >
                    <X size={12} />
                    {t('memory:memoryPage.ignore')}
                  </button>
                </div>
              </div>
            ))}
            {!data.candidates?.length && (
              <small>{t('memory:memoryPage.thereAreNoPendingMemoryCandidates')}</small>
            )}
          </div>
        </Panel>
      </div>
      <Panel
        className="graph-panel before:[content:''] before:absolute before:z-[0] before:inset-0 before:pointer-events-none before:bg-[url('/memory-galaxy-background.webp')] before:[background-position:center_48%] before:[background-size:cover] before:bg-no-repeat before:[filter:saturate(.62)_contrast(1.08)] before:[mix-blend-mode:screen] before:opacity-[.42] after:[content:''] after:absolute after:z-[0] after:inset-0 after:pointer-events-none after:[background-image:radial-gradient(var(--galaxy-dot)_.9px,transparent_1.2px),_radial-gradient(var(--galaxy-dot-dim)_.8px,transparent_1.2px),_radial-gradient(var(--galaxy-dot)_1.2px,transparent_1.6px)] after:[background-position:0_0,_37px_53px,_71px_19px] after:[background-size:96px_96px,_152px_152px,_236px_236px] after:[animation:galaxy-field-twinkle_6.4s_ease-in-out_infinite] after:opacity-[.7] before:inset-[-8%] before:[animation:galaxy-background-drift_34s_ease-in-out_infinite_alternate] before:[will-change:transform] after:[animation:galaxy-field-drift_15s_ease-in-out_infinite_alternate] after:[will-change:background-position,opacity] max-[650px]:min-h-[430px] relative min-h-[500px] overflow-hidden [border-color:var(--galaxy-border)] bg-[var(--galaxy-bg)]! [background-image:radial-gradient(ellipse_46%_40%_at_50%_48%,_var(--galaxy-core),_transparent_72%),_radial-gradient(ellipse_62%_48%_at_16%_6%,_var(--galaxy-nebula-a),_transparent_70%),_radial-gradient(ellipse_56%_52%_at_88%_98%,_var(--galaxy-nebula-b),_transparent_72%)] p-0 backdrop-blur-none! galaxy-panel"
        onPointerMove={handleParallax}
        onPointerLeave={resetParallax}
      >
        <div className="graph-toolbar [&_button]:grid [&_button]:w-[32px] [&_button]:h-[32px] [&_button]:place-items-center [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[12px] [.galaxy-panel_&]:border-[var(--galaxy-toolbar-border)] [.galaxy-panel_&]:bg-[var(--galaxy-toolbar-bg)] [.galaxy-panel_&]:text-[var(--galaxy-toolbar-text)] [.galaxy-panel_&]:[backdrop-filter:blur(9px)] [.galaxy-panel_&_button:hover]:bg-[var(--galaxy-toolbar-hover)] dark:bg-[var(--solid)] dark:text-[var(--text)] absolute z-[4] [top:10px] [left:10px] flex gap-[3px] [padding:3px] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--solid)]">
          <button
            title={t('memory:memoryPage.addMemory')}
            onClick={() => setNodeModal({ spaceId })}
          >
            <Plus size={14} />
          </button>
          <button
            title={t('memory:memoryPage.zoomIn')}
            onClick={() => setZoom((value) => Math.min(1.6, Number((value + 0.15).toFixed(2))))}
          >
            <ZoomIn size={13} />
          </button>
          <button
            title={t('memory:memoryPage.zoomOut')}
            onClick={() => setZoom((value) => Math.max(0.7, Number((value - 0.15).toFixed(2))))}
          >
            <ZoomOut size={13} />
          </button>
          <button title={t('memory:memoryPage.refresh')} onClick={() => load(spaceId)}>
            <RefreshCw className={loading ? 'animate-spin' : ''} size={13} />
          </button>
        </div>
        <div
          className={`galaxy-stage [&_>_svg]:absolute [&_>_svg]:inset-0 [&_>_svg]:w-full [&_>_svg]:h-full [&_>_svg]:overflow-visible [&_>_svg]:z-[1] [&_>_svg]:[transform:translate3d(calc(var(--px,0)_*_9px),calc(var(--py,0)_*_6px),0)] [&_>_svg]:[transition:transform_.9s_var(--ease-out)] absolute z-[1] inset-0 [transform-origin:50%_50%] [transition:transform_var(--d2)_var(--ease-out)] ${focusId ? 'has-focus' : ''}`}
          ref={stageRef}
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
                      stopColor={STAR_COLORS[source.node.type] || 'var(--galaxy-line-active)'}
                    />
                    <stop
                      offset="1"
                      stopColor={STAR_COLORS[target.node.type] || 'var(--galaxy-line-active)'}
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
              onClick={() => setSelectedId(node.id)}
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
          <span className="absolute z-[3] [right:12px] [bottom:9px] [border:1px_solid_var(--galaxy-toolbar-border)] rounded-[var(--r-pill)] bg-[rgba(12,11,20,.52)] [padding:3px_10px] text-[var(--galaxy-label)] text-[11px] tracking-[.5px] pointer-events-none [backdrop-filter:blur(6px)]">
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
      <div className="detail-stack flex min-w-0 flex-col gap-[12px] [.mcp-layout_>_&]:min-h-0 max-[1150px]:[.memory-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.memory-layout_>_&]:grid max-[1150px]:[.memory-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[1150px]:[.mcp-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.mcp-layout_>_&]:grid max-[1150px]:[.mcp-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[1150px]:[.skills-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.skills-layout_>_&]:grid max-[1150px]:[.skills-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[650px]:[.memory-layout_>_&]:[grid-column:auto] max-[650px]:[.memory-layout_>_&]:grid-cols-[1fr] max-[650px]:[.mcp-layout_>_&]:[grid-column:auto] max-[650px]:[.mcp-layout_>_&]:grid-cols-[1fr] max-[650px]:[.skills-layout_>_&]:[grid-column:auto] max-[650px]:[.skills-layout_>_&]:grid-cols-[1fr]">
        <Panel>
          <SectionTitle title={t('memory:memoryPage.selectedMemory')} />
          {selected ? (
            <>
              <h2>{selected.title}</h2>
              <p className="muted-copy m-[8px_0_14px] text-[var(--text-muted)] text-[12px] leading-[1.55]">
                {selected.content}
              </p>
              <div className="key-value [&:first-of-type]:mt-[7px] [&_span]:text-[var(--text-muted)] [&_button]:flex [&_button]:items-center [&_button]:gap-[4px] [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[var(--text-soft)] [&_button]:text-[12px] [&_strong]:min-w-0 [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap flex min-h-[31px] items-center justify-between gap-[10px] [border-top:1px_solid_var(--stroke-soft)] text-[13px]">
                <span>{t('memory:memoryPage.memoryType')}</span>
                <strong className="inline-flex items-center gap-[6px]">
                  <i
                    className={`g-dot [&.g-concept]:bg-[var(--g-concept)] [&.g-concept]:shadow-[0_0_5px_var(--g-concept-glow)] [&.g-file]:bg-[var(--g-file)] [&.g-file]:shadow-[0_0_5px_var(--g-file-glow)] [&.g-risk]:bg-[var(--g-risk)] [&.g-risk]:shadow-[0_0_5px_var(--g-risk-glow)] [&.g-preference]:bg-[var(--g-preference)] [&.g-preference]:shadow-[0_0_5px_var(--g-preference-glow)] [&.g-decision]:bg-[var(--g-decision)] [&.g-decision]:shadow-[0_0_5px_var(--g-decision-glow)] [&.g-fact]:bg-[var(--g-fact)] [&.g-fact]:shadow-[0_0_5px_var(--g-fact-glow)] [&.g-task]:bg-[var(--g-task)] [&.g-task]:shadow-[0_0_5px_var(--g-task-glow)] inline-block w-[7px] h-[7px] flex-none rounded-[50%] g-${selected.type}`}
                  />
                  {memoryTypeLabel(selected.type, t)}
                </strong>
              </div>
              <div className="key-value [&:first-of-type]:mt-[7px] [&_span]:text-[var(--text-muted)] [&_button]:flex [&_button]:items-center [&_button]:gap-[4px] [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[var(--text-soft)] [&_button]:text-[12px] [&_strong]:min-w-0 [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap flex min-h-[31px] items-center justify-between gap-[10px] [border-top:1px_solid_var(--stroke-soft)] text-[13px]">
                <span>{t('memory:memoryPage.source')}</span>
                <strong>
                  {selected.sourceType === 'conversation_confirmed'
                    ? t('memory:memoryPage.userConfirmedConversationCandidate')
                    : selected.sourceType === 'user_confirmed'
                      ? t('memory:memoryPage.explicitUserRequest')
                      : selected.sourceType === 'agent'
                        ? t('memory:memoryPage.createdByAgent')
                        : t('memory:memoryPage.addedManually')}
                </strong>
              </div>
              <div className="key-value [&:first-of-type]:mt-[7px] [&_span]:text-[var(--text-muted)] [&_button]:flex [&_button]:items-center [&_button]:gap-[4px] [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[var(--text-soft)] [&_button]:text-[12px] [&_strong]:min-w-0 [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap flex min-h-[31px] items-center justify-between gap-[10px] [border-top:1px_solid_var(--stroke-soft)] text-[13px]">
                <span>{t('memory:memoryPage.authority')}</span>
                <strong>{selected.authority ?? 0}/100</strong>
              </div>
              {selected.evidence && (
                <div className="key-value [&:first-of-type]:mt-[7px] [&_span]:text-[var(--text-muted)] [&_button]:flex [&_button]:items-center [&_button]:gap-[4px] [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[var(--text-soft)] [&_button]:text-[12px] [&_strong]:min-w-0 [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap flex min-h-[31px] items-center justify-between gap-[10px] [border-top:1px_solid_var(--stroke-soft)] text-[13px]">
                  <span>{t('memory:memoryPage.evidence')}</span>
                  <strong>{selected.evidence}</strong>
                </div>
              )}
              <div className="key-value [&:first-of-type]:mt-[7px] [&_span]:text-[var(--text-muted)] [&_button]:flex [&_button]:items-center [&_button]:gap-[4px] [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[var(--text-soft)] [&_button]:text-[12px] [&_strong]:min-w-0 [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap flex min-h-[31px] items-center justify-between gap-[10px] [border-top:1px_solid_var(--stroke-soft)] text-[13px]">
                <span>{t('memory:memoryPage.created')}</span>
                <strong>{formatMemoryTime(selected.createdAt, language)}</strong>
              </div>
              <div className="key-value [&:first-of-type]:mt-[7px] [&_span]:text-[var(--text-muted)] [&_button]:flex [&_button]:items-center [&_button]:gap-[4px] [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[var(--text-soft)] [&_button]:text-[12px] [&_strong]:min-w-0 [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap flex min-h-[31px] items-center justify-between gap-[10px] [border-top:1px_solid_var(--stroke-soft)] text-[13px]">
                <span>{t('memory:memoryPage.relatedMemories')}</span>
                <strong>{t('memory:memoryPage.count', { count: relatedNodeIds.size })}</strong>
              </div>
              <div className="mt-[15px] flex gap-2 max-[650px]:flex-wrap">
                <Button size="lg" onClick={() => setNodeModal(selected)}>
                  <Pencil size={14} />
                  {t('memory:memoryPage.edit')}
                </Button>
                <Button variant="destructive" size="lg" onClick={() => deleteNode(selected)}>
                  <Trash2 size={14} />
                  {t('memory:memoryPage.delete')}
                </Button>
              </div>
            </>
          ) : (
            <p className="muted-copy m-[8px_0_14px] text-[var(--text-muted)] text-[12px] leading-[1.55]">
              {t('memory:memoryPage.selectAMemoryInTheMapToViewItsDetails')}
            </p>
          )}
          {error && <AppError>{error}</AppError>}
        </Panel>
        <Panel>
          <SectionTitle title={t('memory:memoryPage.relatedFiles')} />
          {relatedFiles.map((file) => (
            <div
              className="file-row [&_span]:flex [&_span]:min-w-0 [&_span]:flex-col [&_span]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[var(--text-soft)] [&_button]:text-[13px] grid grid-cols-[auto_minmax(0,1fr)_auto_auto_auto] items-center gap-[6px] [border-top:1px_solid_var(--stroke-soft)] [padding:8px_0]"
              key={file.id}
            >
              <FileCode2 size={15} />
              <span>
                <strong>{file.title}</strong>
                <small>{file.sourcePath || file.cwd || t('memory:memoryPage.localMemory')}</small>
              </span>
              <button onClick={() => setSelectedId(file.id)}>{t('memory:memoryPage.view')}</button>
              <button onClick={() => setNodeModal(file)}>
                <Pencil size={13} />
              </button>
              <button className="danger" onClick={() => deleteNode(file)}>
                <Trash2 size={13} />
              </button>
            </div>
          ))}
          {!relatedFiles.length && (
            <p className="muted-copy m-[8px_0_14px] text-[var(--text-muted)] text-[12px] leading-[1.55]">
              {t('memory:memoryPage.thisMemoryHasNoRelatedFiles')}
            </p>
          )}
        </Panel>
      </div>
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
    </div>
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
      className="modal-backdrop max-[650px]:p-[8px] fixed z-[70] inset-0 grid place-items-center overflow-y-auto bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)] [padding:20px] [overscroll-behavior:contain] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form
        className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)]"
        onSubmit={submit}
      >
        <AppCardHeader>
          <div>
            <h2>{node ? t('memory:memoryPage.editMemory') : t('memory:memoryPage.addMemory')}</h2>
            <p>{t('memory:memoryPage.lightUpIdeasWorthKeepingAsMemoriesYouCanReturnToLater')}</p>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={t('memory:memoryPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
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
        <div className="form-grid grid gap-[9px]">
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
        <div className="flex justify-end gap-[8px] [margin-top:18px]">
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            onClick={onClose}
          >
            {t('memory:memoryPage.cancel')}
          </Button>
          <Button
            size="lg"
            disabled={saving || !draft.spaceId || !draft.title.trim() || !draft.content.trim()}
          >
            {saving ? <RefreshCw className="animate-spin" size={14} /> : <Pencil size={14} />}
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
      className="modal-backdrop max-[650px]:p-[8px] fixed z-[70] inset-0 grid place-items-center overflow-y-auto bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)] [padding:20px] [overscroll-behavior:contain] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form
        className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)]"
        onSubmit={submit}
      >
        <AppCardHeader>
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
            aria-label={t('memory:memoryPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
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
        <div className="flex justify-end gap-[8px] [margin-top:18px]">
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            onClick={onClose}
          >
            {t('memory:memoryPage.cancel')}
          </Button>
          <Button size="lg" disabled={saving || !name.trim()}>
            {saving ? <RefreshCw className="animate-spin" size={14} /> : <Plus size={14} />}
            {saving ? t('memory:memoryPage.saving') : t('memory:memoryPage.save')}
          </Button>
        </div>
      </form>
    </div>
  )
}
