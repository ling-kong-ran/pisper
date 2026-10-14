// 语音对话主视觉：经纬网格点云球（纯 Canvas 2D，零依赖）。
// 视觉参考 AsLive：密集点阵构成球面、极点处的漩涡纹理、青/品红噪声斑块；
// 开口瞬间音量尖峰让表面炸起触手。立体感来自透视投影 + 深度衰减 +
// 背侧点阵透出的格栅纵深（点阵稀疏无需深度剔除）。
import { useEffect, useRef } from 'react'
import type { VoiceModeStage } from './voice-mode-state'

type StageParams = {
  amp: number // 表面噪声形变幅度
  freq: number // 噪声空间频率
  speed: number // 噪声时间流速
  spike: number // 音量尖峰强度
  rot: number // 自转角速度（rad/s）
  dim: number
  behavior: 'calm' | 'reactive' | 'converge' | 'swirl' | 'beat' | 'jitter'
}

const STAGE_PARAMS: Record<VoiceModeStage, StageParams> = {
  idle: { amp: 0.14, freq: 1.7, speed: 0.22, spike: 0, rot: 0.22, dim: 0.8, behavior: 'calm' },
  requesting: {
    amp: 0.08,
    freq: 1.6,
    speed: 0.3,
    spike: 0,
    rot: 0.4,
    dim: 0.5,
    behavior: 'calm',
  },
  listening: {
    amp: 0.15,
    freq: 2,
    speed: 0.55,
    spike: 0.55,
    rot: 0.4,
    dim: 1,
    behavior: 'reactive',
  },
  transcribing: {
    amp: 0.08,
    freq: 1.8,
    speed: 1,
    spike: 0,
    rot: 1.5,
    dim: 0.95,
    behavior: 'converge',
  },
  thinking: {
    amp: 0.2,
    freq: 2.3,
    speed: 0.8,
    spike: 0,
    rot: 0.85,
    dim: 1,
    behavior: 'swirl',
  },
  speaking: {
    amp: 0.13,
    freq: 1.9,
    speed: 0.5,
    spike: 0.1,
    rot: 0.35,
    dim: 1,
    behavior: 'beat',
  },
  error: {
    amp: 0.12,
    freq: 2.2,
    speed: 1.2,
    spike: 0,
    rot: 0.1,
    dim: 0.9,
    behavior: 'jitter',
  },
}

// 经纬网格：纬度环 × 每环经度点。极点处点自然聚拢成漩涡——这是该视觉的签名。
const LAT_RINGS = 64
const LON_PER_RING = 96
const CAMERA_Z = 2.6
const TAU = Math.PI * 2

// 点的基色（暗色主题）：白为主，噪声斑块染青/品红。
const WHITE: [number, number, number] = [223, 233, 255]
const CYAN: [number, number, number] = [89, 230, 255]
const MAGENTA: [number, number, number] = [255, 95, 210]

// 亮色主题配色：深色点阵 + 更深的青/品红，普通混合压在浅底上
// （加色混合在白底上会爆白，必须随主题切换混合模式）。
type ThemePalette = {
  base: [number, number, number]
  cyan: [number, number, number]
  magenta: [number, number, number]
  haloInner: [number, number, number]
  haloMid: [number, number, number]
  haloAlpha: number
  composite: GlobalCompositeOperation
  alphaBase: number
  alphaFront: number
}
const THEME_PALETTES: Record<'dark' | 'light', ThemePalette> = {
  dark: {
    base: WHITE,
    cyan: CYAN,
    magenta: MAGENTA,
    haloInner: [70, 110, 170],
    haloMid: [50, 80, 140],
    haloAlpha: 0.1,
    composite: 'lighter',
    alphaBase: 0.1,
    alphaFront: 0.68,
  },
  light: {
    base: [34, 44, 76],
    cyan: [6, 132, 180],
    magenta: [176, 48, 150],
    haloInner: [110, 140, 210],
    haloMid: [130, 160, 220],
    haloAlpha: 0.16,
    composite: 'source-over',
    alphaBase: 0.18,
    alphaFront: 0.85,
  },
}

function hash3(x: number, y: number, z: number) {
  const n = Math.sin(x * 127.1 + y * 311.7 + z * 74.7) * 43758.5453123
  return n - Math.floor(n)
}

function smoothstep(t: number) {
  return t * t * (3 - 2 * t)
}

