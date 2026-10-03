// 星系布局算法：节点按类型分布到螺旋轨道上。
import { MAX_STARS, GOLDEN_ANGLE, GALAXY_VIEW } from './memory-galaxy-constants'
import { STAR_COLORS, STAR_GLOWS } from './memory-galaxy-constants'
import type { GalaxyPoint, GalaxyStar, MemoryNode } from './memory-types'

export function hashSeed(text: string) {
  let hash = 0
  for (let index = 0; index < text.length; index += 1) {
    hash = (hash * 31 + text.charCodeAt(index)) & 0x7fffffff
  }
  return hash
}

// 根据空间 ID 稳定排序节点，再按黄金角螺旋分配坐标，避免重排时闪烁。
export function galaxyLayout(nodes: MemoryNode[], spaceId: string): GalaxyStar[] {
  const visible = nodes.filter((node) => node.spaceId === spaceId).slice(0, MAX_STARS)
  return visible.map((node, index) => {
    const seed = hashSeed(node.id + spaceId)
    const angle = index * GOLDEN_ANGLE + (seed % 100) * 0.001
    const distance = 20 + (index / Math.max(visible.length - 1, 1)) * 80
    const x = GALAXY_VIEW.cx + Math.cos(angle) * (distance / 100) * (GALAXY_VIEW.width / 2 - 20)
    const y = GALAXY_VIEW.cy + Math.sin(angle) * (distance / 100) * (GALAXY_VIEW.height / 2 - 16)
    return { node, x, y, twinkle: 1.5 + (seed % 30) / 10 }
  })
}

export function linkCurve(source: GalaxyPoint, target: GalaxyPoint, seed: number) {
  const mx = (source.x + target.x) / 2
  const my = (source.y + target.y) / 2
  const dx = target.x - source.x
  const dy = target.y - source.y
  const bend = (seed % 40 - 20) / 200
  const cx = mx + -dy * bend
  const cy = my + dx * bend
  return `M ${source.x} ${source.y} Q ${cx} ${cy} ${target.x} ${target.y}`
}

export function starColor(type: MemoryNode['type']) {
  return STAR_COLORS[type]
}

export function starGlow(type: MemoryNode['type']) {
  return STAR_GLOWS[type]
}
