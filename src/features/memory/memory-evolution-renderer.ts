// 记忆年轮的画布：Canvas 2D + 加法混合 + 预渲染光晕，再用降采样叠加做泛光，不依赖 WebGL 和任何图形库。
// 画面是时刻 T 的纯函数：闪光、余烬、光脉都按 T 算，所以时间轴往回拖也完全可逆，拖到哪就是哪；
// 实际时钟只驱动呼吸、微粒和缓慢的自转。舞台是恒定的深色场景，颜色不随主题翻转；
// Canvas 读不到 CSS 变量，又要逐帧按数值混色，所以调色板写在这里。
import {
  discX,
  discY,
  evolutionAgeFade,
  EVOLUTION_TYPES,
  FLAG_LATE,
  FLAG_PINNED,
  FLAG_STALE,
  FLAG_TAINTED,
  rhoAt,
  type EvolutionScene,
  versionChainOf,
} from './memory-evolution-model'
import type { MemoryType } from './memory-types'

type Rgb = readonly [number, number, number]

const TAU = Math.PI * 2
const RING_SEGMENTS = 144
const GRAIN_SEGMENTS = 72
const PERSPECTIVE = 0.2
const SPRITE_SIZE = 96
const MORPH_MS = 620
// 分辨率上限：再高的屏幕也不超过这么多像素，泛光是全屏操作，像素数决定开销。
const MAX_CANVAS_PIXELS = 5_500_000
// 最新那一带的光：由外向内越来越淡，内边界按前沿半径的比例算。
const BAND_LAYERS = [
  { inner: 0.965, alpha: 0.05 },
  { inner: 0.9, alpha: 0.032 },
  { inner: 0.78, alpha: 0.018 },
] as const
// 前沿呼吸的快慢，约 5 秒一次。
const BREATH_RATE = 0.0012
// 整批描边的线条按这个比例分老、新两档：比前沿半径的这个比例还靠里的算老。
const YOUNG_FROM = 0.6

export const TYPE_COLORS: Record<MemoryType, Rgb> = {
  decision: [255, 196, 92],
  preference: [186, 164, 255],
  fact: [176, 214, 255],
  concept: [104, 176, 255],
  risk: [255, 104, 158],
  file: [92, 226, 176],
  task: [92, 222, 240],
}
export const EMBER: Rgb = [255, 96, 44]
export const ARCHIVE: Rgb = [96, 118, 164]
const UNTRUSTED: Rgb = [156, 164, 178]
const RING: Rgb = [236, 208, 164]
const FRONTIER: Rgb = [255, 232, 192]
const THREAD: Rgb = [255, 176, 104]
const PULSE: Rgb = [255, 236, 204]
const CORE: Rgb = [255, 206, 138]
const DUST_WARM: Rgb = [255, 222, 180]
const DUST_COOL: Rgb = [176, 202, 255]

export const rgbCss = ([r, g, b]: Rgb, alpha = 1) => `rgba(${r}, ${g}, ${b}, ${alpha})`

// rim：0 把整个圆盘放在取景中央，1 把近侧的外圈固定在取景下方。
export type EvolutionCamera = { yaw: number; tilt: number; zoom: number; rim: number }
export type EvolutionInsets = { top: number; right: number; bottom: number; left: number }
export type EvolutionFrame = {
  at: number
  // 实际时间（毫秒）：只驱动呼吸、微粒和闲时光脉。
  wall: number
  // 诞生和替换的特效持续多久（时刻长度）。随播放速度变化，保证任何速度下都约等于半秒。
  effectWindow: number
  // false 时不呼吸、不闲时光脉（减少动效）。
  ambient: boolean
  // 特效强度，0 到 1：时间停下来时闪光和光脉熄灭，不会冻在画面上。
  effectGain: number
  // 新数据淡入，0 到 1。
  fade: number
  // 时间正在走：前沿线更亮。
  moving: boolean
}

function smoothRange(value: number, from: number, to: number) {
  const progress = Math.min(1, Math.max(0, (value - from) / (to - from)))
  return progress * progress * (3 - 2 * progress)
}

function makeCanvas(width: number, height: number) {
  const canvas = document.createElement('canvas')
  canvas.width = Math.max(1, Math.round(width))
  canvas.height = Math.max(1, Math.round(height))
  return canvas
}

// 光点：极亮的白芯 + 带颜色的中圈 + 很淡的外晕。加法混合下，重叠的地方自然变亮。
function glowSprite([r, g, b]: Rgb) {
  const canvas = makeCanvas(SPRITE_SIZE, SPRITE_SIZE)
  const context = canvas.getContext('2d')
  if (!context) return canvas
  const center = SPRITE_SIZE / 2
  const gradient = context.createRadialGradient(center, center, 0, center, center, center)
  const mix = (t: number) => [255 + (r - 255) * t, 255 + (g - 255) * t, 255 + (b - 255) * t]
  const color = (t: number, alpha: number) => {
    const [mr, mg, mb] = mix(t)
    return `rgba(${mr | 0}, ${mg | 0}, ${mb | 0}, ${alpha})`
  }
  gradient.addColorStop(0, color(0.12, 1))
  gradient.addColorStop(0.08, color(0.5, 0.96))
  gradient.addColorStop(0.16, color(1, 0.62))
  gradient.addColorStop(0.3, color(1, 0.22))
  gradient.addColorStop(0.56, color(1, 0.06))
  gradient.addColorStop(1, color(1, 0))
  context.fillStyle = gradient
  context.fillRect(0, 0, SPRITE_SIZE, SPRITE_SIZE)
  return canvas
}

// 大而软的光晕：钉住的记忆、髓心和闪光用它。
function haloSprite([r, g, b]: Rgb) {
  const canvas = makeCanvas(SPRITE_SIZE, SPRITE_SIZE)
  const context = canvas.getContext('2d')
  if (!context) return canvas
  const center = SPRITE_SIZE / 2
  const gradient = context.createRadialGradient(center, center, 0, center, center, center)
  gradient.addColorStop(0, `rgba(${r}, ${g}, ${b}, 0.55)`)
  gradient.addColorStop(0.35, `rgba(${r}, ${g}, ${b}, 0.16)`)
  gradient.addColorStop(0.7, `rgba(${r}, ${g}, ${b}, 0.035)`)
  gradient.addColorStop(1, `rgba(${r}, ${g}, ${b}, 0)`)
  context.fillStyle = gradient
  context.fillRect(0, 0, SPRITE_SIZE, SPRITE_SIZE)
  return canvas
}

export const QUALITY_HIGH = 3
export const QUALITY_MEDIUM = 2
export const QUALITY_LOW = 1

