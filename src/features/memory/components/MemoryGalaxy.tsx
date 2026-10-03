// 记忆星系可视化：星点、连线和视差效果的纯渲染组件。
import { useMemo } from 'react'
import type { CSSProperties, PointerEvent as ReactPointerEvent, RefObject } from 'react'
import { galaxyLayout, hashSeed, linkCurve, starColor, starGlow } from '@/features/memory/model/memory-layout'
import { GALAXY_VIEW } from '@/features/memory/model/memory-galaxy-constants'
import { formatMemoryTime, memoryTypeLabel } from '@/features/memory/model/memory-utils'
import type { GalaxyStar, MemoryLink, MemoryNode } from '@/features/memory/model/memory-types'

type MemoryGalaxyProps = {
  nodes: MemoryNode[]
  links: MemoryLink[]
  spaceId: string
  selectedId: string
  zoom: number
  playing: boolean
  pauseClass: string
  stageRef: RefObject<HTMLDivElement | null>
  attachStage: (node: HTMLDivElement | null) => void
  onParallax: (event: ReactPointerEvent<HTMLDivElement>) => void
  onParallaxReset: () => void
  onSelect: (id: string) => void
  onEdit: (node: MemoryNode) => void
}

export function MemoryGalaxy({
  nodes,
  links,
  spaceId,
  selectedId,
  zoom,
  playing,
  pauseClass,
  stageRef: _stageRef,
  attachStage,
  onParallax,
  onParallaxReset,
  onSelect,
  onEdit,
}: MemoryGalaxyProps) {
  const stars = useMemo(() => galaxyLayout(nodes, spaceId), [nodes, spaceId])
  const starById = useMemo(() => new Map(stars.map((star) => [star.node.id, star])), [stars])
  const visibleLinks = useMemo(() => {
    const visibleIds = new Set(stars.map((star) => star.node.id))
    return links.filter((link) => visibleIds.has(link.sourceId) && visibleIds.has(link.targetId))
  }, [links, stars])

  return (
    <div
      ref={attachStage}
      className={`memory-galaxy-stage graph-panel galaxy-panel bg-[var(--galaxy-bg)]! ${pauseClass}`}
      onPointerMove={playing ? onParallax : undefined}
      onPointerLeave={onParallaxReset}
      style={{
        '--galaxy-zoom': zoom,
      } as CSSProperties}
    >
      <svg
        viewBox={`0 0 ${GALAXY_VIEW.width} ${GALAXY_VIEW.height}`}
        className="memory-galaxy-svg"
        aria-hidden
      >
        <defs>
          {stars.map((star) => (
            <radialGradient key={`glow-${star.node.id}`} id={`star-glow-${star.node.id}`}>
              <stop offset="0%" stopColor={starGlow(star.node.type)} stopOpacity="0.7" />
              <stop offset="100%" stopColor={starGlow(star.node.type)} stopOpacity="0" />
            </radialGradient>
          ))}
        </defs>
        {visibleLinks.map((link) => {
          const source = starById.get(link.sourceId)
          const target = starById.get(link.targetId)
          if (!source || !target) return null
          const seed = hashSeed(link.id)
          return (
            <path
              key={link.id}
              d={linkCurve(source, target, seed)}
              fill="none"
              stroke="var(--g-link)"
              strokeOpacity={selectedId === link.sourceId || selectedId === link.targetId ? 0.5 : 0.15}
              strokeWidth="0.8"
            />
          )
        })}
        {stars.map((star) => (
          <MemoryStar
            key={star.node.id}
            star={star}
            selected={selectedId === star.node.id}
            onSelect={onSelect}
            onEdit={onEdit}
          />
        ))}
      </svg>
    </div>
  )
}

function MemoryStar({
  star,
  selected,
  onSelect,
  onEdit,
}: {
  star: GalaxyStar
  selected: boolean
  onSelect: (id: string) => void
  onEdit: (node: MemoryNode) => void
}) {
  const { node } = star
  const style = {
    '--star-size': selected ? '9' : '6',
    '--twinkle-delay': `${(hashSeed(node.id) % 30) / 10}s`,
    '--g-star-color': starColor(node.type),
    '--g-star-glow': starGlow(node.type),
  } as CSSProperties
  return (
    <g
      className={`memory-star galaxy-star ${selected ? 'selected active' : ''}`}
      style={style}
      transform={`translate(${star.x} ${star.y})`}
      onClick={() => onSelect(node.id)}
      onDoubleClick={() => onEdit(node)}
      role="button"
      tabIndex={0}
      onKeyDown={(event) => {
        if (event.key === 'Enter') onSelect(node.id)
        if (event.key === 'Escape') onEdit(node)
      }}
      aria-label={memoryTypeLabel(node.type, (key) => key)}
    >
      <circle r="14" fill={`url(#star-glow-${node.id})`} opacity="0.6" />
      <circle
        r={selected ? 5 : 3.5}
        fill={starColor(node.type)}
        className="memory-star-dot"
      />
      {selected && (
        <circle r="8" fill="none" stroke={starColor(node.type)} strokeWidth="0.8" opacity="0.4">
          <animate attributeName="r" values="7;12;7" dur="2s" repeatCount="indefinite" />
          <animate
            attributeName="opacity"
            values="0.4;0;0.4"
            dur="2s"
            repeatCount="indefinite"
          />
        </circle>
      )}
      <title>
        {node.title} · {formatMemoryTime(node.createdAt)}
      </title>
    </g>
  )
}
