// 星忆星图（规格 v2 2.7、10 A6：星忆保留星图）：星忆页里“星图”视图的深空舞台。
// 黄金角螺旋布局（越重要越靠近核心，最多 24 颗），关联线、悬停聚焦、视差与缩放；
// 星图始终是深空场景，不随主题翻转（负责人保留的 Memory glow），离屏或页面隐藏时暂停全部装饰动画。
// 从 MemoryPage 拆出：页面持有数据、选中与弹窗，这里只持有缩放、悬停与视差这些纯展示状态。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { CSSProperties, PointerEvent as ReactPointerEvent } from 'react'
import { Plus, ZoomIn, ZoomOut } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { StarOrbit } from '@/components/StarOrbit'
import { AppCard as Panel } from '@/components/ui/app-primitives'
import { useAnimationVisibility } from '@/hooks/use-animation-visibility'
import type { MemoryLink, MemoryNode, MemoryType } from './memory-types'

type GalaxyStar = {
  node: MemoryNode
  x: number
  y: number
  twinkle: number
}
type GalaxyPoint = Pick<GalaxyStar, 'x' | 'y'>
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

// 星图类型色点：沿用星系配色 token，外加 1px 细环保证浅色底上的浅色类型可见。
export function GalaxyDot({ type }: { type: MemoryType }) {
  return (
    <i
      aria-hidden="true"
      className={`g-dot g-${type} inline-block size-2 shrink-0 rounded-full ring-1 ring-stroke`}
      style={{ background: STAR_COLORS[type] }}
    />
  )
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

export function MemoryGalaxy({
  nodes,
  links,
  spaceId,
  selectedId,
  loading,
  onSelect,
  onAdd,
}: {
  nodes: MemoryNode[]
  links: MemoryLink[]
  spaceId: string
  selectedId: string
  loading: boolean
  onSelect: (id: string) => void
  onAdd: () => void
}) {
  const { t } = useI18n()
  const [zoom, setZoom] = useState(1)
  const [hoveredId, setHoveredId] = useState('')
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

  const stars = useMemo(() => galaxyLayout(nodes, spaceId), [nodes, spaceId])
  const starById = useMemo(() => new Map(stars.map((star) => [star.node.id, star])), [stars])
  const visibleLinks = useMemo(() => {
    const visibleIds = new Set(stars.map((star) => star.node.id))
    return links.filter((link) => visibleIds.has(link.sourceId) && visibleIds.has(link.targetId))
  }, [stars, links])
  // 聚焦模式：悬停优先于选中，聚焦星的关系网保持明亮，其余星辰淡出
  const focusId = hoveredId || selectedId
  const focusRelatedIds = new Set(
    links.flatMap((link) => {
      if (link.sourceId === focusId) return [link.targetId]
      if (link.targetId === focusId) return [link.sourceId]
      return []
    }),
  )

  return (
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
          onClick={onAdd}
        >
          <Plus />
        </button>
        <button
          type="button"
          title={t('memory:memoryPage.zoomIn')}
          aria-label={t('memory:memoryPage.zoomIn')}
          onClick={() => setZoom((value) => Math.min(1.6, Number((value + 0.15).toFixed(2))))}
        >
          <ZoomIn />
        </button>
        <button
          type="button"
          title={t('memory:memoryPage.zoomOut')}
          aria-label={t('memory:memoryPage.zoomOut')}
          onClick={() => setZoom((value) => Math.max(0.7, Number((value - 0.15).toFixed(2))))}
        >
          <ZoomOut />
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
            aria-pressed={selectedId === node.id}
            onClick={() => onSelect(node.id)}
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
            total: nodes.length,
          })}
        </span>
      )}
      {!loading && !stars.length && (
        <div className="memory-empty [.galaxy-panel_&]:text-[var(--galaxy-label)] [.galaxy-panel_&]:[--stroke:var(--galaxy-toolbar-border)] [.galaxy-panel_&]:[--control-muted:var(--galaxy-label)] [.galaxy-panel_&]:[--star:var(--g-decision)] [.galaxy-panel_&]:[--text-muted:var(--galaxy-label)] absolute z-[3] inset-0 grid [align-content:center] justify-items-center gap-[8px] text-[var(--text-muted)] text-[12px] pointer-events-none">
          <StarOrbit size={44} />
          <span>
            {t('memory:memoryPage.thisMemorySpaceIsStillUnlitClickInTheTopLeftToPlantTheFirstStar')}
          </span>
        </div>
      )}
    </Panel>
  )
}
