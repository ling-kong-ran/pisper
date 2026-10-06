// 记忆页：星系可视化管理记忆节点、空间和候选条目。
// 仅负责状态编排和数据请求，星系渲染委托给 MemoryGalaxy。
import { useCallback, useEffect, useRef, useState } from 'react'
import type { PointerEvent as ReactPointerEvent } from 'react'
import { ChevronRight, Pencil, Plus, RefreshCw, Trash2 } from 'lucide-react'
import {
  AppCard as Panel,
  AppSectionTitle as SectionTitle,
  AppError,
  AppEmptyState,
} from '@/components/ui/app-primitives'
import { useI18n } from '@/app/i18n/use-i18n'
import { apiJson } from '@/lib/http/api'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { useAnimationVisibility } from '@/hooks/use-animation-visibility'
import { Button } from '@/components/ui/button'
import { MemoryGalaxy } from '@/features/memory/components/MemoryGalaxy'
import { MemoryNodeModal } from '@/features/memory/components/MemoryNodeModal'
import { MemorySpaceModal } from '@/features/memory/components/MemorySpaceModal'
import { MEMORY_TYPES } from '@/features/memory/model/memory-galaxy-constants'
import { memoryTypeLabel, spaceLabel } from '@/features/memory/model/memory-utils'
import type { MemoryData, MemoryNode, MemorySpace } from '@/features/memory/model/memory-types'
import type { Notify } from '@/app/routes/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