export class EvolutionRenderer {
  readonly canvas: HTMLCanvasElement
  private readonly ctx: CanvasRenderingContext2D
  camera: EvolutionCamera = { yaw: 0, tilt: 0.92, zoom: 1, rim: 0 }
  quality: number
  private scene: EvolutionScene | null = null
  private width = 1
  private height = 1
  private dpr = 1
  private maxDpr = 2
  // 取景留白：targetInsets 是要去的位置，insets 每帧追一点，选中卡片弹出时盘面平滑让开。
  private insets: EvolutionInsets = { top: 0, right: 0, bottom: 0, left: 0 }
  private targetInsets: EvolutionInsets = { top: 0, right: 0, bottom: 0, left: 0 }
  private readonly typeSprites: HTMLCanvasElement[]
  private readonly emberSprite: HTMLCanvasElement
  private readonly archiveSprite: HTMLCanvasElement
  private readonly untrustedSprite: HTMLCanvasElement
  private readonly pulseSprite: HTMLCanvasElement
  private readonly coreSprite: HTMLCanvasElement
  private readonly discSprite: HTMLCanvasElement
  private readonly pinnedHalo: HTMLCanvasElement
  private background: HTMLCanvasElement | null = null
  private bloomA: HTMLCanvasElement | null = null
  private bloomB: HTMLCanvasElement | null = null
  private ringPaths: Float32Array[] = []
  private grainPaths: { rho: number; path: Float32Array }[] = []
  private readonly frontierPath = new Float32Array(RING_SEGMENTS * 2)
  private readonly rimPath = new Float32Array(RING_SEGMENTS * 2)
  // 最新那一带由外向内的三层边界。
  private readonly bandPaths = BAND_LAYERS.map(() => new Float32Array(RING_SEGMENTS * 2))
  private screenX = new Float32Array(0)
  private screenY = new Float32Array(0)
  private screenR = new Float32Array(0)
  private liftNow = new Float32Array(0)
  // 当前帧的世界坐标：平时等于场景的坐标，数据刷新时从旧位置滑过去。
  private worldX = new Float32Array(0)
  private worldY = new Float32Array(0)
  private morphX: Float32Array | null = null
  private morphY: Float32Array | null = null
  private morphStart = 0
  private slowWindows = 0
  private chainMask = new Uint8Array(0)
  private ringLabels: string[] = []
  private sectorOrder: number[] = []
  private selected = -1
  private selectedSector = -1
  private hovered = -1
  private ringFormatter: (at: number) => string = () => ''
  private readonly dust: Float32Array
  private readonly rippleQueue = new Int32Array(48)
  private readonly lateQueue = new Int32Array(24)
  private readonly typeCss: string[]
  private ambientPulses: { replacement: number; start: number }[] = []
  private nextAmbientPulse = 0
  private frameTimes = 0
  private frameCount = 0
  // 投影结果放在字段里，避免每个点分配一个对象。
  private px = 0
  private py = 0
  private ps = 1
  private cx = 0
  private cy = 0
  private unit = 1
  private cosYaw = 1
  private sinYaw = 0
  private cosTilt = 1
  private sinTilt = 0

  constructor(canvas: HTMLCanvasElement, { quality = QUALITY_HIGH }: { quality?: number } = {}) {
    this.canvas = canvas
    const context = canvas.getContext('2d', { alpha: false })
    if (!context) throw new Error('canvas 2d unavailable')
    this.ctx = context
    this.quality = quality
    this.typeSprites = EVOLUTION_TYPES.map((type) => glowSprite(TYPE_COLORS[type]))
    this.typeCss = EVOLUTION_TYPES.map((type) => rgbCss(TYPE_COLORS[type]))
    this.emberSprite = glowSprite(EMBER)
    this.archiveSprite = glowSprite(ARCHIVE)
    this.untrustedSprite = glowSprite(UNTRUSTED)
    this.pulseSprite = glowSprite(PULSE)
    this.coreSprite = haloSprite(CORE)
    this.discSprite = haloSprite([206, 158, 104])
    this.pinnedHalo = haloSprite(FRONTIER)
    // 环境微粒：位置固定、不依赖数据。3-8 条记忆时也是它们把舞台撑起来。
    this.dust = new Float32Array(200 * 5)
    let seed = 20261105
    const random = () => {
      seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0
      return seed / 4294967296
    }
    for (let index = 0; index < 200; index += 1) {
      const angle = random() * TAU
      const radius = 0.08 + Math.sqrt(random()) * 1.42
      this.dust[index * 5] = Math.cos(angle) * radius
      this.dust[index * 5 + 1] = Math.sin(angle) * radius
      this.dust[index * 5 + 2] = (random() - 0.35) * 0.42
      this.dust[index * 5 + 3] = random() * TAU
      this.dust[index * 5 + 4] = 0.15 + random() * 0.55
    }
  }

  setRingFormatter(format: (at: number) => string) {
    this.ringFormatter = format
    this.refreshLabels()
  }

  // 返回 true 表示新旧数据有足够多相同的记忆，光点会从旧位置滑到新位置，不需要整屏淡入。
  setScene(scene: EvolutionScene | null, { animate = false, now = 0 } = {}) {
    const previous = this.scene
    const previousX = this.worldX
    const previousY = this.worldY
    this.scene = scene
    this.ambientPulses = []
    this.morphX = null
    this.morphY = null
    if (!scene) return false
    let morphing = false
    if (animate && previous && previous.count && scene.count) {
      const fromX = Float32Array.from(scene.x)
      const fromY = Float32Array.from(scene.y)
      let matched = 0
      scene.facts.forEach((fact, index) => {
        const old = previous.indexById.get(fact.id)
        if (old === undefined) return
        fromX[index] = previousX[old]
        fromY[index] = previousY[old]
        matched += 1
      })
      if (matched >= Math.max(1, scene.count * 0.3)) {
        this.morphX = fromX
        this.morphY = fromY
        this.morphStart = now
        morphing = true
      }
    }
    this.worldX = Float32Array.from(this.morphX ?? scene.x)
    this.worldY = Float32Array.from(this.morphY ?? scene.y)
    this.screenX = new Float32Array(scene.count)
    this.screenY = new Float32Array(scene.count)
    this.screenR = new Float32Array(scene.count)
    this.liftNow = new Float32Array(scene.count)
    this.chainMask = new Uint8Array(scene.count)
    this.ringPaths = scene.rings.map((ring) => {
      const path = new Float32Array(RING_SEGMENTS * 2)
      this.fillRingPath(path, ring.rho)
      return path
    })
    this.fillRingPath(this.rimPath, 1)
    this.grainPaths = this.buildGrain()
    this.sectorOrder = scene.sectors
      .map((sector, index) => ({ sector, index }))
      .filter(({ sector }) => sector.kind === 'topic')
      .sort((left, right) => right.sector.count - left.sector.count)
      .slice(0, 10)
      .map((item) => item.index)
    this.refreshLabels()
    this.setSelection(this.selected < scene.count ? this.selected : -1)
    this.hovered = -1
    return morphing
  }

  // 木纹：两圈之间的细线，不携带数据，只是让盘面有质感。
  private buildGrain() {
    const grain: { rho: number; path: Float32Array }[] = []
    let seed = 7
    for (let rho = 0.07; rho < 1; rho += 0.0115) {
      seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0
      const jitter = ((seed >>> 8) / 16777216 - 0.5) * 0.006
      const path = new Float32Array(GRAIN_SEGMENTS * 2)
      for (let index = 0; index < GRAIN_SEGMENTS; index += 1) {
        const theta = (index / GRAIN_SEGMENTS) * TAU
        path[index * 2] = discX(theta, rho + jitter)
        path[index * 2 + 1] = discY(theta, rho + jitter)
      }
      grain.push({ rho: rho + jitter, path })
    }
    return grain
  }

  private refreshLabels() {
    this.ringLabels = this.scene ? this.scene.rings.map((ring) => this.ringFormatter(ring.at)) : []
  }

  setSelection(index: number) {
    const { scene } = this
    this.selected = index
    this.chainMask.fill(0)
    this.selectedSector = -1
    if (!scene || index < 0 || index >= scene.count) {
      this.selected = -1
      return
    }
    for (const member of versionChainOf(scene, index)) this.chainMask[member] = 1
    this.selectedSector = scene.sector[index]
  }

  setHover(index: number) {
    this.hovered = index
  }

  resize(width: number, height: number, maxDpr: number) {
    const nextWidth = Math.max(1, width)
    const nextHeight = Math.max(1, height)
    if (nextWidth === this.width && nextHeight === this.height && maxDpr === this.maxDpr) return
    this.width = nextWidth
    this.height = nextHeight
    this.maxDpr = maxDpr
    this.applyResolution()
  }