// 轻量 3D 值噪声：哈希晶格 + 三线性平滑插值。
function noise3(x: number, y: number, z: number) {
  const xi = Math.floor(x)
  const yi = Math.floor(y)
  const zi = Math.floor(z)
  const xf = smoothstep(x - xi)
  const yf = smoothstep(y - yi)
  const zf = smoothstep(z - zi)
  let value = 0
  for (let dx = 0; dx <= 1; dx += 1) {
    for (let dy = 0; dy <= 1; dy += 1) {
      for (let dz = 0; dz <= 1; dz += 1) {
        value +=
          (dx ? xf : 1 - xf) *
          (dy ? yf : 1 - yf) *
          (dz ? zf : 1 - zf) *
          hash3(xi + dx, yi + dy, zi + dz)
      }
    }
  }
  return value
}

function easeOutBack(t: number) {
  const c1 = 1.70158
  const c3 = c1 + 1
  return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2)
}

function heartbeat(t: number) {
  const phase = (t * 0.9) % 1
  return (
    Math.exp(-Math.pow((phase - 0.12) * 9, 2)) + 0.45 * Math.exp(-Math.pow((phase - 0.34) * 9, 2))
  )
}

export function DigitalOrb({
  stage,
  level,
  size = 380,
  dark = true,
}: {
  stage: VoiceModeStage
  level: number
  size?: number
  dark?: boolean
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  // 高频状态走 ref，rAF 循环内部读取，避免每帧重建动画闭包。
  const stageRef = useRef(stage)
  const levelRef = useRef(level)
  const darkRef = useRef(dark)
  stageRef.current = stage
  levelRef.current = level
  darkRef.current = dark

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return undefined
    const ctx = canvas.getContext('2d')
    if (!ctx) return undefined

    const dpr = Math.min(window.devicePixelRatio || 1, 2)
    canvas.width = size * dpr
    canvas.height = size * dpr
    ctx.scale(dpr, dpr)

    // 预生成经纬点阵的球面坐标（含每点的颜色噪声相位偏移）。
    const latSin = new Float32Array(LAT_RINGS)
    const latCos = new Float32Array(LAT_RINGS)
    for (let la = 0; la < LAT_RINGS; la += 1) {
      const phi = -Math.PI / 2 + ((la + 0.5) / LAT_RINGS) * Math.PI
      latSin[la] = Math.sin(phi)
      latCos[la] = Math.cos(phi)
    }
    const lonCos = new Float32Array(LON_PER_RING)
    const lonSin = new Float32Array(LON_PER_RING)
    for (let lo = 0; lo < LON_PER_RING; lo += 1) {
      const theta = (lo / LON_PER_RING) * TAU
      lonCos[lo] = Math.cos(theta)
      lonSin[lo] = Math.sin(theta)
    }

    const center = size / 2
    const baseRadius = size * 0.31
    const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches

    let raf = 0
    let last = performance.now()
    const born = last
    let clock = 0
    let rotY = 0
    let current = { ...STAGE_PARAMS[stageRef.current] }
    let smoothLevel = 0
    let prevLevel = 0
    let punch = 0 // 开口瞬间的「弹跳」响应量

    const frame = (now: number) => {
      const dt = Math.min((now - last) / 1000, 0.05)
      last = now
      const target = STAGE_PARAMS[stageRef.current]
      const ease = Math.min(1, dt * 5)
      current = {
        ...target,
        amp: current.amp + (target.amp - current.amp) * ease,
        freq: current.freq + (target.freq - current.freq) * ease,
        speed: current.speed + (target.speed - current.speed) * ease,
        spike: current.spike + (target.spike - current.spike) * ease,
        rot: current.rot + (target.rot - current.rot) * ease,
        dim: current.dim + (target.dim - current.dim) * ease,
      }
      const speedScale = reducedMotion ? 0.35 : 1
      clock += dt * speedScale
      rotY += current.rot * dt * speedScale
      smoothLevel += (levelRef.current - smoothLevel) * Math.min(1, dt * 14)
      // 开口就响应：检测音量上升沿，脉冲注入形变与亮度。
      const onset = Math.max(0, smoothLevel - prevLevel - 0.02)
      prevLevel = smoothLevel
      punch = Math.min(0.6, punch * Math.pow(0.02, dt) + onset * 2.4)

      let energy = smoothLevel
      if (current.behavior === 'beat') {
        energy = Math.max(
          0.08,
          0.4 +
            0.28 * Math.abs(Math.sin(clock * 5.2)) * Math.sin(clock * 1.6) +
            0.12 * Math.sin(clock * 9.1),
        )
      } else if (current.behavior === 'converge') {
        energy = 0.18
      } else if (current.behavior === 'jitter') {
        energy = 0.3 + 0.2 * Math.sin(clock * 17)
      } else if (current.behavior !== 'reactive') {
        energy = 0.1 + 0.05 * Math.sin(clock * 1.3)
      }

      // rAF 帧时间戳可能略早于 effect 内取的 performance.now()，t 必须钳位。
      const birth = easeOutBack(Math.min(1, Math.max(0, (now - born) / 900)))
      const beat = current.behavior === 'beat' ? energy : heartbeat(clock) * 0.5 + energy * 0.5
      let pulse = birth * (1 + beat * 0.04 + punch * 0.1)
      if (current.behavior === 'converge') pulse *= 0.9 + 0.1 * Math.sin(clock * 4.2)

      ctx.clearRect(0, 0, size, size)
      const palette = THEME_PALETTES[darkRef.current ? 'dark' : 'light']

      // 中央暗晕：让点阵球在页面上有「悬浮光团」的底色。
      const halo = ctx.createRadialGradient(center, center, 0, center, center, baseRadius * 1.7)
      halo.addColorStop(
        0,
        `rgba(${palette.haloInner[0]},${palette.haloInner[1]},${palette.haloInner[2]},${(palette.haloAlpha * current.dim).toFixed(3)})`,
      )
      halo.addColorStop(
        0.6,
        `rgba(${palette.haloMid[0]},${palette.haloMid[1]},${palette.haloMid[2]},${(palette.haloAlpha * 0.5 * current.dim).toFixed(3)})`,
      )
      halo.addColorStop(1, 'rgba(0,0,0,0)')
      ctx.fillStyle = halo
      ctx.fillRect(0, 0, size, size)

      const sinY = Math.sin(rotY)
      const cosY = Math.cos(rotY)
      const tilt = 0.38
      const sinX = Math.sin(tilt)
      const cosX = Math.cos(tilt)
      const nt = clock * current.speed
      const freq = current.freq
      const amp = current.amp * pulse
      const spike = current.spike * (energy + punch)
      const R = baseRadius * pulse
      const jitterTick = Math.floor(clock * 15)
      const brightness = current.dim * (0.72 + 0.5 * beat) + punch * 0.3

      ctx.globalCompositeOperation = palette.composite
      for (let la = 0; la < LAT_RINGS; la += 1) {
        const ringSin = latSin[la]
        const ringCos = latCos[la]
        for (let lo = 0; lo < LON_PER_RING; lo += 1) {
          const px = ringCos * lonCos[lo]
          const py = ringSin
          const pz = ringCos * lonSin[lo]
          // 表面形变：基础噪声 + 聆听态的音量尖峰触手。
          const n = noise3(px * freq + nt, py * freq + nt * 0.7, pz * freq) - 0.5
          let r = 1 + n * 2 * amp
          if (current.behavior === 'reactive') r += Math.pow(Math.max(n, 0), 2) * 1.7 * spike
          else if (current.behavior === 'swirl') r += 0.03 * Math.sin(9 * px + clock * 4.6)
          else if (current.behavior === 'jitter')
            r += 0.02 * (hash3(la * LON_PER_RING + lo, jitterTick, 3) - 0.5)
          const x = px * r
          const y = py * r
          const z = pz * r
          // 绕 Y 自转 + 绕 X 固定俯仰（单位球空间），透视投影后再放大到像素。
          const rx = x * cosY - z * sinY
          const rz = x * sinY + z * cosY
          const ry = y * cosX - rz * sinX
          const rz2 = y * sinX + rz * cosX
          const perspective = CAMERA_Z / (CAMERA_Z - rz2)
          const sx = center + rx * perspective * R
          const sy = center + ry * perspective * R
          const front = (rz2 + 1) / 2 // 0 背侧 → 1 前侧
          // 颜色斑块：低频噪声染青/品红，其余取基色；背侧压暗营造体积。
          const patch = noise3(px * 1.25 + 31, py * 1.25 + nt * 0.12, pz * 1.25 - 17)
          let color = palette.base
          if (patch > 0.62) color = palette.magenta
          else if (patch < 0.38) color = palette.cyan
          const alpha = (palette.alphaBase + palette.alphaFront * front * front) * brightness
          if (alpha < 0.015) continue
          const dot = (1 + 1.4 * front + spike * 0.4) * (0.55 + 0.45 * perspective)
          ctx.fillStyle = `rgba(${color[0]},${color[1]},${color[2]},${alpha.toFixed(3)})`
          ctx.fillRect(sx, sy, dot, dot)
        }
      }
      ctx.globalCompositeOperation = 'source-over'

      raf = requestAnimationFrame(frame)
    }

    raf = requestAnimationFrame(frame)
    return () => cancelAnimationFrame(raf)
  }, [size])

  return (
    <canvas
      ref={canvasRef}
      className="pointer-events-none block"
      style={{ width: size, height: size }}
      aria-hidden="true"
    />
  )
}