type MemoryNodeModalState = Partial<MemoryNode> & { spaceId: string }
type MemoryPageProps = {
  notify: Notify
  query?: string
  registerPrimaryAction: (action: () => void) => () => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
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
  const { t } = useI18n()
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
  const [zoom] = useState(1)
  const [nodeModal, setNodeModal] = useState<MemoryNodeModalState | null>(null)
  const [spaceModal, setSpaceModal] = useState<Partial<MemorySpace> | null>(null)
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
  // 不可见时暂停面板内所有 CSS 无限动画
  const galaxyPauseClass = galaxyPlaying
    ? ''
    : '[&:before]:[animation-play-state:paused]! [&:after]:[animation-play-state:paused]! [&_*]:[animation-play-state:paused]! [&_*:before]:[animation-play-state:paused]! [&_*:after]:[animation-play-state:paused]!'
  usePagePrimaryAction(registerPrimaryAction, () =>
    setNodeModal({ spaceId: spaceId || data.selectedSpaceId }),
  )

  // 鼠标视差：星辰与连线按深度分层缓动跟随，rAF 节流避免高频写入
  const handleParallax = (event: ReactPointerEvent<HTMLDivElement>) => {
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

  const selected = data.nodes.find((node) => node.id === selectedId) || null
  const selectedSpace = data.spaces.find((space) => space.id === spaceId) || null

  // 删除记忆节点（确认后刷新列表）。
  const deleteNode = async () => {
    if (!selected) return
    const approved = await requestConfirm({
      title: t('memory:memoryPage.deleteMemory'),
      message: t('memory:memoryPage.deleteMemoryTitle', { name: selected.title }),
      confirmLabel: t('memory:memoryPage.delete'),
    })
    if (!approved) return
    try {
      await apiJson(`/api/memory/nodes/${encodeURIComponent(selected.id)}`, { method: 'DELETE' })
      await load(spaceId)
      notify(t('memory:memoryPage.memoryDeleted'))
    } catch (deleteError) {
      setError(errorMessage(deleteError))
    }
  }

  // 删除记忆空间（确认后刷新列表）。
  const deleteSpace = async () => {
    if (!selectedSpace || selectedSpace.kind === 'global') return
    const approved = await requestConfirm({
      title: t('memory:memoryPage.deleteMemorySpace'),
      message: t('memory:memoryPage.deleteMemorySpaceName', { name: selectedSpace.name }),
      confirmLabel: t('memory:memoryPage.delete'),
    })
    if (!approved) return
    try {
      await apiJson(`/api/memory/spaces/${encodeURIComponent(selectedSpace.id)}`, {
        method: 'DELETE',
      })
      await load('')
      notify(t('memory:memoryPage.memorySpaceDeleted'))
    } catch (deleteError) {
      setError(errorMessage(deleteError))
    }
  }

  if (loading && !data.nodes.length)
    return (
      <AppEmptyState>
        <RefreshCw className="animate-spin" size={23} />
        <h2>{t('memory:memoryPage.loadingMemories')}</h2>
      </AppEmptyState>
    )

  return (
    <>
      {error && <AppError>{error}</AppError>}
      <div className="memory-layout grid min-h-[100%] min-w-0 grid-cols-[repeat(4,minmax(0,1fr))] gap-[12px] overflow-x-hidden max-[1150px]:grid-cols-[repeat(2,minmax(0,1fr))] max-[650px]:grid-cols-[1fr]">
        <Panel className="memory-spaces-panel">
          <SectionTitle title={t('memory:memoryPage.memorySpaces')} />
          {data.spaces.map((space) => (
            <button
              className={`memory-space-item ${spaceId === space.id ? 'active' : ''}`}
              key={space.id}
              onClick={() => setSpaceId(space.id)}
            >
              <span>{spaceLabel(space, t)}</span>
              <small>{t('memory:memoryPage.countMemories', { count: space.nodeCount })}</small>
              <ChevronRight size={13} />
            </button>
          ))}
          <Button
            variant="outline"
            className="mt-[10px] w-full bg-surface-subtle"
            onClick={() => setSpaceModal({})}
          >
            <Plus size={13} />
            {t('memory:memoryPage.newMemorySpace')}
          </Button>
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
        <MemoryGalaxy
          nodes={data.nodes}
          links={data.links}
          spaceId={spaceId}
          selectedId={selectedId}
          zoom={zoom}
          playing={galaxyPlaying}
          pauseClass={galaxyPauseClass}
          stageRef={stageRef}
          attachStage={attachGalaxyStage}
          onParallax={handleParallax}
          onParallaxReset={resetParallax}
          onSelect={setSelectedId}
          onEdit={(node) => setNodeModal(node)}
        />
        <Panel className="min-h-0">
          <SectionTitle title={t('memory:memoryPage.memoryDrafts')} />
          {selected && (
            <div className="memory-node-detail [&_strong]:text-[13px] [&_p]:text-[var(--text-muted)] [&_p]:text-[12px] [&_p]:leading-[1.5] [padding:8px_0]">
              <strong>{selected.title}</strong>
              <p>{selected.content}</p>
              <div className="flex gap-[6px] [margin-top:8px]">
                <Button size="sm" variant="outline" onClick={() => setNodeModal(selected)}>
                  <Pencil size={12} />
                  {t('memory:memoryPage.edit')}
                </Button>
                <Button size="sm" variant="destructive" onClick={deleteNode}>
                  <Trash2 size={12} />
                  {t('memory:memoryPage.delete')}
                </Button>
              </div>
            </div>
          )}
        </Panel>
      </div>
      {nodeModal && (
        <MemoryNodeModal
          spaces={data.spaces}
          node={data.nodes.find((node) => node.id === nodeModal.id) || null}
          initialSpaceId={nodeModal.spaceId || spaceId}
          onClose={() => setNodeModal(null)}
          onSaved={async (message) => {
            setNodeModal(null)
            await load(spaceId)
            notify(message)
          }}
        />
      )}
      {spaceModal && (
        <MemorySpaceModal
          space={data.spaces.find((space) => space.id === spaceModal.id) || null}
          onClose={() => setSpaceModal(null)}
          onSaved={async (space, message) => {
            setSpaceModal(null)
            await load(spaceId || space.id)
            notify(message)
          }}
        />
      )}
    </>
  )
}