  setInsets(insets: EvolutionInsets, { animate = false } = {}) {
    this.targetInsets = insets
    if (!animate) this.insets = insets
  }

  // 取景还在移动：出帧循环要继续（减少动效时不会发生）。
  get settling() {
    const { insets, targetInsets } = this
    return (
      insets.top !== targetInsets.top ||
      insets.right !== targetInsets.right ||
      insets.bottom !== targetInsets.bottom ||
      insets.left !== targetInsets.left
    )
  }

  private stepInsets() {
    const step = (current: number, target: number) =>
      Math.abs(target - current) < 0.5 ? target : current + (target - current) * 0.14
    const { insets, targetInsets } = this
    if (!this.settling) return
    this.insets = {
      top: step(insets.top, targetInsets.top),
      right: step(insets.right, targetInsets.right),
      bottom: step(insets.bottom, targetInsets.bottom),
      left: step(insets.left, targetInsets.left),
    }
  }

  private applyResolution() {
    const device = typeof devicePixelRatio === 'number' ? devicePixelRatio : 1
    const cap = this.quality === QUALITY_LOW ? 1 : this.maxDpr
    const pixelCap = Math.sqrt(MAX_CANVAS_PIXELS / (this.width * this.height))
    this.dpr = Math.max(1, Math.min(device || 1, cap, pixelCap))
    this.canvas.width = Math.max(1, Math.round(this.width * this.dpr))
    this.canvas.height = Math.max(1, Math.round(this.height * this.dpr))
    this.background = this.paintBackground()
    this.bloomA = makeCanvas(this.canvas.width / 4, this.canvas.height / 4)
    this.bloomB = makeCanvas(this.canvas.width / 10, this.canvas.height / 10)
  }

  // 按实际帧间隔自动降档：顶不住就先去掉最宽的泛光和木纹，再降分辨率。连续两个窗口都慢才动手，
  // 主线程偶尔忙一下不算；不升档，避免画面来回闪。
  noteFrameInterval(ms: number) {
    this.frameTimes += ms
    this.frameCount += 1
    if (this.frameCount < 45) return
    const average = this.frameTimes / this.frameCount
    this.frameTimes = 0
    this.frameCount = 0
    this.slowWindows = average > 30 ? this.slowWindows + 1 : 0
    if (this.slowWindows >= 2 && this.quality > QUALITY_LOW) {
      this.slowWindows = 0
      this.quality -= 1
      this.applyResolution()
    }
  }

  private paintBackground() {
    const canvas = makeCanvas(Math.min(1024, this.width), Math.min(1024, this.height))
    const context = canvas.getContext('2d')
    if (!context) return canvas
    const { width, height } = canvas
    context.fillStyle = '#05060a'
    context.fillRect(0, 0, width, height)
    const center = this.layoutCenter()
    const glow = context.createRadialGradient(
      (center.x / this.width) * width,
      (center.y / this.height) * height,
      0,
      (center.x / this.width) * width,
      (center.y / this.height) * height,
      Math.max(width, height) * 0.72,
    )
    glow.addColorStop(0, 'rgba(30, 27, 32, 0.8)')
    glow.addColorStop(0.42, 'rgba(10, 13, 22, 0.7)')
    glow.addColorStop(1, 'rgba(3, 4, 8, 0)')
    context.fillStyle = glow
    context.fillRect(0, 0, width, height)
    return canvas
  }

  private layoutCenter() {
    const { top, bottom, left, right } = this.insets
    return {
      x: left + (this.width - left - right) / 2,
      y: top + (this.height - top - bottom) / 2,
    }
  }

  private fillRingPath(path: Float32Array, rho: number) {
    for (let index = 0; index < RING_SEGMENTS; index += 1) {
      const theta = (index / RING_SEGMENTS) * TAU
      path[index * 2] = discX(theta, rho)
      path[index * 2 + 1] = discY(theta, rho)
    }
  }

  private project(x: number, y: number, z: number) {
    const rx = x * this.cosYaw - y * this.sinYaw
    const ry = x * this.sinYaw + y * this.cosYaw
    const depth = ry * this.sinTilt + z * this.cosTilt
    const scale = 1 / (1 - PERSPECTIVE * depth)
    this.px = this.cx + this.unit * rx * scale
    this.py = this.cy + this.unit * (ry * this.cosTilt - z * this.sinTilt) * scale
    this.ps = scale
  }

  private traceRing(path: Float32Array) {
    const { ctx } = this
    for (let index = 0; index < RING_SEGMENTS; index += 1) {
      this.project(path[index * 2], path[index * 2 + 1], 0)
      if (index === 0) ctx.moveTo(this.px, this.py)
      else ctx.lineTo(this.px, this.py)
    }
    ctx.closePath()
  }

  private applyCamera(frontierRho: number) {
    const { camera } = this
    this.stepInsets()
    this.cosYaw = Math.cos(camera.yaw)
    this.sinYaw = Math.sin(camera.yaw)
    this.cosTilt = Math.cos(camera.tilt)
    this.sinTilt = Math.sin(camera.tilt)
    const { top, bottom, left, right } = this.insets
    const availableWidth = Math.max(40, this.width - left - right)
    const availableHeight = Math.max(40, this.height - top - bottom)
    const center = this.layoutCenter()
    const near = 1 + PERSPECTIVE * this.sinTilt * 0.55
    const fit = Math.min(
      availableWidth / (2.08 * near),
      availableHeight / (2.08 * Math.max(this.cosTilt, 0.34) * near + 0.14 * this.sinTilt),
    )
    // 年轮还小的时候镜头离得近，随着生长慢慢拉远。世界坐标不变，只是取景变化。
    const dolly = 1 + 0.72 * (1 - frontierRho) ** 2
    this.unit = fit * camera.zoom * dolly
    this.cx = center.x
    this.cy = center.y + this.sinTilt * this.unit * 0.02
    // 把近侧的外圈固定在取景下方，圆心往上让。年轮还小时不用：整个圆盘本来就放得下。
    const grown = smoothRange(frontierRho, 0.35, 0.75)
    const rim = camera.rim * grown
    if (rim > 0) {
      const nearY =
        (this.unit * frontierRho * this.cosTilt) / (1 - PERSPECTIVE * frontierRho * this.sinTilt)
      const anchored = top + availableHeight * 0.94 - nearY
      this.cy += (Math.min(this.cy, anchored) - this.cy) * rim
    }
  }

  // 返回离 (x, y) 最近的可见记忆，没有则 -1。余烬和归档让一点距离，优先选中还亮着的。
  pick(x: number, y: number, radius: number, at: number) {
    const { scene } = this
    if (!scene) return -1
    let best = -1
    let bestDistance = radius * radius
    for (let index = 0; index < scene.count; index += 1) {
      const sx = this.screenX[index]
      if (Number.isNaN(sx)) continue
      const dx = sx - x
      const dy = this.screenY[index] - y
      let distance = dx * dx + dy * dy
      if (at >= scene.replacedAt[index] || at >= scene.archivedAt[index]) distance *= 1.8
      if (distance < bestDistance) {
        bestDistance = distance
        best = index
      }
    }
    return best
  }

  screenPosition(index: number) {
    const x = this.screenX[index]
    if (x === undefined || Number.isNaN(x)) return null
    return { x, y: this.screenY[index] }
  }

