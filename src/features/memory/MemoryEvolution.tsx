// 记忆年轮：按时间滑动演算记忆的生长、替换与沉淀。时间由内向外长成一圈圈年轮，最外圈就是当前演算到的时刻。
// 算状态在 memory-evolution-model.ts，画面在 memory-evolution-renderer.ts；这里负责取数、时间轴、播放和叠在舞台上的界面。
// 时间轴走动的权威值在 ref 里：渲染每帧读它，React 状态只以约 30 次/秒同步给滑块和读数，拖动和播放不会卡在重渲染上。
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { CSSProperties, KeyboardEvent, PointerEvent } from 'react'
import { Info, Pause, Play, SkipForward, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { Slider } from '@/components/ui/slider'
import { OrbitSpinner } from '@/components/ui/spinner'
import { useAnimationVisibility } from '@/hooks/use-animation-visibility'
import { motionReduced } from '@/lib/motion'
import { cn } from '@/lib/utils'
import { fetchMemoryDetail, fetchMemoryEvolution } from './memory-api'
import {
  buildEvolutionScene,
  EVOLUTION_TYPES,
  FLAG_LATE,
  FLAG_PINNED,
  FLAG_STALE,
  FLAG_TAINTED,
  factStateNameAt,
  OPENING_DURATION_MS,
  openingProgress,
  sliderFromTime,
  SLIDER_MAX,
  statsAt,
  timeFromSlider,
  versionChainOf,
  type EvolutionScene,
  type FactStateName,
} from './memory-evolution-model'
import {
  ARCHIVE,
  drawEvolutionHistogram,
  EMBER,
  EvolutionRenderer,
  QUALITY_HIGH,
  QUALITY_LOW,
  QUALITY_MEDIUM,
  rgbCss,
  TYPE_COLORS,
} from './memory-evolution-renderer'
import type { MemoryDetail, MemoryType } from './memory-types'

export type MemoryEvolutionProps = {
  // 'all' 表示全部空间。
  spaceId: string
  // 记忆变了（新增、删除、撤销）时加一，重新取数。
  refreshKey?: number
  selectedId?: string | null
  onSelect: (id: string | null) => void
}

// 舞台是恒定的深色场景，亮色主题下也一样：把主题 token 就地改成深色值，里面的基础控件不用逐个覆盖样式。
const STAGE_MIN_HEIGHT = 400
const STAGE_MIN_HEIGHT_NARROW = 540
const STAGE_MAX_HEIGHT = 880
const STAGE_BOTTOM_GAP = 16

const STAGE_TOKENS = {
  colorScheme: 'dark',
  '--text': 'rgba(246, 240, 228, 0.96)',
  '--foreground': 'rgba(246, 240, 228, 0.96)',
  '--text-soft': 'rgba(246, 240, 228, 0.86)',
  '--text-secondary': 'rgba(240, 232, 218, 0.74)',
  '--text-muted': 'rgba(232, 224, 208, 0.58)',
  '--text-faint': 'rgba(232, 224, 208, 0.38)',
  '--surface-hover': 'rgba(255, 244, 224, 0.1)',
  '--surface-active': 'rgba(255, 244, 224, 0.17)',
  '--surface-muted': 'rgba(255, 244, 224, 0.07)',
  '--surface-subtle': 'rgba(255, 244, 224, 0.1)',
  '--stroke': 'rgba(255, 244, 224, 0.13)',
  '--stroke-soft': 'rgba(255, 244, 224, 0.08)',
  '--stroke-strong': 'rgba(255, 244, 224, 0.22)',
  '--solid': 'rgba(255, 244, 224, 0.14)',
  '--star': 'rgba(255, 232, 192, 0.92)',
  '--star-hover': 'rgba(255, 240, 214, 1)',
  '--key-bg': 'rgba(255, 240, 214, 1)',
  '--key-border': 'rgba(255, 244, 224, 0.2)',
  '--key-shadow': '0 0 14px rgba(255, 214, 150, 0.55)',
  '--control-border': 'rgba(255, 240, 214, 0.9)',
  '--focus': 'rgba(255, 222, 170, 1)',
  '--focus-ring': 'rgba(255, 222, 170, 0.32)',
  '--danger': 'rgba(255, 150, 130, 1)',
  '--evo-panel': 'rgba(13, 14, 19, 0.74)',
  '--evo-line': 'rgba(255, 244, 224, 0.13)',
  '--evo-scrim': 'rgba(5, 6, 10, 0.86)',
  '--evo-scrim-clear': 'rgba(5, 6, 10, 0)',
  '--evo-base': 'rgba(5, 6, 10, 1)',
  '--evo-active': 'rgba(255, 232, 192, 1)',
} as CSSProperties

const EMBER_DOT = rgbCss(EMBER)
const ARCHIVE_DOT = rgbCss(ARCHIVE)
const PLAY_BASE_MS = 24_000
const SPEEDS = ['0.5', '1', '2'] as const
type Speed = (typeof SPEEDS)[number]
const DEFAULT_YAW = -0.38
const WIDE_TILT = 0.94
const NARROW_TILT = 0.68
const OPENING_START_TILT = 0.24
const ROTATION_PER_MS = 0.000026
const FADE_MS = 450
const IDLE_AFTER_MS = 20_000
// 选中时把这条记忆转到靠近观众的一侧，略微推近。
const SELECTED_SCREEN_ANGLE = Math.PI * 0.62
const SELECTED_ZOOM = 1.1
const HISTOGRAM_HEIGHT = 30
// 滑块的中心在轨道两端各内缩半个滑块，密度图要对齐它。
const THUMB_INSET = 7

// 开场只在这次打开应用时放一次：从别的视图切回来不重播。
let openingPlayed = false

function initialQuality(coarse: boolean) {
  const memory = (navigator as Navigator & { deviceMemory?: number }).deviceMemory
  if (memory !== undefined && memory <= 2) return QUALITY_LOW
  if (coarse || (memory !== undefined && memory <= 4)) return QUALITY_MEDIUM
  return QUALITY_HIGH
}

function smooth(value: number) {
  const clamped = Math.min(1, Math.max(0, value))
  return clamped * clamped * (3 - 2 * clamped)
}

const errorMessage = (error: unknown) => (error instanceof Error ? error.message : String(error))
const isAbort = (error: unknown) => error instanceof DOMException && error.name === 'AbortError'

export function MemoryEvolution({
  spaceId,
  refreshKey = 0,
  selectedId = null,
  onSelect,
}: MemoryEvolutionProps) {
  const { t, language } = useI18n()
  const [scene, setScene] = useState<EvolutionScene | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [reloadKey, setReloadKey] = useState(0)
  const [displayAt, setDisplayAt] = useState(0)
  const [announcedAt, setAnnouncedAt] = useState(0)
  const [isPlaying, setIsPlaying] = useState(false)
  const [speed, setSpeed] = useState<Speed>('1')
  const [hover, setHover] = useState<{ index: number; x: number; y: number } | null>(null)
  const [detail, setDetail] = useState<MemoryDetail | null>(null)
  const [breakout, setBreakout] = useState<{ width: number; offset: number } | null>(null)
  const [stageHeight, setStageHeight] = useState(0)
  const [histogramWidth, setHistogramWidth] = useState(0)

  const wrapperRef = useRef<HTMLDivElement | null>(null)
  const stageRef = useRef<HTMLDivElement | null>(null)
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const readoutRef = useRef<HTMLDivElement | null>(null)
  const cardRef = useRef<HTMLElement | null>(null)
  const controlsRef = useRef<HTMLDivElement | null>(null)
  const histogramRef = useRef<HTMLCanvasElement | null>(null)
  const histogramBoxRef = useRef<HTMLDivElement | null>(null)
  const rendererRef = useRef<EvolutionRenderer | null>(null)
  const sceneRef = useRef<EvolutionScene | null>(null)
  const selectedIndexRef = useRef(-1)
  const hoverIndexRef = useRef(-1)
  const speedRef = useRef<Speed>('1')
  // 时间轴的权威值。
  const atRef = useRef(0)
  const renderAtRef = useRef(0)
  const modeRef = useRef<'idle' | 'opening' | 'play'>('idle')
  const openingElapsedRef = useRef(0)
  const effectWindowRef = useRef(1)
  const fadeStartRef = useRef(0)
  const dirtyRef = useRef(true)
  const rafRef = useRef(0)
  const runningRef = useRef(false)
  const lastTickRef = useRef(0)
  const lastDrawRef = useRef(0)
  const lastPublishRef = useRef(0)
  const lastActiveRef = useRef(0)
  const baseYawRef = useRef(DEFAULT_YAW)
  const tiltRef = useRef(WIDE_TILT)
  const tiltTouchedRef = useRef(false)
  const yawTargetRef = useRef<number | null>(null)
  const zoomRef = useRef(1)
  const layoutRef = useRef<(animate?: boolean) => void>(() => {})
  const dragRef = useRef<{
    id: number
    x: number
    y: number
    lastX: number
    lastY: number
    moved: boolean
  } | null>(null)
  const reducedRef = useRef(false)

  const { ref: visibilityRef, playing: stageVisible } = useAnimationVisibility<HTMLDivElement>()
  const setStageRef = useCallback(
    (node: HTMLDivElement | null) => {
      stageRef.current = node
      visibilityRef(node)
    },
    [visibilityRef],
  )

  const publish = useCallback((force = false) => {
    const current = sceneRef.current
    if (!current) return
    const now = performance.now()
    if (!force && now - lastPublishRef.current < 33) return
    lastPublishRef.current = now
    setDisplayAt(atRef.current)
  }, [])

  const tick = useCallback(
    (timestamp: number) => {
      rafRef.current = 0
      const renderer = rendererRef.current
      if (!renderer) return
      const current = sceneRef.current
      const reduced = reducedRef.current
      const dt = lastTickRef.current ? Math.min(64, timestamp - lastTickRef.current) : 16
      lastTickRef.current = timestamp
      let moving = false
      if (current) {
        const { start, end } = current.domain
        const span = Math.max(1, end - start)
        if (modeRef.current === 'opening') {
          openingElapsedRef.current += dt
          const progress = openingProgress(openingElapsedRef.current)
          atRef.current = start + span * progress
          renderer.camera.tilt =
            OPENING_START_TILT + (tiltRef.current - OPENING_START_TILT) * smooth(progress * 1.15)
          renderer.camera.yaw = baseYawRef.current - 1.3 * (1 - progress) ** 2
          moving = true
          if (openingElapsedRef.current >= OPENING_DURATION_MS) {
            atRef.current = end
            modeRef.current = 'idle'
            setIsPlaying(false)
            publish(true)
          }
        } else if (modeRef.current === 'play') {
          atRef.current += (dt * span * Number(speedRef.current)) / PLAY_BASE_MS
          moving = true
          if (atRef.current >= end) {
            atRef.current = end
            modeRef.current = 'idle'
            setIsPlaying(false)
            publish(true)
          }
        }
        // 点击时间轴时不瞬移：画面快进过去，一路的闪光都看得到。
        const previous = renderAtRef.current
        const difference = atRef.current - previous
        if (modeRef.current !== 'idle' || Math.abs(difference) < span * 0.0002)
          renderAtRef.current = atRef.current
        else renderAtRef.current = previous + difference * (1 - Math.exp(-dt / 85))
        if (renderAtRef.current !== previous) moving = true
        // 特效持续的时刻长度跟着速度走，任何速度下都约等于半秒；停下来后迅速收拢，闪光自然熄灭。
        const velocity = Math.abs(renderAtRef.current - previous) / Math.max(1, dt)
        const target = Math.min(span * 0.08, Math.max(span * 0.002, velocity * 520))
        const rate = target > effectWindowRef.current ? 90 : 420
        if (reduced) effectWindowRef.current = span * 0.002
        else {
          effectWindowRef.current += (target - effectWindowRef.current) * (1 - Math.exp(-dt / rate))
          if (Math.abs(target - effectWindowRef.current) > span * 0.0002) moving = true
        }
      }
      const dragging = Boolean(dragRef.current?.moved)
      if (
        !reduced &&
        !dragging &&
        modeRef.current !== 'opening' &&
        hoverIndexRef.current < 0 &&
        selectedIndexRef.current < 0
      ) {
        baseYawRef.current += dt * ROTATION_PER_MS
      }
      // 选中后的镜头运动：走最短的方向转过去，手一碰就让出控制权。
      if (yawTargetRef.current !== null && !dragging) {
        const delta = Math.atan2(
          Math.sin(yawTargetRef.current - baseYawRef.current),
          Math.cos(yawTargetRef.current - baseYawRef.current),
        )
        if (Math.abs(delta) < 0.002 || reduced) {
          baseYawRef.current = yawTargetRef.current
          yawTargetRef.current = null
        } else {
          baseYawRef.current += delta * (1 - Math.exp(-dt / 240))
          moving = true
        }
      }
      const zoomTarget = zoomRef.current * (selectedIndexRef.current >= 0 ? SELECTED_ZOOM : 1)
      if (Math.abs(renderer.camera.zoom - zoomTarget) > 0.001 && !reduced) {
        renderer.camera.zoom += (zoomTarget - renderer.camera.zoom) * (1 - Math.exp(-dt / 240))
        moving = true
      } else {
        renderer.camera.zoom = zoomTarget
      }
      if (renderer.settling) moving = true
      if (modeRef.current !== 'opening') renderer.camera.yaw = baseYawRef.current
      const fade = Math.min(1, (timestamp - fadeStartRef.current) / FADE_MS)
      // 没人动时降帧：自转很慢，看不出来，但能省很多电。
      if (moving || dragging) lastActiveRef.current = timestamp
      const ambientInterval =
        timestamp - lastActiveRef.current > IDLE_AFTER_MS
          ? 66
          : renderer.quality === QUALITY_HIGH
            ? 32
            : 48
      const shouldDraw =
        moving ||
        dirtyRef.current ||
        dragging ||
        fade < 1 ||
        renderer.morphing ||
        (!reduced && timestamp - lastDrawRef.current >= ambientInterval)
      if (shouldDraw) {
        renderer.draw({
          at: renderAtRef.current,
          wall: timestamp,
          effectWindow: effectWindowRef.current,
          effectGain: current
            ? Math.min(
                1,
                Math.max(
                  0,
                  (effectWindowRef.current /
                    (Math.max(1, current.domain.end - current.domain.start) * 0.002) -
                    1.05) /
                    1.6,
                ),
              )
            : 0,
          ambient: !reduced,
          fade: reduced ? 1 : fade,
          moving: modeRef.current !== 'idle' || dragging,
        })
        lastDrawRef.current = timestamp
        dirtyRef.current = false
      }
      if (!reduced) renderer.noteFrameInterval(dt)
      if (moving) publish()
      if (
        runningRef.current &&
        (!reduced || moving || fade < 1 || dirtyRef.current || renderer.morphing)
      ) {
        rafRef.current = requestAnimationFrame(tick)
      } else if (!moving) {
        publish(true)
      }
    },
    [publish],
  )

  const wake = useCallback(() => {
    dirtyRef.current = true
    if (runningRef.current && !rafRef.current) rafRef.current = requestAnimationFrame(tick)
  }, [tick])

  // 用户一碰时间轴就接管：开场和播放都停下。
  const takeOver = useCallback(() => {
    const renderer = rendererRef.current
    if (modeRef.current === 'opening' && renderer) {
      renderer.camera.tilt = tiltRef.current
      renderer.camera.yaw = baseYawRef.current
    }
    if (modeRef.current !== 'idle') {
      modeRef.current = 'idle'
      setIsPlaying(false)
    }
  }, [])

  const moveTo = useCallback(
    (at: number) => {
      const current = sceneRef.current
      if (!current) return
      takeOver()
      atRef.current = Math.min(current.domain.end, Math.max(current.domain.start, at))
      publish(true)
      wake()
    },
    [publish, takeOver, wake],
  )

  const togglePlay = useCallback(() => {
    const current = sceneRef.current
    if (!current || !current.count) return
    if (modeRef.current !== 'idle') {
      takeOver()
      publish(true)
      wake()
      return
    }
    const { start, end } = current.domain
    // 已经在现在了：从头重演一遍。
    if (atRef.current >= end - (end - start) * 0.002) {
      atRef.current = start
      renderAtRef.current = start
    }
    modeRef.current = 'play'
    setIsPlaying(true)
    wake()
  }, [publish, takeOver, wake])

  // 把舞台撑到比内容列更宽：内容列只有 760，门面需要更大的画幅。滚动容器本来就裁掉横向溢出。
  useLayoutEffect(() => {
    const wrapper = wrapperRef.current
    if (!wrapper) return
    const scroller = wrapper.closest('[data-feature-scroller]')
    if (!scroller) return
    const update = () => {
      const available = scroller.clientWidth
      const gutter = available <= 650 ? 0 : 24
      const width = Math.min(available - gutter * 2, 1280)
      const own = wrapper.clientWidth
      setBreakout(width > own ? { width, offset: (own - width) / 2 } : { width: own, offset: 0 })
      // 时间轴是这个视图的主角，一进来就要看得到：页头多高不是这里能假设的，量出来再定舞台高度。
      const stage = stageRef.current
      if (stage) {
        const top =
          stage.getBoundingClientRect().top -
          scroller.getBoundingClientRect().top +
          scroller.scrollTop
        const room = scroller.clientHeight - top - STAGE_BOTTOM_GAP
        const min = available <= 650 ? STAGE_MIN_HEIGHT_NARROW : STAGE_MIN_HEIGHT
        setStageHeight(Math.round(Math.min(STAGE_MAX_HEIGHT, Math.max(min, room))))
      }
    }
    update()
    const observer = new ResizeObserver(update)
    observer.observe(scroller)
    observer.observe(wrapper)
    return () => observer.disconnect()
  }, [])

  useLayoutEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const coarse = matchMedia('(pointer: coarse)').matches
    reducedRef.current = motionReduced()
    let renderer: EvolutionRenderer
    try {
      renderer = new EvolutionRenderer(canvas, { quality: initialQuality(coarse) })
    } catch {
      return
    }
    renderer.camera.yaw = baseYawRef.current
    rendererRef.current = renderer
    const layout = (animate = false) => {
      const stage = stageRef.current
      if (!stage) return
      const width = stage.clientWidth
      const height = stage.clientHeight
      const wide = width >= 720
      const readout = readoutRef.current?.offsetHeight ?? 0
      const controls = controlsRef.current?.offsetHeight ?? 0
      // 窄屏时选中卡片贴在时间轴上方，要知道时间轴有多高。
      stage.style.setProperty('--evo-controls', `${controls}px`)
      if (!tiltTouchedRef.current) {
        tiltRef.current = wide ? WIDE_TILT : NARROW_TILT
        if (modeRef.current !== 'opening') renderer.camera.tilt = tiltRef.current
      }
      renderer.resize(width, height, coarse ? 1.5 : 2)
      // 窄屏时卡片横跨整个舞台，盘面让到它上方；宽屏时卡片在角落，转动镜头就够了。
      const card = wide ? 0 : (cardRef.current?.offsetHeight ?? 0)
      renderer.setInsets(
        {
          top: wide ? 18 : readout + 10,
          bottom: controls + 4 + (card ? card + 4 : 0),
          left: wide ? 28 : 6,
          right: wide ? 28 : 6,
        },
        { animate: animate && !reducedRef.current },
      )
      // 改尺寸会清空画布，立刻补一帧，避免闪黑。
      dirtyRef.current = true
      if (runningRef.current) {
        if (rafRef.current) cancelAnimationFrame(rafRef.current)
        rafRef.current = 0
        tick(performance.now())
      }
    }
    layout()
    layoutRef.current = layout
    const observer = new ResizeObserver(() => layout())
    if (stageRef.current) observer.observe(stageRef.current)
    if (readoutRef.current) observer.observe(readoutRef.current)
    if (controlsRef.current) observer.observe(controlsRef.current)
    return () => {
      observer.disconnect()
      rendererRef.current = null
    }
  }, [tick])

  // 文档不可见或舞台滚出视口时停下 rAF。
  useEffect(() => {
    runningRef.current = stageVisible
    if (stageVisible) {
      lastTickRef.current = 0
      wake()
    }
    return () => {
      runningRef.current = false
      if (rafRef.current) cancelAnimationFrame(rafRef.current)
      rafRef.current = 0
    }
  }, [stageVisible, wake])

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    // 普通滚轮留给页面滚动；触控板捏合和 Ctrl + 滚轮才缩放。
    const onWheel = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return
      event.preventDefault()
      const renderer = rendererRef.current
      if (!renderer) return
      zoomRef.current = Math.min(
        2.6,
        Math.max(0.72, zoomRef.current * Math.exp(-event.deltaY * 0.0022)),
      )
      wake()
    }
    canvas.addEventListener('wheel', onWheel, { passive: false })
    return () => canvas.removeEventListener('wheel', onWheel)
  }, [wake])

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    setError('')
    fetchMemoryEvolution(spaceId, controller.signal)
      .then((data) => {
        if (controller.signal.aborted) return
        setScene(buildEvolutionScene(data))
        setLoading(false)
      })
      .catch((loadError: unknown) => {
        if (controller.signal.aborted || isAbort(loadError)) return
        setError(errorMessage(loadError))
        setLoading(false)
      })
    return () => controller.abort()
  }, [spaceId, refreshKey, reloadKey])

  const ringFormat = useMemo(() => {
    const options: Intl.DateTimeFormatOptions =
      scene?.unit === 'day' || scene?.unit === 'week'
        ? { month: 'short', day: 'numeric' }
        : { year: 'numeric', month: 'short' }
    return new Intl.DateTimeFormat(language, options)
  }, [language, scene?.unit])
  const dateFormat = useMemo(
    () => new Intl.DateTimeFormat(language, { year: 'numeric', month: 'long', day: 'numeric' }),
    [language],
  )
  const shortDateFormat = useMemo(
    () => new Intl.DateTimeFormat(language, { year: 'numeric', month: '2-digit', day: '2-digit' }),
    [language],
  )

  useEffect(() => {
    const renderer = rendererRef.current
    if (!renderer) return
    renderer.setRingFormatter((at) => ringFormat.format(at))
    dirtyRef.current = true
  }, [ringFormat])

  // 新数据到达：第一次按开场播；之后时间轴尽量留在原来的时刻，原来就在“现在”的跟着新的现在走。
  useEffect(() => {
    if (!scene) return
    const previous = sceneRef.current
    const renderer = rendererRef.current
    const { start, end } = scene.domain
    let next = end
    if (previous) {
      const previousSpan = previous.domain.end - previous.domain.start
      const wasAtNow = atRef.current >= previous.domain.end - previousSpan * 0.002
      if (!wasAtNow) next = Math.min(end, Math.max(start, atRef.current))
      if (modeRef.current !== 'idle') {
        modeRef.current = 'idle'
        setIsPlaying(false)
      }
    } else if (!openingPlayed && !reducedRef.current && scene.count > 0) {
      openingPlayed = true
      modeRef.current = 'opening'
      openingElapsedRef.current = 0
      next = start
      setIsPlaying(true)
    }
    sceneRef.current = scene
    atRef.current = next
    renderAtRef.current = next
    effectWindowRef.current = (end - start) * 0.002
    const timestamp = performance.now()
    const morphing = renderer?.setScene(scene, {
      animate: Boolean(previous) && !reducedRef.current,
      now: timestamp,
    })
    // 同一批记忆刷新时光点平滑滑到新位置；换了空间或第一次打开才整屏淡入。
    if (!morphing) fadeStartRef.current = timestamp
    setDisplayAt(next)
    setAnnouncedAt(next)
    setHover(null)
    hoverIndexRef.current = -1
    wake()
  }, [scene, wake])

  const selectedIndex = useMemo(() => {
    if (!scene || !selectedId) return -1
    return scene.indexById.get(selectedId) ?? -1
  }, [scene, selectedId])

  useEffect(() => {
    const renderer = rendererRef.current
    selectedIndexRef.current = selectedIndex
    renderer?.setSelection(selectedIndex)
    const current = sceneRef.current
    if (current && selectedIndex >= 0) {
      yawTargetRef.current = SELECTED_SCREEN_ANGLE - current.theta[selectedIndex]
      // 从列表选中一条还没发生的记忆：时间轴跟到它出现的时刻。
      if (atRef.current < current.appearAt[selectedIndex]) moveTo(current.appearAt[selectedIndex])
    } else {
      yawTargetRef.current = null
    }
    wake()
  }, [moveTo, selectedIndex, wake])

  useEffect(() => {
    setDetail(null)
    if (!selectedId) return
    const controller = new AbortController()
    fetchMemoryDetail(selectedId, controller.signal)
      .then((next) => {
        if (!controller.signal.aborted) setDetail(next)
      })
      .catch(() => {
        // 详情取不到时卡片还有标题和时间，不打扰。
      })
    return () => controller.abort()
  }, [selectedId, refreshKey])

  const chain = useMemo(
    () => (scene && selectedIndex >= 0 ? versionChainOf(scene, selectedIndex) : []),
    [scene, selectedIndex],
  )

  // 卡片高度变了（选中、详情到达、关掉）：窄屏时盘面重新取景。
  useLayoutEffect(() => {
    layoutRef.current(true)
    wake()
  }, [chain, detail, wake])

  // Esc 取消选中。焦点可能在页面任何地方（从列表选中、点完按钮），所以挂在 document 上；别的弹层打开时不抢。
  useEffect(() => {
    if (!selectedId) return
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return
      if (document.querySelector('[role="dialog"], [data-slot="popover-content"]')) return
      onSelect(null)
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [onSelect, selectedId])

  // 读屏按停稳后的时刻播报，不随拖动刷屏；自动播放时不播报。
  useEffect(() => {
    if (isPlaying) return
    const timer = window.setTimeout(() => setAnnouncedAt(displayAt), 700)
    return () => window.clearTimeout(timer)
  }, [displayAt, isPlaying])

  useEffect(() => {
    const box = histogramBoxRef.current
    if (!box) return
    const observer = new ResizeObserver(() => setHistogramWidth(box.clientWidth))
    observer.observe(box)
    setHistogramWidth(box.clientWidth)
    return () => observer.disconnect()
  }, [])

  useEffect(() => {
    const canvas = histogramRef.current
    if (!canvas || !scene || !histogramWidth) return
    drawEvolutionHistogram(canvas, scene, displayAt, histogramWidth, HISTOGRAM_HEIGHT)
  }, [displayAt, histogramWidth, scene])

  const stats = useMemo(
    () => (scene ? statsAt(scene, displayAt) : { active: 0, superseded: 0, archived: 0, known: 0 }),
    [displayAt, scene],
  )
  const announcedStats = useMemo(
    () => (scene ? statsAt(scene, announcedAt) : stats),
    [announcedAt, scene, stats],
  )
  const hasFacts = Boolean(scene?.count)
  const atNow = scene
    ? displayAt >= scene.domain.end - (scene.domain.end - scene.domain.start) * 0.002
    : true
  const dateLabel = scene ? dateFormat.format(displayAt) : ''
  const stageLabel = t('memory:memoryEvolution.stageLabel', {
    date: scene ? dateFormat.format(announcedAt) : '',
    active: announcedStats.active,
    superseded: announcedStats.superseded,
    archived: announcedStats.archived,
  })

  const typeLabel = (type: MemoryType) => {
    if (type === 'decision') return t('memory:memoryEvolution.typeDecision')
    if (type === 'preference') return t('memory:memoryEvolution.typePreference')
    if (type === 'fact') return t('memory:memoryEvolution.typeFact')
    if (type === 'risk') return t('memory:memoryEvolution.typeRisk')
    if (type === 'file') return t('memory:memoryEvolution.typeFile')
    if (type === 'task') return t('memory:memoryEvolution.typeTask')
    return t('memory:memoryEvolution.typeConcept')
  }
  const stateLabel = (state: FactStateName) => {
    if (state === 'superseded') return t('memory:memoryEvolution.superseded')
    if (state === 'archived') return t('memory:memoryEvolution.archived')
    if (state === 'hidden') return t('memory:memoryEvolution.notYet')
    return t('memory:memoryEvolution.active')
  }

  const onKeyDownCapture = (event: KeyboardEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement
    // 分段控件和弹层有自己的方向键，按钮有自己的空格。
    const insideControl = Boolean(
      target.closest('[data-slot="segmented-control"], [data-slot="popover-content"]'),
    )
    if (event.key === ' ') {
      if (target.closest('button, a, [role="radio"]')) return
      event.preventDefault()
      togglePlay()
      return
    }
    const current = sceneRef.current
    if (!current || !current.count || insideControl) return
    const span = current.domain.end - current.domain.start
    const step = span * (event.shiftKey ? 0.05 : 0.01)
    let next: number | null = null
    if (event.key === 'ArrowLeft') next = atRef.current - step
    else if (event.key === 'ArrowRight') next = atRef.current + step
    else if (event.key === 'Home') next = current.domain.start
    else if (event.key === 'End') next = current.domain.end
    if (next === null) return
    event.preventDefault()
    // 捕获阶段处理，不让滑块再按自己的小步长走一次。
    event.stopPropagation()
    moveTo(next)
  }

  const onPointerDown = (event: PointerEvent<HTMLCanvasElement>) => {
    if (event.button !== 0) return
    dragRef.current = {
      id: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      moved: false,
    }
    event.currentTarget.setPointerCapture?.(event.pointerId)
  }

  const onPointerMove = (event: PointerEvent<HTMLCanvasElement>) => {
    const renderer = rendererRef.current
    if (!renderer) return
    const drag = dragRef.current
    if (drag && drag.id === event.pointerId) {
      const dx = event.clientX - drag.lastX
      const dy = event.clientY - drag.lastY
      if (!drag.moved && Math.hypot(event.clientX - drag.x, event.clientY - drag.y) < 5) return
      drag.moved = true
      yawTargetRef.current = null
      drag.lastX = event.clientX
      drag.lastY = event.clientY
      baseYawRef.current += dx * 0.0062
      // 触屏纵向留给页面滚动；鼠标可以调俯仰。
      if (event.pointerType === 'mouse') {
        tiltTouchedRef.current = true
        tiltRef.current = Math.min(1.16, Math.max(0, tiltRef.current - dy * 0.0042))
        renderer.camera.tilt = tiltRef.current
      }
      if (modeRef.current === 'opening') takeOver()
      hoverIndexRef.current = -1
      renderer.setHover(-1)
      setHover(null)
      wake()
      return
    }
    if (event.pointerType !== 'mouse') return
    const rect = event.currentTarget.getBoundingClientRect()
    const x = event.clientX - rect.left
    const y = event.clientY - rect.top
    const index = renderer.pick(x, y, 15, renderAtRef.current)
    if (index !== hoverIndexRef.current) {
      hoverIndexRef.current = index
      renderer.setHover(index)
      wake()
    }
    setHover(index >= 0 ? { index, x, y } : null)
  }

  const onPointerUp = (event: PointerEvent<HTMLCanvasElement>) => {
    const drag = dragRef.current
    dragRef.current = null
    const renderer = rendererRef.current
    if (!renderer || !sceneRef.current || !drag || drag.moved) return
    const rect = event.currentTarget.getBoundingClientRect()
    const index = renderer.pick(
      event.clientX - rect.left,
      event.clientY - rect.top,
      event.pointerType === 'mouse' ? 16 : 28,
      renderAtRef.current,
    )
    onSelect(index >= 0 ? sceneRef.current.facts[index].id : null)
  }

  const onPointerLeave = () => {
    if (hoverIndexRef.current >= 0) {
      hoverIndexRef.current = -1
      rendererRef.current?.setHover(-1)
      wake()
    }
    setHover(null)
  }

  const onDoubleClick = () => {
    const renderer = rendererRef.current
    if (!renderer) return
    zoomRef.current = 1
    tiltTouchedRef.current = false
    tiltRef.current = (stageRef.current?.clientWidth ?? 800) >= 720 ? WIDE_TILT : NARROW_TILT
    renderer.camera.tilt = tiltRef.current
    wake()
  }

  const hoveredFact = hover && scene ? scene.facts[hover.index] : null
  const historyById = useMemo(() => {
    const map = new Map<string, string>()
    if (detail?.item) map.set(detail.item.id, detail.item.content)
    for (const version of detail?.history || []) map.set(version.id, version.content)
    return map
  }, [detail])

  const speedControl = (className: string) => (
    <SegmentedControl
      size="sm"
      className={className}
      aria-label={t('memory:memoryEvolution.speed')}
      value={speed}
      onValueChange={(next) => {
        speedRef.current = next
        setSpeed(next)
      }}
      options={SPEEDS.map((value) => ({ value, label: `${value}×` }))}
    />
  )

  const stageWidth = breakout?.width ?? null
  const tooltipStyle = (() => {
    if (!hover) return undefined
    const width = stageRef.current?.clientWidth ?? 800
    const height = stageRef.current?.clientHeight ?? 600
    return {
      left: Math.min(width - 272, hover.x + 16),
      ...(hover.y > height - 150 ? { bottom: height - hover.y + 16 } : { top: hover.y + 16 }),
    }
  })()

  return (
    <div ref={wrapperRef} className="min-w-0">
      <div
        ref={setStageRef}
        role="group"
        aria-label={t('memory:memoryEvolution.regionLabel')}
        data-memory-evolution
        data-at={scene ? new Date(displayAt).toISOString() : undefined}
        data-active-count={stats.active}
        data-playing={isPlaying || undefined}
        onKeyDownCapture={onKeyDownCapture}
        style={{
          ...STAGE_TOKENS,
          ...(stageWidth ? { width: stageWidth, marginLeft: breakout?.offset ?? 0 } : {}),
          ...(stageHeight ? { height: stageHeight } : {}),
        }}
        className="@container relative isolate h-[clamp(560px,calc(100dvh-196px),880px)] overflow-hidden rounded-xl bg-[var(--evo-base)] text-content select-none max-[650px]:h-[clamp(540px,calc(100dvh-132px),780px)] max-[650px]:rounded-none"
      >
        <canvas
          ref={canvasRef}
          role="img"
          aria-label={stageLabel}
          tabIndex={0}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={() => {
            dragRef.current = null
          }}
          onPointerLeave={onPointerLeave}
          onDoubleClick={onDoubleClick}
          className="absolute inset-0 size-full cursor-grab touch-pan-y outline-none focus-visible:ring-2 focus-visible:ring-focus focus-visible:ring-inset active:cursor-grabbing"
        />

        <div className="sr-only" aria-live="polite" role="status">
          {scene && !isPlaying ? stageLabel : ''}
        </div>

        <div
          ref={readoutRef}
          className="pointer-events-none absolute top-5 left-6 z-10 flex max-w-[min(70%,420px)] flex-col gap-1.5 @max-[719px]:top-3.5 @max-[719px]:right-4 @max-[719px]:left-4 @max-[719px]:max-w-none"
        >
          <div className="flex items-baseline gap-2.5">
            {hasFacts && (
              <span
                data-memory-evolution-date
                className="text-[30px] leading-9 font-light tracking-[-0.015em] text-content tabular-nums @max-[719px]:text-2xl @max-[719px]:leading-8"
              >
                {dateLabel}
              </span>
            )}
            {hasFacts && atNow && (
              <span className="text-xs text-[var(--evo-active)]">
                {t('memory:memoryEvolution.now')}
              </span>
            )}
          </div>
          {hasFacts && (
            <dl className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-content-muted tabular-nums">
              <div className="flex items-center gap-1.5">
                <span
                  aria-hidden="true"
                  className="size-1.5 rounded-full bg-[var(--evo-active)] shadow-[0_0_8px_var(--evo-active)]"
                />
                <dt>{t('memory:memoryEvolution.active')}</dt>
                <dd data-memory-evolution-active className="font-medium text-content">
                  {stats.active}
                </dd>
              </div>
              <div className="flex items-center gap-1.5">
                <span
                  aria-hidden="true"
                  className="size-1.5 rounded-full"
                  style={{ backgroundColor: EMBER_DOT, boxShadow: `0 0 8px ${EMBER_DOT}` }}
                />
                <dt>{t('memory:memoryEvolution.superseded')}</dt>
                <dd className="font-medium text-content">{stats.superseded}</dd>
              </div>
              <div className="flex items-center gap-1.5">
                <span
                  aria-hidden="true"
                  className="size-1.5 rounded-full"
                  style={{ backgroundColor: ARCHIVE_DOT }}
                />
                <dt>{t('memory:memoryEvolution.archived')}</dt>
                <dd className="font-medium text-content">{stats.archived}</dd>
              </div>
            </dl>
          )}
          {scene?.truncated && (
            <p className="m-0 text-xs text-content-faint">
              {t('memory:memoryEvolution.truncated', { count: scene.count })}
            </p>
          )}
          {error && scene && <p className="m-0 text-xs text-danger">{error}</p>}
        </div>

        {hasFacts && selectedIndex < 0 && (
          <ul
            aria-label={t('memory:memoryEvolution.legend')}
            className="pointer-events-none absolute top-1/2 right-6 z-10 hidden -translate-y-[62%] list-none flex-col gap-1.5 p-0 text-[11px] text-content-muted @min-[880px]:flex"
          >
            {EVOLUTION_TYPES.map((type) => (
              <li key={type} className="flex items-center justify-end gap-2">
                {typeLabel(type)}
                <span
                  aria-hidden="true"
                  className="size-1.5 rounded-full"
                  style={{
                    backgroundColor: rgbCss(TYPE_COLORS[type]),
                    boxShadow: `0 0 7px ${rgbCss(TYPE_COLORS[type])}`,
                  }}
                />
              </li>
            ))}
          </ul>
        )}

        {hasFacts && (
          <div className="absolute top-3.5 right-4 z-10 @max-[719px]:top-2.5 @max-[719px]:right-2.5">
            <Popover>
              <PopoverTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label={t('memory:memoryEvolution.howToRead')}
                  title={t('memory:memoryEvolution.howToRead')}
                >
                  <Info aria-hidden="true" />
                </Button>
              </PopoverTrigger>
              <PopoverContent align="end" className="w-80 gap-2 text-xs">
                <p className="m-0 leading-relaxed">{t('memory:memoryEvolution.howToReadBody')}</p>
                <ul className="m-0 grid list-none grid-cols-2 gap-x-3 gap-y-1 p-0">
                  {EVOLUTION_TYPES.map((type) => (
                    <li key={type} className="flex items-center gap-2 text-content-secondary">
                      <span
                        aria-hidden="true"
                        className="size-2 rounded-full"
                        style={{ backgroundColor: rgbCss(TYPE_COLORS[type]) }}
                      />
                      {typeLabel(type)}
                    </li>
                  ))}
                </ul>
                <p className="m-0 leading-relaxed text-content-muted">
                  {t('memory:memoryEvolution.gestures')}
                </p>
              </PopoverContent>
            </Popover>
          </div>
        )}

        {hoveredFact && hover && scene && hover.index !== selectedIndex && (
          <div
            role="tooltip"
            data-memory-evolution-tooltip
            style={tooltipStyle}
            className="pointer-events-none absolute z-20 flex w-max max-w-64 flex-col gap-0.5 rounded-md border border-[var(--evo-line)] bg-[var(--evo-panel)] px-2.5 py-1.5 text-xs backdrop-blur-md"
          >
            <strong className="line-clamp-2 font-medium text-content">{hoveredFact.title}</strong>
            <span className="text-content-muted tabular-nums">
              {typeLabel(hoveredFact.type)} · {shortDateFormat.format(scene.validAt[hover.index])} ·{' '}
              {stateLabel(factStateNameAt(scene, hover.index, displayAt))}
            </span>
            {Boolean(scene.flags[hover.index] & FLAG_LATE) && (
              <span className="text-content-muted tabular-nums">
                {t('memory:memoryEvolution.learnedLate', {
                  date: shortDateFormat.format(scene.appearAt[hover.index]),
                })}
              </span>
            )}
          </div>
        )}

        {scene && selectedIndex >= 0 && (
          <section
            ref={cardRef}
            data-memory-evolution-card
            aria-label={scene.facts[selectedIndex].title}
            className="absolute top-14 right-4 z-20 flex max-h-[calc(100%-230px)] w-[320px] flex-col gap-2 overflow-y-auto rounded-lg border border-[var(--evo-line)] bg-[var(--evo-panel)] p-3 backdrop-blur-md @max-[719px]:top-auto @max-[719px]:right-2.5 @max-[719px]:bottom-[calc(var(--evo-controls,160px)+4px)] @max-[719px]:left-2.5 @max-[719px]:max-h-[32%] @max-[719px]:w-auto"
          >
            <header className="flex min-w-0 items-start justify-between gap-2">
              <h3 className="m-0 min-w-0 text-[13px] leading-5 font-medium break-words text-content">
                {scene.facts[selectedIndex].title}
              </h3>
              <Button
                variant="ghost"
                size="icon-sm"
                className="-mt-1 -mr-1 flex-none"
                aria-label={t('memory:memoryEvolution.closeCard')}
                onClick={() => onSelect(null)}
              >
                <X aria-hidden="true" />
              </Button>
            </header>
            <p className="m-0 flex flex-wrap gap-x-2 text-[11px] text-content-muted">
              <span>{typeLabel(scene.facts[selectedIndex].type)}</span>
              {Boolean(scene.flags[selectedIndex] & FLAG_PINNED) && (
                <span>{t('memory:memoryEvolution.pinned')}</span>
              )}
              {Boolean(scene.flags[selectedIndex] & FLAG_TAINTED) && (
                <span>{t('memory:memoryEvolution.untrusted')}</span>
              )}
              {Boolean(scene.flags[selectedIndex] & FLAG_STALE) && (
                <span>{t('memory:memoryEvolution.stale')}</span>
              )}
            </p>
            <ol className="m-0 flex list-none flex-col gap-0.5 p-0">
              {chain.map((index) => {
                const fact = scene.facts[index]
                const state = factStateNameAt(scene, index, displayAt)
                const content = historyById.get(fact.id)
                return (
                  <li key={fact.id}>
                    <button
                      type="button"
                      aria-current={index === selectedIndex || undefined}
                      onClick={() => {
                        moveTo(scene.appearAt[index])
                        onSelect(fact.id)
                      }}
                      className={cn(
                        'flex w-full min-w-0 flex-col gap-0.5 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-surface-hover focus-visible:ring-2 focus-visible:ring-focus focus-visible:outline-none pointer-coarse:min-h-11',
                        index === selectedIndex && 'bg-surface-muted',
                      )}
                    >
                      <span className="flex items-center gap-1.5 text-[11px] text-content-muted tabular-nums">
                        <span
                          aria-hidden="true"
                          className="size-1.5 flex-none rounded-full"
                          style={{
                            backgroundColor:
                              state === 'superseded'
                                ? EMBER_DOT
                                : state === 'archived'
                                  ? ARCHIVE_DOT
                                  : rgbCss(TYPE_COLORS[fact.type] ?? TYPE_COLORS.concept),
                          }}
                        />
                        {shortDateFormat.format(scene.validAt[index])} · {stateLabel(state)}
                      </span>
                      <span className="line-clamp-3 text-xs leading-[1.45] break-words text-content-soft">
                        {content || fact.title}
                      </span>
                    </button>
                  </li>
                )
              })}
            </ol>
          </section>
        )}

        {scene && !hasFacts && (
          <div className="absolute inset-x-0 bottom-5 z-10 flex justify-center px-6">
            <InlineEmptyState
              compact
              illustration={<span />}
              title={t('memory:memoryEvolution.emptyTitle')}
              description={t('memory:memoryEvolution.emptyDescription')}
            />
          </div>
        )}

        {!scene && loading && (
          <div
            role="status"
            className="absolute inset-0 z-10 grid place-items-center text-xs text-content-muted"
          >
            <span className="flex items-center gap-2">
              <OrbitSpinner className="size-4" />
              {t('memory:memoryEvolution.loading')}
            </span>
          </div>
        )}

        {!scene && !loading && error && (
          <div className="absolute inset-0 z-10 grid place-items-center">
            <PageState
              kind="error"
              title={t('memory:memoryEvolution.loadFailed')}
              description={error}
              action={
                <Button
                  variant="secondary"
                  className="max-[650px]:h-11"
                  onClick={() => setReloadKey((value) => value + 1)}
                >
                  {t('memory:memoryEvolution.retry')}
                </Button>
              }
            />
          </div>
        )}

        <div
          ref={controlsRef}
          className={cn(
            'absolute inset-x-0 bottom-0 z-10 bg-gradient-to-t from-[var(--evo-scrim)] via-[var(--evo-scrim)] via-40% to-[var(--evo-scrim-clear)] px-5 pt-7 pb-3.5 @max-[719px]:px-3 @max-[719px]:pt-5 @max-[719px]:pb-3',
            !hasFacts && 'invisible',
          )}
        >
          <div className="flex items-end gap-3 @max-[719px]:flex-col @max-[719px]:items-stretch @max-[719px]:gap-1">
            <div className="flex flex-none items-center gap-1 pb-1 @max-[719px]:order-2 @max-[719px]:justify-between @max-[719px]:pb-0">
              <div className="flex items-center gap-1">
                <Button
                  variant="ghost"
                  size="icon"
                  data-memory-evolution-play
                  className="max-[650px]:size-11"
                  aria-label={
                    isPlaying ? t('memory:memoryEvolution.pause') : t('memory:memoryEvolution.play')
                  }
                  title={
                    isPlaying ? t('memory:memoryEvolution.pause') : t('memory:memoryEvolution.play')
                  }
                  disabled={!hasFacts}
                  onClick={togglePlay}
                >
                  {isPlaying ? <Pause aria-hidden="true" /> : <Play aria-hidden="true" />}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  data-memory-evolution-now
                  className="aria-disabled:opacity-50 max-[650px]:h-11"
                  // 不用 disabled：已经在现在时按钮还在，焦点不能因为它变灰就丢掉。
                  aria-disabled={!hasFacts || (atNow && !isPlaying)}
                  onClick={() => {
                    if (scene && !(atNow && !isPlaying)) moveTo(scene.domain.end)
                  }}
                >
                  <SkipForward aria-hidden="true" />
                  {t('memory:memoryEvolution.backToNow')}
                </Button>
              </div>
              {speedControl('hidden @max-[719px]:inline-flex')}
            </div>
            <div className="min-w-0 flex-1 @max-[719px]:order-1">
              <div
                ref={histogramBoxRef}
                aria-hidden="true"
                style={{
                  marginLeft: THUMB_INSET,
                  marginRight: THUMB_INSET,
                  height: HISTOGRAM_HEIGHT,
                }}
              >
                <canvas ref={histogramRef} className="block size-full" />
              </div>
              {scene && (
                <Slider
                  aria-label={t('memory:memoryEvolution.timelineLabel')}
                  valueText={dateLabel}
                  min={0}
                  max={SLIDER_MAX}
                  step={1}
                  value={[sliderFromTime(scene.domain, displayAt)]}
                  disabled={!hasFacts}
                  className="h-5 pointer-coarse:h-11 max-[650px]:h-11"
                  onValueChange={([value]) => moveTo(timeFromSlider(scene.domain, value))}
                />
              )}
              {scene && (
                <div
                  aria-hidden="true"
                  className="flex justify-between px-[7px] text-[11px] text-content-faint tabular-nums"
                >
                  <span>{shortDateFormat.format(scene.domain.start)}</span>
                  <span>{shortDateFormat.format(scene.domain.end)}</span>
                </div>
              )}
            </div>
            {speedControl('mb-1 flex-none @max-[719px]:hidden')}
          </div>
        </div>
      </div>
    </div>
  )
}

export default MemoryEvolution
