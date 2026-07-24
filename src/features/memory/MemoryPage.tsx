import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Check,
  ChevronDown,
  ChevronRight,
  FileCode2,
  Pencil,
  Plus,
  RefreshCw,
  Trash2,
  X,
  ZoomIn,
  ZoomOut,
} from 'lucide-react'
import { Panel, SectionTitle } from '../../components/ui'
import { AppSelect } from '../../components/AppSelect'
import { useI18n } from '../../app/use-i18n'
import { StarOrbit } from '../../components/StarOrbit'
import { apiJson } from '../../lib/api'
import { usePagePrimaryAction } from '../../hooks/usePagePrimaryAction'
import type { CSSProperties, FormEvent, PointerEvent as ReactPointerEvent } from 'react'
import type { Notify } from '../../app/route-context'
import type { I18nValues } from '../../app/i18n'
import type { ConfirmDialogOptions } from '../../hooks/useAppDialog'

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

// 与 index.css 中的 --g-* 星辰色保持一致，用于连线渐变
const STAR_COLORS: Record<MemoryType, string> = {
  concept: '#6eb5ff',
  file: '#4ade80',
  risk: '#fb7185',
  preference: '#c4b5fd',
  decision: '#fbbf24',
  fact: '#e2e8f0',
  task: '#67e8f9',
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
    <div className="memory-layout">
      <div className="memory-left-stack">
        <Panel className="wiki-panel">
          <div className="card-head">
            <SectionTitle title={t('memory:memoryPage.memorySpaces')} />
            <button
              className="icon-button"
              title={t('memory:memoryPage.newMemorySpace')}
              onClick={() => setSpaceModal({})}
            >
              <Plus size={13} />
            </button>
          </div>
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
            <div className="memory-space-actions">
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
          <div className="galaxy-legend">
            {MEMORY_TYPES.map((type) => (
              <span key={type}>
                <i className={`g-dot g-${type}`} />
                {memoryTypeLabel(type, t)}
              </span>
            ))}
          </div>
        </Panel>
        <Panel className="memory-candidates-panel">
          <div className="card-head">
            <SectionTitle
              title={`${t('memory:memoryPage.memoryDrafts')} · ${data.candidates?.length || 0}`}
            />
            {Boolean(data.candidates?.length) && (
              <button
                className="button secondary tiny"
                disabled={Boolean(resolvingCandidateId)}
                onClick={ignoreAllCandidates}
              >
                <X size={12} />
                {t('memory:memoryPage.ignoreAll')}
              </button>
            )}
          </div>
          <div className="memory-candidate-list">
            {(data.candidates || []).map((candidate) => (
              <div className="memory-candidate" key={candidate.id}>
                <strong>{candidate.title}</strong>
                <small>
                  {spaceLabel(
                    data.spaces.find((space) => space.id === candidate.spaceId),
                    t,
                  )}
                </small>
                <span>{candidate.content}</span>
                {candidate.evidence && (
                  <small>
                    {t('memory:memoryPage.evidence')}：{candidate.evidence}
                  </small>
                )}
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
        className="graph-panel galaxy-panel"
        onPointerMove={handleParallax}
        onPointerLeave={resetParallax}
      >
        <div className="graph-toolbar">
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
            <RefreshCw className={loading ? 'spin' : ''} size={13} />
          </button>
        </div>
        <div
          className={`galaxy-stage ${focusId ? 'has-focus' : ''}`}
          ref={stageRef}
          style={{ transform: `scale(${zoom})` }}
        >
          <i className="galaxy-spiral" aria-hidden="true" />
          <i className="galaxy-core" aria-hidden="true" />
          <i className="galaxy-aurora galaxy-aurora-one" aria-hidden="true" />
          <i className="galaxy-aurora galaxy-aurora-two" aria-hidden="true" />
          <i className="galaxy-aurora galaxy-aurora-three" aria-hidden="true" />
          <svg viewBox="0 0 600 420" preserveAspectRatio="none" aria-hidden="true">
            <g className="galaxy-orbits">
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
                    <stop offset="0" stopColor={STAR_COLORS[source.node.type] || '#93b4ff'} />
                    <stop offset="1" stopColor={STAR_COLORS[target.node.type] || '#93b4ff'} />
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
                    className={`link-path ${active ? 'active' : ''}`}
                    d={path}
                    stroke={`url(#lg-${index})`}
                  />
                  {active && (
                    <circle className="link-pulse" r="2.1">
                      <animateMotion dur="2.8s" repeatCount="indefinite" path={path} />
                    </circle>
                  )}
                </g>
              )
            })}
          </svg>
          <i className="galaxy-meteor" aria-hidden="true" />
          <i className="galaxy-meteor meteor-two" aria-hidden="true" />
          <i className="galaxy-meteor meteor-three" aria-hidden="true" />
          {stars.map(({ node, x, y, twinkle }, index) => (
            <button
              className={`galaxy-star star-${node.type} ${selectedId === node.id ? 'active' : ''} ${focusId && node.id !== focusId && !focusRelatedIds.has(node.id) ? 'dimmed' : ''}`}
              onClick={() => setSelectedId(node.id)}
              onMouseEnter={() => setHoveredId(node.id)}
              onMouseLeave={() => setHoveredId('')}
              style={
                {
                  left: `${(x / GALAXY_VIEW.width) * 100}%`,
                  top: `${(y / GALAXY_VIEW.height) * 100}%`,
                  '--star-size': `${10 + Math.round((node.importance || 0.5) * 9)}px`,
                  '--twinkle-delay': `${twinkle}s`,
                  '--enter-delay': `${index * 55}ms`,
                  '--depth': (hashSeed(node.id) % 100) / 100,
                } as GalaxyStarStyle
              }
              title={node.content}
              key={node.id}
            >
              {selectedId === node.id && <i className="orbit-ring" aria-hidden="true" />}
              {selectedId === node.id && <i className="dash-ring" aria-hidden="true" />}
              <svg className="star-core" viewBox="0 0 32 32" aria-hidden="true">
                <path
                  className="star-ray"
                  d="M16 0 L19.7 12.3 L32 16 L19.7 19.7 L16 32 L12.3 19.7 L0 16 L12.3 12.3 Z"
                />
                <path
                  className="star-shape"
                  d="M16 2 L18.8 13.2 L30 16 L18.8 18.8 L16 30 L13.2 18.8 L2 16 L13.2 13.2 Z"
                />
                <circle className="star-heart" cx="16" cy="16" r="3.2" />
              </svg>
              <span className="star-label">{node.title}</span>
            </button>
          ))}
        </div>
        {!!stars.length && (
          <span className="galaxy-count">
            ✦{' '}
            {t('memory:memoryPage.visibleTotalMemories', {
              visible: stars.length,
              total: data.nodes.length,
            })}
          </span>
        )}
        {!loading && !stars.length && (
          <div className="memory-empty">
            <StarOrbit size={44} />
            <span>
              {t(
                'memory:memoryPage.thisMemorySpaceIsStillUnlitClickInTheTopLeftToPlantTheFirstStar',
              )}
            </span>
          </div>
        )}
      </Panel>
      <div className="detail-stack">
        <Panel>
          <SectionTitle title={t('memory:memoryPage.selectedMemory')} />
          {selected ? (
            <>
              <h2>{selected.title}</h2>
              <p className="muted-copy">{selected.content}</p>
              <div className="key-value">
                <span>{t('memory:memoryPage.memoryType')}</span>
                <strong className="type-with-dot">
                  <i className={`g-dot g-${selected.type}`} />
                  {memoryTypeLabel(selected.type, t)}
                </strong>
              </div>
              <div className="key-value">
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
              <div className="key-value">
                <span>{t('memory:memoryPage.authority')}</span>
                <strong>{selected.authority ?? 0}/100</strong>
              </div>
              {selected.evidence && (
                <div className="key-value">
                  <span>{t('memory:memoryPage.evidence')}</span>
                  <strong>{selected.evidence}</strong>
                </div>
              )}
              <div className="key-value">
                <span>{t('memory:memoryPage.created')}</span>
                <strong>{formatMemoryTime(selected.createdAt, language)}</strong>
              </div>
              <div className="key-value">
                <span>{t('memory:memoryPage.relatedMemories')}</span>
                <strong>{t('memory:memoryPage.count', { count: relatedNodeIds.size })}</strong>
              </div>
              <div className="button-row">
                <button className="button primary" onClick={() => setNodeModal(selected)}>
                  <Pencil size={14} />
                  {t('memory:memoryPage.edit')}
                </button>
                <button className="button danger" onClick={() => deleteNode(selected)}>
                  <Trash2 size={14} />
                  {t('memory:memoryPage.delete')}
                </button>
              </div>
            </>
          ) : (
            <p className="muted-copy">
              {t('memory:memoryPage.selectAMemoryInTheMapToViewItsDetails')}
            </p>
          )}
          {error && <div className="config-error">{error}</div>}
        </Panel>
        <Panel>
          <SectionTitle title={t('memory:memoryPage.relatedFiles')} />
          {relatedFiles.map((file) => (
            <div className="file-row" key={file.id}>
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
            <p className="muted-copy">{t('memory:memoryPage.thisMemoryHasNoRelatedFiles')}</p>
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
      className="modal-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form className="modal" onSubmit={submit}>
        <div className="card-head">
          <div>
            <h2>{node ? t('memory:memoryPage.editMemory') : t('memory:memoryPage.addMemory')}</h2>
            <p>{t('memory:memoryPage.lightUpIdeasWorthKeepingAsMemoriesYouCanReturnToLater')}</p>
          </div>
          <button
            type="button"
            className="icon-button"
            aria-label={t('memory:memoryPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </div>
        <label className="field-label">
          {t('memory:memoryPage.memorySpace')}
          <span className="select-wrap">
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
            <ChevronDown size={13} />
          </span>
        </label>
        <label className="field-label">
          {t('memory:memoryPage.memoryTitle')}
          <input
            value={draft.title}
            onChange={(event) => setDraft({ ...draft, title: event.target.value })}
            placeholder={t('memory:memoryPage.forExampleProjectUIConstraints')}
          />
        </label>
        <label className="field-label">
          {t('memory:memoryPage.memoryContent')}
          <textarea
            value={draft.content}
            onChange={(event) => setDraft({ ...draft, content: event.target.value })}
            placeholder={t('memory:memoryPage.recordStandaloneReusableMemoryForFutureChats')}
          />
        </label>
        <div className="form-grid">
          <label className="field-label">
            {t('memory:memoryPage.memoryType')}
            <span className="select-wrap">
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
              <ChevronDown size={13} />
            </span>
          </label>
          <label className="field-label">
            {t('memory:memoryPage.importance')}
            <span className="select-wrap">
              <AppSelect
                value={draft.importance}
                onChange={(event) => setDraft({ ...draft, importance: Number(event.target.value) })}
              >
                <option value="0.3">{t('memory:memoryPage.normal')}</option>
                <option value="0.5">{t('memory:memoryPage.common')}</option>
                <option value="0.8">{t('memory:memoryPage.important')}</option>
                <option value="1">{t('memory:memoryPage.strict')}</option>
              </AppSelect>
              <ChevronDown size={13} />
            </span>
          </label>
        </div>
        <label className="field-label">
          {t('memory:memoryPage.relatedFilePath')}
          <input
            value={draft.sourcePath}
            onChange={(event) => setDraft({ ...draft, sourcePath: event.target.value })}
            placeholder={t('memory:memoryPage.optionalForExampleECodeProjectREADMEMd')}
          />
        </label>
        {error && <div className="config-error">{error}</div>}
        <div className="modal-actions">
          <button type="button" className="button secondary" onClick={onClose}>
            {t('memory:memoryPage.cancel')}
          </button>
          <button
            className="button primary"
            disabled={saving || !draft.spaceId || !draft.title.trim() || !draft.content.trim()}
          >
            {saving ? <RefreshCw className="spin" size={14} /> : <Pencil size={14} />}
            {saving ? t('memory:memoryPage.saving') : t('memory:memoryPage.save')}
          </button>
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
      className="modal-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form className="modal" onSubmit={submit}>
        <div className="card-head">
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
          <button
            type="button"
            className="icon-button"
            aria-label={t('memory:memoryPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </div>
        <label className="field-label">
          {t('memory:memoryPage.memorySpaceName')}
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={t('memory:memoryPage.forExampleProductDesignGuidelines')}
            autoFocus
          />
        </label>
        {error && <div className="config-error">{error}</div>}
        <div className="modal-actions">
          <button type="button" className="button secondary" onClick={onClose}>
            {t('memory:memoryPage.cancel')}
          </button>
          <button className="button primary" disabled={saving || !name.trim()}>
            {saving ? <RefreshCw className="spin" size={14} /> : <Plus size={14} />}
            {saving ? t('memory:memoryPage.saving') : t('memory:memoryPage.save')}
          </button>
        </div>
      </form>
    </div>
  )
}