  draw(frame: EvolutionFrame) {
    const { ctx, scene, dpr } = this
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    ctx.globalCompositeOperation = 'source-over'
    ctx.globalAlpha = 1
    if (this.background) ctx.drawImage(this.background, 0, 0, this.width, this.height)
    else {
      ctx.fillStyle = '#05060a'
      ctx.fillRect(0, 0, this.width, this.height)
    }
    const frontierRho = scene ? rhoAt(scene.domain, frame.at) : 1
    this.applyCamera(frontierRho)
    ctx.globalCompositeOperation = 'lighter'
    ctx.lineCap = 'round'
    ctx.lineJoin = 'round'
    this.drawDust(frame)
    if (scene) {
      this.stepMorph(scene, frame)
      this.drawDiscBody(scene, frame, frontierRho)
      this.drawNewestBand(frame, frontierRho)
      this.drawDigests(scene, frame, frontierRho)
      this.computePositions(scene, frame)
      this.drawThreads(scene, frame, frontierRho)
      this.drawStems(scene, frame, frontierRho)
      this.drawFacts(scene, frame, frontierRho)
      this.drawPulses(scene, frame)
      this.drawFrontier(frame, frontierRho)
    }
    this.applyBloom()
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    ctx.globalCompositeOperation = 'source-over'
    ctx.globalAlpha = 1
    if (scene) this.drawLabels(scene, frame, frontierRho)
    if (frame.fade < 1) {
      ctx.globalAlpha = 1 - frame.fade
      ctx.fillStyle = '#05060a'
      ctx.fillRect(0, 0, this.width, this.height)
      ctx.globalAlpha = 1
    }
  }

  private stepMorph(scene: EvolutionScene, frame: EvolutionFrame) {
    const { morphX, morphY } = this
    if (!morphX || !morphY) return
    const progress = Math.min(1, Math.max(0, (frame.wall - this.morphStart) / MORPH_MS))
    const eased = progress * progress * (3 - 2 * progress)
    for (let index = 0; index < scene.count; index += 1) {
      this.worldX[index] = morphX[index] + (scene.x[index] - morphX[index]) * eased
      this.worldY[index] = morphY[index] + (scene.y[index] - morphY[index]) * eased
    }
    if (progress >= 1) {
      this.morphX = null
      this.morphY = null
    }
  }

  // 数据刷新后的过渡是否还在进行：还在就得继续出帧（减少动效时也一样，它不会发生）。
  get morphing() {
    return this.morphX !== null
  }

  private drawDust(frame: EvolutionFrame) {
    const { ctx, dust } = this
    const count = this.quality === QUALITY_HIGH ? 200 : this.quality === QUALITY_MEDIUM ? 110 : 40
    const drift = frame.ambient ? frame.wall * 0.000012 : 0
    for (let index = 0; index < count; index += 1) {
      const offset = index * 5
      const speed = 0.55 + (index % 7) * 0.05
      const cos = Math.cos(drift * speed)
      const sin = Math.sin(drift * speed)
      const x = dust[offset] * cos - dust[offset + 1] * sin
      const y = dust[offset] * sin + dust[offset + 1] * cos
      this.project(x, y, dust[offset + 2])
      if (this.px < -4 || this.py < -4 || this.px > this.width + 4 || this.py > this.height + 4)
        continue
      const twinkle = frame.ambient
        ? 0.55 + 0.45 * Math.sin(frame.wall * 0.0007 + dust[offset + 3])
        : 0.7
      const size = (0.7 + dust[offset + 4] * 1.1) * this.ps
      ctx.globalAlpha = dust[offset + 4] * twinkle * 0.5
      ctx.fillStyle = rgbCss(index % 3 === 0 ? DUST_COOL : DUST_WARM)
      ctx.fillRect(this.px - size / 2, this.py - size / 2, size, size)
    }
    ctx.globalAlpha = 1
  }

  // 年轮本体：已经长出来的盘面微微发光，隔一圈填一层“晚材”，线条的亮度跟那一圈的活动量走。
  private drawDiscBody(scene: EvolutionScene, frame: EvolutionFrame, frontierRho: number) {
    const { ctx } = this
    const dimmed = this.selected >= 0 ? 0.55 : 1
    this.project(discX(0, 0), discY(0, 0), 0)
    const coreX = this.px
    const coreY = this.py
    // 中心的光只在开场时撑场面：长大以后它会把视线从最新的外圈抢走。
    const coreDim = 1 - 0.72 * smoothRange(frontierRho, 0.2, 0.7)
    // 盘面的底光
    const bodyRadius = this.unit * Math.max(frontierRho, 0.1) * 1.32
    ctx.globalAlpha = 0.5 * coreDim * dimmed
    ctx.drawImage(
      this.discSprite,
      this.cx - bodyRadius,
      this.cy - bodyRadius * this.cosTilt,
      bodyRadius * 2,
      bodyRadius * 2 * this.cosTilt,
    )
    // 终点圈的淡影：开场时能看出它将长到多大。
    if (frontierRho < 0.995) {
      ctx.beginPath()
      this.traceRing(this.rimPath)
      ctx.globalAlpha = 0.06
      ctx.strokeStyle = rgbCss(RING)
      ctx.lineWidth = 0.8
      ctx.stroke()
    }
    if (this.quality >= QUALITY_MEDIUM) {
      // 中档隔一根画一根：质感还在，线条少一半。
      const grainStep = this.quality === QUALITY_HIGH ? 1 : 2
      ctx.strokeStyle = rgbCss(RING)
      ctx.lineWidth = 0.5
      // 按老、新分两批画：逐根设透明度要多出上百次描边。
      const youngFrom = frontierRho * YOUNG_FROM
      for (const young of [false, true]) {
        ctx.globalAlpha = (young ? 0.075 : 0.03) * grainStep * dimmed
        ctx.beginPath()
        for (let line = 0; line < this.grainPaths.length; line += grainStep) {
          const { rho, path } = this.grainPaths[line]
          if (rho > frontierRho) break
          if (rho >= youngFrom !== young) continue
          for (let index = 0; index < GRAIN_SEGMENTS; index += 1) {
            this.project(path[index * 2], path[index * 2 + 1], 0)
            if (index === 0) ctx.moveTo(this.px, this.py)
            else ctx.lineTo(this.px, this.py)
          }
          ctx.closePath()
        }
        ctx.stroke()
      }
    }
    const { rings } = scene
    let lastVisible = -1
    for (let index = 0; index < rings.length; index += 1) {
      if (rings[index].at > frame.at) break
      lastVisible = index
    }
    ctx.fillStyle = rgbCss(RING)
    for (let index = 0; index <= lastVisible; index += 2) {
      const ring = rings[index]
      ctx.beginPath()
      this.traceRing(this.ringPaths[index])
      if (index > 0) this.traceRing(this.ringPaths[index - 1])
      ctx.globalAlpha =
        (0.012 + 0.03 * ring.activity) * dimmed * evolutionAgeFade(ring.rho, frontierRho)
      ctx.fill('evenodd')
    }
    ctx.strokeStyle = rgbCss(RING)
    for (let index = 0; index <= lastVisible; index += 1) {
      const ring = rings[index]
      ctx.beginPath()
      this.traceRing(this.ringPaths[index])
      ctx.globalAlpha =
        (0.11 + 0.17 * ring.activity + (ring.major ? 0.13 : 0)) *
        dimmed *
        evolutionAgeFade(ring.rho, frontierRho)
      ctx.lineWidth = ring.major ? 1 : 0.65
      ctx.stroke()
    }
    // 髓心：一切开始的地方，记得的事越多越亮。
    const known = this.knownCount(scene, frame.at)
    const pulse = frame.ambient ? 1 + 0.08 * Math.sin(frame.wall * 0.0011) : 1
    const coreSize = this.unit * (0.17 + 0.028 * Math.log1p(known)) * pulse
    ctx.globalAlpha = 0.62 * coreDim * dimmed
    ctx.drawImage(this.coreSprite, coreX - coreSize, coreY - coreSize, coreSize * 2, coreSize * 2)
    ctx.globalAlpha = 1
  }

  // 最新那一带亮起来，带一点呼吸：视线一进来就落在“现在”。
  private drawNewestBand(frame: EvolutionFrame, frontierRho: number) {
    const { ctx } = this
    const breath = frame.ambient ? 0.82 + 0.18 * Math.sin(frame.wall * BREATH_RATE) : 0.9
    const strength = breath * (frame.moving ? 1.2 : 1) * (this.selected >= 0 ? 0.35 : 1)
    const layers = this.quality === QUALITY_LOW ? 1 : BAND_LAYERS.length
    ctx.fillStyle = rgbCss(RING)
    this.fillRingPath(this.frontierPath, frontierRho)
    for (let layer = 0; layer < layers; layer += 1) {
      const { inner, alpha } = BAND_LAYERS[layer]
      const path = this.bandPaths[layer]
      this.fillRingPath(path, frontierRho * inner)
      ctx.beginPath()
      this.traceRing(this.frontierPath)
      this.traceRing(path)
      ctx.globalAlpha = alpha * strength
      ctx.fill('evenodd')
    }
    ctx.globalAlpha = 1
  }

  private knownCount(scene: EvolutionScene, at: number) {
    let low = 0
    let high = scene.sortedAppear.length
    while (low < high) {
      const middle = (low + high) >> 1
      if (scene.sortedAppear[middle] <= at) low = middle + 1
      else high = middle
    }
    return low
  }

  private drawDigests(scene: EvolutionScene, frame: EvolutionFrame, frontierRho: number) {
    const { ctx } = this
    if (!scene.digests.length) return
    ctx.strokeStyle = rgbCss(FRONTIER)
    ctx.lineWidth = 1.25
    const youngFrom = frontierRho * YOUNG_FROM
    for (const young of [false, true]) {
      ctx.globalAlpha = this.selected >= 0 ? 0.1 : young ? 0.34 : 0.14
      ctx.beginPath()
      for (const arc of scene.digests) {
        if (arc.at > frame.at || arc.rho >= youngFrom !== young) continue
        const steps = Math.max(4, Math.ceil(((arc.end - arc.start) / TAU) * RING_SEGMENTS))
        for (let step = 0; step <= steps; step += 1) {
          const theta = arc.start + ((arc.end - arc.start) * step) / steps
          this.project(discX(theta, arc.rho), discY(theta, arc.rho), 0)
          if (step === 0) ctx.moveTo(this.px, this.py)
          else ctx.lineTo(this.px, this.py)
        }
      }
      ctx.stroke()
    }
    ctx.globalAlpha = 1
  }

  // 第一遍只算位置：连线、光脉和拾取都要用。
  private computePositions(scene: EvolutionScene, frame: EvolutionFrame) {
    const { at, effectWindow } = frame
    const { appearAt, replacedAt, archivedAt, lift, count } = scene
    const { worldX: x, worldY: y } = this
    for (let index = 0; index < count; index += 1) {
      if (at < appearAt[index]) {
        this.screenX[index] = Number.NaN
        continue
      }
      let z = lift[index]
      const ended = Math.min(replacedAt[index], archivedAt[index])
      if (at >= ended) {
        // 被替换：从高处落回盘面；归档：继续往下沉。
        const fall = Math.min(1, (at - ended) / (effectWindow * 1.6))
        const eased = fall * fall * (3 - 2 * fall)
        z =
          at >= archivedAt[index]
            ? lift[index] * (1 - eased) - 0.035 * eased
            : lift[index] * (1 - eased)
      }
      this.liftNow[index] = z
      this.project(x[index], y[index], z)
      this.screenX[index] = this.px
      this.screenY[index] = this.py
      this.screenR[index] = this.ps
    }
  }

  private threadControl(from: number, to: number) {
    const ax = this.worldX[from]
    const ay = this.worldY[from]
    const bx = this.worldX[to]
    const by = this.worldY[to]
    const dx = bx - ax
    const dy = by - ay
    const length = Math.sqrt(dx * dx + dy * dy)
    this.project(
      (ax + bx) / 2,
      (ay + by) / 2,
      (this.liftNow[from] + this.liftNow[to]) / 2 + 0.018 + length * 0.46,
    )
  }

  // “事实改变了”的痕迹：每次替换留下一道细弧，从旧事实拱向新事实。
  private drawThreads(scene: EvolutionScene, frame: EvolutionFrame, frontierRho: number) {
    const { ctx, selected, chainMask } = this
    const { at } = frame
    ctx.strokeStyle = rgbCss(THREAD)
    ctx.lineWidth = 0.95
    const youngFrom = frontierRho * YOUNG_FROM
    for (const young of [false, true]) {
      ctx.globalAlpha = selected >= 0 ? 0.06 : young ? 0.42 : 0.16
      ctx.beginPath()
      for (const item of scene.replacements) {
        if (item.at > at) break
        if (item.to < 0 || at < scene.appearAt[item.to]) continue
        if (selected >= 0 && chainMask[item.from] && chainMask[item.to]) continue
        if (scene.rho[item.to] >= youngFrom !== young) continue
        this.threadControl(item.from, item.to)
        ctx.moveTo(this.screenX[item.from], this.screenY[item.from])
        ctx.quadraticCurveTo(this.px, this.py, this.screenX[item.to], this.screenY[item.to])
      }
      ctx.stroke()
    }
    if (selected >= 0) {
      ctx.strokeStyle = rgbCss(FRONTIER)
      ctx.lineWidth = 1.5
      ctx.globalAlpha = 0.8
      ctx.beginPath()
      for (const item of scene.replacements) {
        if (item.at > at) break
        if (item.to < 0 || at < scene.appearAt[item.to]) continue
        if (!chainMask[item.from] || !chainMask[item.to]) continue
        this.threadControl(item.from, item.to)
        ctx.moveTo(this.screenX[item.from], this.screenY[item.from])
        ctx.quadraticCurveTo(this.px, this.py, this.screenX[item.to], this.screenY[item.to])
      }
      ctx.stroke()
    }
    ctx.globalAlpha = 1
  }

  // 还在生效的记忆浮在盘面上，一根细线连回它所在的年轮。
  private drawStems(scene: EvolutionScene, frame: EvolutionFrame, frontierRho: number) {
    if (this.sinTilt < 0.2 || this.quality < QUALITY_MEDIUM) return
    if (this.quality === QUALITY_MEDIUM && scene.count > 800) return
    const { ctx, selected } = this
    const { at } = frame
    const { appearAt, replacedAt, archivedAt, count } = scene
    const { worldX: x, worldY: y } = this
    ctx.strokeStyle = rgbCss(RING)
    ctx.lineWidth = 0.6
    const youngFrom = frontierRho * YOUNG_FROM
    for (const young of [false, true]) {
      ctx.globalAlpha = selected >= 0 ? 0.05 : young ? 0.3 : 0.12
      ctx.beginPath()
      for (let index = 0; index < count; index += 1) {
        if (at < appearAt[index] || at >= replacedAt[index] || at >= archivedAt[index]) continue
        if (scene.rho[index] >= youngFrom !== young) continue
        this.project(x[index], y[index], 0)
        ctx.moveTo(this.px, this.py)
        ctx.lineTo(this.screenX[index], this.screenY[index])
      }
      ctx.stroke()
    }
    // 落在盘面上的倒影：压扁的一小团光，浮起来的感觉主要靠它。条数太多时省掉。
    if (this.quality === QUALITY_HIGH && count <= 1500 && selected < 0) {
      const squash = Math.max(0.2, this.cosTilt)
      for (let index = 0; index < count; index += 1) {
        if (at < appearAt[index] || at >= replacedAt[index] || at >= archivedAt[index]) continue
        this.project(x[index], y[index], 0)
        const width = 12 * (0.5 + scene.size[index]) * this.ps
        ctx.globalAlpha = 0.2 * evolutionAgeFade(scene.rho[index], frontierRho)
        ctx.drawImage(
          this.typeSprites[scene.typeIndex[index]],
          this.px - width / 2,
          this.py - (width * squash) / 2,
          width,
          width * squash,
        )
      }
    }
    ctx.globalAlpha = 1
  }

  private drawFacts(scene: EvolutionScene, frame: EvolutionFrame, frontierRho: number) {
    const { ctx, selected, selectedSector, chainMask, hovered } = this
    const { at, effectWindow, wall, ambient } = frame
    const { appearAt, replacedAt, archivedAt, size, brightness, phase, typeIndex, flags, count } =
      scene
    const density = Math.min(1.75, Math.max(0.74, (420 / Math.max(1, count)) ** 0.26))
    const pointScale = Math.min(1.35, Math.max(0.7, this.unit / 330)) * density
    const maxRipples = this.quality === QUALITY_HIGH ? 48 : this.quality === QUALITY_MEDIUM ? 24 : 8
    // 条数太多时，次要的小光点退化成方点：省掉绝大部分光栅化开销，这么小也看不出区别。
    const cheapBelow =
      count > 2200 || (count > 800 && this.quality < QUALITY_HIGH)
        ? 0.34
        : this.quality === QUALITY_LOW && count > 300
          ? 0.26
          : 0
    let ripples = 0
    let lates = 0
    for (let index = 0; index < count; index += 1) {
      if (at < appearAt[index]) continue
      const sx = this.screenX[index]
      const sy = this.screenY[index]
      const depth = this.screenR[index]
      let emphasis = 1
      if (selected >= 0)
        emphasis = chainMask[index] ? 1.3 : scene.sector[index] === selectedSector ? 0.5 : 0.11
      if (index === hovered) emphasis = Math.max(emphasis, 1.5)
      // 空气透视：远处暗、近处亮，比透视缩放更能让人读出纵深。
      emphasis *= 0.62 + 0.42 * Math.min(1, Math.max(0, (depth - 0.84) / 0.3))
      // 钉住的是用户说要一直记着的；选中和悬停是正在看的：这些不随年龄变暗。
      const ageFade =
        selected >= 0 || index === hovered || flags[index] & FLAG_PINNED
          ? 1
          : evolutionAgeFade(scene.rho[index], frontierRho)
      const base = (6 + 19 * size[index]) * pointScale * depth * (0.72 + 0.28 * ageFade)
      const ended = Math.min(replacedAt[index], archivedAt[index])
      if (at >= archivedAt[index]) {
        const sink = Math.min(1, (at - archivedAt[index]) / (effectWindow * 1.6))
        const drawSize = base * (0.62 + 0.25 * (1 - sink))
        ctx.globalAlpha = Math.min(1, (0.2 * ageFade + 0.4 * (1 - sink)) * emphasis)
        ctx.drawImage(this.archiveSprite, sx - drawSize / 2, sy - drawSize / 2, drawSize, drawSize)
        continue
      }
      if (at >= ended) {
        // 被替换：颜色烧成余烬，火星缓慢明灭；替换的瞬间有一次爆亮。
        const burn = Math.min(1, (at - ended) / effectWindow)
        const smolder = ambient ? 0.82 + 0.18 * Math.sin(wall * 0.0016 + phase[index]) : 0.9
        const drawSize = base * (0.86 + 0.9 * (1 - burn) * (1 - burn))
        if (burn < 1) {
          ctx.globalAlpha = Math.min(1, (1 - burn) * brightness[index] * emphasis)
          const sprite = this.spriteFor(index, typeIndex, flags)
          ctx.drawImage(sprite, sx - drawSize / 2, sy - drawSize / 2, drawSize, drawSize)
        }
        ctx.globalAlpha = Math.min(1, (0.62 * ageFade + 0.38 * (1 - burn)) * smolder * emphasis)
        ctx.drawImage(this.emberSprite, sx - drawSize / 2, sy - drawSize / 2, drawSize, drawSize)
        continue
      }
      const age = at - appearAt[index]
      const flash = age < effectWindow ? (1 - age / effectWindow) * frame.effectGain : 0
      if (size[index] < cheapBelow && flash === 0 && index !== hovered && selected < 0) {
        const dot = 1.7 * depth
        ctx.globalAlpha = Math.min(1, brightness[index] * emphasis * ageFade)
        ctx.fillStyle = this.typeCss[typeIndex[index]]
        ctx.fillRect(sx - dot / 2, sy - dot / 2, dot, dot)
        continue
      }
      // 越新呼吸幅度越大。
      const swing = 0.08 + 0.1 * ageFade
      const breath = ambient ? 1 - swing + swing * Math.sin(wall * 0.0013 + phase[index]) : 0.95
      let alpha = brightness[index] * breath * emphasis * ageFade
      if (flags[index] & FLAG_STALE) alpha *= 0.72
      const drawSize = base * (1 + 2.4 * flash * flash)
      ctx.globalAlpha = Math.min(1, alpha + flash * 0.5)
      const sprite = this.spriteFor(index, typeIndex, flags)
      ctx.drawImage(sprite, sx - drawSize / 2, sy - drawSize / 2, drawSize, drawSize)
      if (flags[index] & FLAG_PINNED) {
        const haloSize = base * 2.9
        ctx.globalAlpha = Math.min(1, 0.5 * emphasis * breath)
        ctx.drawImage(this.pinnedHalo, sx - haloSize / 2, sy - haloSize / 2, haloSize, haloSize)
      }
      if (flash > 0) {
        if (ripples < maxRipples) this.rippleQueue[ripples++] = index
        if (flags[index] & FLAG_LATE && lates < this.lateQueue.length)
          this.lateQueue[lates++] = index
      }
    }
    ctx.globalAlpha = 1
    this.drawRipples(scene, frame, ripples)
    this.drawLateBeams(scene, frame, lates)
    this.drawSelectionMarks(scene, frame)
  }

  private spriteFor(index: number, typeIndex: Uint8Array, flags: Uint8Array) {
    return flags[index] & FLAG_TAINTED ? this.untrustedSprite : this.typeSprites[typeIndex[index]]
  }

  // 诞生的涟漪：贴着盘面向外扩散。
  private drawRipples(scene: EvolutionScene, frame: EvolutionFrame, count: number) {
    if (!count) return
    const { ctx } = this
    ctx.lineWidth = 1
    for (let slot = 0; slot < count; slot += 1) {
      const index = this.rippleQueue[slot]
      const progress = (frame.at - scene.appearAt[index]) / frame.effectWindow
      if (progress < 0 || progress >= 1) continue
      this.project(this.worldX[index], this.worldY[index], 0)
      const radius = this.unit * (0.006 + 0.052 * Math.sqrt(progress)) * this.ps
      ctx.globalAlpha = (1 - progress) * (1 - progress) * 0.42 * frame.effectGain
      ctx.strokeStyle = rgbCss(TYPE_COLORS[EVOLUTION_TYPES[scene.typeIndex[index]]])
      ctx.beginPath()
      ctx.ellipse(this.px, this.py, radius, radius * Math.max(0.2, this.cosTilt), 0, 0, TAU)
      ctx.stroke()
    }
    ctx.globalAlpha = 1
  }

  // 晚知道的事：知道的那一刻，一道光从当时的前沿射回它真正发生的那一圈。
  private drawLateBeams(scene: EvolutionScene, frame: EvolutionFrame, count: number) {
    if (!count) return
    const { ctx } = this
    const frontierRho = rhoAt(scene.domain, frame.at)
    ctx.strokeStyle = rgbCss(FRONTIER)
    ctx.lineWidth = 1.2
    for (let slot = 0; slot < count; slot += 1) {
      const index = this.lateQueue[slot]
      const progress = (frame.at - scene.appearAt[index]) / frame.effectWindow
      if (progress < 0 || progress >= 1) continue
      const theta = scene.theta[index]
      this.project(discX(theta, frontierRho), discY(theta, frontierRho), 0)
      ctx.globalAlpha = (1 - progress) * 0.8 * frame.effectGain
      ctx.beginPath()
      ctx.moveTo(this.px, this.py)
      ctx.lineTo(this.screenX[index], this.screenY[index])
      ctx.stroke()
    }
    ctx.globalAlpha = 1
  }

  private drawSelectionMarks(scene: EvolutionScene, frame: EvolutionFrame) {
    const { ctx, selected, hovered, chainMask } = this
    const mark = (index: number, radius: number, alpha: number) => {
      if (index < 0 || index >= scene.count || frame.at < scene.appearAt[index]) return
      ctx.globalAlpha = alpha
      ctx.beginPath()
      ctx.arc(this.screenX[index], this.screenY[index], radius, 0, TAU)
      ctx.stroke()
    }
    ctx.strokeStyle = rgbCss(FRONTIER)
    ctx.lineWidth = 1
    if (selected >= 0) {
      for (let index = 0; index < scene.count; index += 1) {
        if (chainMask[index])
          mark(index, index === selected ? 12 : 7, index === selected ? 0.95 : 0.55)
      }
    }
    if (hovered >= 0 && hovered !== selected) mark(hovered, 9, 0.7)
    ctx.globalAlpha = 1
  }

  private drawPulseAlong(from: number, to: number, progress: number, strength: number) {
    const { ctx } = this
    this.threadControl(from, to)
    const controlX = this.px
    const controlY = this.py
    const ax = this.screenX[from]
    const ay = this.screenY[from]
    const bx = this.screenX[to]
    const by = this.screenY[to]
    const point = (u: number) => {
      const v = 1 - u
      this.px = v * v * ax + 2 * v * u * controlX + u * u * bx
      this.py = v * v * ay + 2 * v * u * controlY + u * u * by
    }
    const head = Math.min(1, progress * 1.3)
    const tail = Math.max(0, head - 0.42)
    const fade = strength * (progress > 0.75 ? (1 - progress) / 0.25 : 1)
    ctx.strokeStyle = rgbCss(PULSE)
    const segments = 9
    for (let step = 0; step < segments; step += 1) {
      const start = tail + ((head - tail) * step) / segments
      const end = tail + ((head - tail) * (step + 1)) / segments
      point(start)
      const sx = this.px
      const sy = this.py
      point(end)
      ctx.globalAlpha = Math.min(1, ((step + 1) / segments) ** 2 * fade)
      ctx.lineWidth = 0.7 + 1.5 * ((step + 1) / segments)
      ctx.beginPath()
      ctx.moveTo(sx, sy)
      ctx.lineTo(this.px, this.py)
      ctx.stroke()
    }
    point(head)
    const size = 20 * Math.min(1.2, this.unit / 300)
    ctx.globalAlpha = Math.min(1, fade)
    ctx.drawImage(this.pulseSprite, this.px - size / 2, this.py - size / 2, size, size)
  }

  // 光沿弧线从旧事实流向新事实：替换发生时必有一次；停下来时偶尔重演一条，让画面活着。
  private drawPulses(scene: EvolutionScene, frame: EvolutionFrame) {
    const { at, effectWindow, wall, ambient } = frame
    const { replacements } = scene
    if (!replacements.length) return
    const limit = this.quality === QUALITY_HIGH ? 36 : this.quality === QUALITY_MEDIUM ? 18 : 6
    let low = 0
    let high = replacements.length
    while (low < high) {
      const middle = (low + high) >> 1
      if (replacements[middle].at <= at) low = middle + 1
      else high = middle
    }
    const visibleCount = low
    const window = effectWindow * 1.8
    let drawn = 0
    for (let index = visibleCount - 1; index >= 0 && drawn < limit; index -= 1) {
      const item = replacements[index]
      const progress = (at - item.at) / window
      if (progress >= 1) break
      if (item.to < 0 || at < scene.appearAt[item.to]) continue
      if (frame.effectGain > 0.02)
        this.drawPulseAlong(item.from, item.to, progress, frame.effectGain)
      drawn += 1
    }
    if (!ambient) {
      this.ctx.globalAlpha = 1
      return
    }
    if (this.selected >= 0) {
      // 选中时沿整条版本链反复走光。
      let edge = 0
      for (const item of replacements) {
        if (item.to < 0 || item.at > at) continue
        if (!this.chainMask[item.from] || !this.chainMask[item.to]) continue
        const cycle = (((wall * 0.00045 + edge * 0.27) % 1) + 1) % 1
        this.drawPulseAlong(item.from, item.to, cycle, 0.9)
        edge += 1
      }
    } else if (visibleCount) {
      if (wall >= this.nextAmbientPulse && this.ambientPulses.length < 3) {
        this.nextAmbientPulse = wall + 650 + (Math.sin(wall) + 1) * 500
        const pick = Math.floor(Math.abs(Math.sin(wall * 12.9898)) * visibleCount) % visibleCount
        this.ambientPulses.push({ replacement: pick, start: wall })
      }
      for (let slot = this.ambientPulses.length - 1; slot >= 0; slot -= 1) {
        const pulse = this.ambientPulses[slot]
        const progress = (wall - pulse.start) / 1900
        const item = replacements[pulse.replacement]
        if (progress >= 1 || !item || item.at > at) {
          this.ambientPulses.splice(slot, 1)
          continue
        }
        if (item.to < 0 || at < scene.appearAt[item.to]) continue
        this.drawPulseAlong(item.from, item.to, progress, 0.55)
      }
    }
    this.ctx.globalAlpha = 1
  }

  // 生长前沿：最外圈就是当前演算到的时刻，时间走动时更亮。
  private drawFrontier(frame: EvolutionFrame, frontierRho: number) {
    const { ctx } = this
    const path = this.frontierPath
    ctx.strokeStyle = rgbCss(FRONTIER)
    const resting = frame.ambient ? 0.78 + 0.22 * Math.sin(frame.wall * BREATH_RATE) : 0.85
    const intensity = (frame.moving ? 1.1 : resting) * (this.selected >= 0 ? 0.5 : 1)
    const passes = this.quality === QUALITY_LOW ? 1 : 3
    for (let pass = 0; pass < passes; pass += 1) {
      ctx.beginPath()
      this.traceRing(path)
      if (pass === passes - 1) {
        ctx.lineWidth = 1.1
        ctx.globalAlpha = 0.72 * intensity
      } else if (pass === 0) {
        ctx.lineWidth = 9
        ctx.globalAlpha = 0.035 * intensity
      } else {
        ctx.lineWidth = 3.6
        ctx.globalAlpha = 0.09 * intensity
      }
      ctx.stroke()
    }
    if (frame.ambient && this.quality >= QUALITY_MEDIUM) {
      const size = Math.min(46, this.unit * 0.11) * (frame.moving ? 1.25 : 1)
      for (let glint = 0; glint < 3; glint += 1) {
        const theta = frame.wall * 0.00017 * (glint === 1 ? -1 : 1) + glint * 2.1
        this.project(discX(theta, frontierRho), discY(theta, frontierRho), 0)
        ctx.globalAlpha =
          (0.32 + 0.3 * Math.max(0, Math.sin(frame.wall * 0.0006 + glint * 2))) * intensity
        ctx.drawImage(this.pulseSprite, this.px - size / 2, this.py - size / 2, size, size)
      }
    }
    ctx.globalAlpha = 1
  }

  // 降采样再放大当泛光。比 ctx.filter 模糊便宜得多，Safari 也能用。
  private applyBloom() {
    const { ctx, canvas } = this
    if (this.quality < QUALITY_MEDIUM || !this.bloomA) return
    const first = this.bloomA.getContext('2d')
    if (!first) return
    first.globalCompositeOperation = 'copy'
    first.imageSmoothingEnabled = true
    first.drawImage(canvas, 0, 0, this.bloomA.width, this.bloomA.height)
    ctx.setTransform(1, 0, 0, 1, 0, 0)
    ctx.imageSmoothingEnabled = true
    ctx.globalCompositeOperation = 'lighter'
    ctx.globalAlpha = 0.34
    ctx.drawImage(this.bloomA, 0, 0, canvas.width, canvas.height)
    if (this.quality >= QUALITY_HIGH && this.bloomB) {
      const second = this.bloomB.getContext('2d')
      if (second) {
        second.globalCompositeOperation = 'copy'
        second.imageSmoothingEnabled = true
        second.drawImage(this.bloomA, 0, 0, this.bloomB.width, this.bloomB.height)
        ctx.globalAlpha = 0.3
        ctx.drawImage(this.bloomB, 0, 0, canvas.width, canvas.height)
      }
    }
    ctx.globalAlpha = 1
  }

  // 年轮上的日期固定在屏幕右下方向，不跟着自转；主题名绕在外圈，转到近处才清楚。
  private drawLabels(scene: EvolutionScene, frame: EvolutionFrame, frontierRho: number) {
    const { ctx } = this
    if (this.width < 280 || !scene.count) return
    ctx.textBaseline = 'middle'
    ctx.textAlign = 'left'
    ctx.font = '10.5px ui-sans-serif, system-ui, sans-serif'
    const dim = this.selected >= 0 ? 0.45 : 1
    const fixedAngle = Math.PI * 0.2 - this.camera.yaw
    let lastX = Infinity
    let lastY = Infinity
    for (let index = scene.rings.length - 1; index >= 0; index -= 1) {
      const ring = scene.rings[index]
      if (ring.at > frame.at || !ring.major) continue
      this.project(discX(fixedAngle, ring.rho), discY(fixedAngle, ring.rho), 0)
      const spacing = Math.hypot(this.px - lastX, this.py - lastY)
      if (spacing < 34) continue
      lastX = this.px
      lastY = this.py
      ctx.globalAlpha = 0.62 * dim * evolutionAgeFade(ring.rho, frontierRho)
      ctx.fillStyle = rgbCss(RING)
      const text = this.ringLabels[index] || ''
      // 贴着右边缘时往回收，不让日期被裁掉。
      ctx.fillText(
        text,
        Math.min(this.px + 4, this.width - 8 - ctx.measureText(text).width),
        this.py,
      )
    }
    const labelRho = Math.min(1.09, frontierRho + 0.07)
    const arcRho = Math.min(1.14, frontierRho + 0.12)
    if (this.width >= 560 && frontierRho > 0.32) {
      ctx.textAlign = 'center'
      for (const sectorIndex of this.sectorOrder) {
        const sector = scene.sectors[sectorIndex]
        const middle = (sector.start + sector.end) / 2
        // 主题名就是空间名的一部分时（acme 和 acme-web）不必再写一遍。
        if (scene.spaceArcs.some((arc) => arc.name.includes(sector.label))) continue
        this.project(discX(middle, labelRho), discY(middle, labelRho), 0)
        const nearness = Math.min(1, Math.max(0, (this.ps - 0.86) / 0.24))
        let alpha = (0.14 + 0.5 * nearness) * Math.min(1, (frontierRho - 0.32) * 4)
        if (this.selected >= 0) alpha = sectorIndex === this.selectedSector ? 0.95 : 0.1
        ctx.globalAlpha = alpha
        ctx.fillStyle = rgbCss(RING)
        ctx.fillText(sector.label.slice(0, 18), this.px, this.py)
      }
    }
    // 全部空间时，外圈标出每个空间占的那一段。
    if (scene.spaceArcs.length) {
      ctx.textAlign = 'center'
      for (const arc of scene.spaceArcs) {
        const steps = Math.max(6, Math.ceil(((arc.end - arc.start) / TAU) * RING_SEGMENTS))
        ctx.beginPath()
        for (let step = 0; step <= steps; step += 1) {
          const theta = arc.start + ((arc.end - arc.start) * step) / steps
          this.project(discX(theta, arcRho), discY(theta, arcRho), 0)
          if (step === 0) ctx.moveTo(this.px, this.py)
          else ctx.lineTo(this.px, this.py)
        }
        ctx.strokeStyle = rgbCss(RING)
        ctx.lineWidth = 1
        ctx.globalAlpha = 0.34 * dim
        ctx.stroke()
        const middle = (arc.start + arc.end) / 2
        this.project(discX(middle, arcRho + 0.035), discY(middle, arcRho + 0.035), 0)
        ctx.globalAlpha = 0.72 * dim
        ctx.fillStyle = rgbCss(FRONTIER)
        ctx.fillText(arc.name.slice(0, 22), this.px, this.py)
      }
    }
    ctx.globalAlpha = 1
  }
}

// 时间轴上方的活动密度图：已经演算过的部分亮，未来的部分暗；替换事件是一根余烬色的短线。
export function drawEvolutionHistogram(
  canvas: HTMLCanvasElement,
  scene: EvolutionScene,
  at: number,
  width: number,
  height: number,
) {
  const context = canvas.getContext('2d')
  if (!context) return
  const device = typeof devicePixelRatio === 'number' ? devicePixelRatio : 1
  const dpr = Math.min(device || 1, 2)
  const pixelWidth = Math.max(1, Math.round(width * dpr))
  const pixelHeight = Math.max(1, Math.round(height * dpr))
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
    canvas.width = pixelWidth
    canvas.height = pixelHeight
  }
  context.setTransform(dpr, 0, 0, dpr, 0, 0)
  context.clearRect(0, 0, width, height)
  const { histogram, histogramReplacements, domain } = scene
  const bins = histogram.length
  const span = Math.max(1, domain.end - domain.start)
  const playhead = Math.min(1, Math.max(0, (at - domain.start) / span)) * width
  let peak = 1
  for (let index = 0; index < bins; index += 1) peak = Math.max(peak, histogram[index])
  const barWidth = width / bins
  const usable = height - 5
  for (let index = 0; index < bins; index += 1) {
    const value = histogram[index]
    const x = index * barWidth
    const played = x + barWidth / 2 <= playhead
    if (value) {
      const barHeight = Math.max(1.5, Math.sqrt(value / peak) * usable)
      context.fillStyle = played ? rgbCss(RING, 0.78) : rgbCss(RING, 0.2)
      context.fillRect(x + 0.5, height - barHeight, Math.max(1, barWidth - 1), barHeight)
    }
    if (histogramReplacements[index]) {
      const size = Math.min(usable, 3 + histogramReplacements[index] * 1.5)
      context.fillStyle = played ? rgbCss(EMBER, 0.95) : rgbCss(EMBER, 0.34)
      context.fillRect(x + barWidth / 2 - 0.75, 0, 1.5, size)
    }
  }
}
