// 星系渲染常量：视图尺寸和类型颜色映射。
import type { MemoryType } from './memory-types'

export const GALAXY_VIEW = { width: 600, height: 420, cx: 300, cy: 206 }
export const MAX_STARS = 24
export const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5))

export const MEMORY_TYPES: MemoryType[] = [
  'concept',
  'file',
  'risk',
  'preference',
  'decision',
  'fact',
  'task',
]

export const STAR_COLORS: Record<MemoryType, string> = {
  concept: 'var(--g-concept)',
  file: 'var(--g-file)',
  risk: 'var(--g-risk)',
  preference: 'var(--g-preference)',
  decision: 'var(--g-decision)',
  fact: 'var(--g-fact)',
  task: 'var(--g-task)',
}

export const STAR_GLOWS: Record<MemoryType, string> = {
  concept: 'var(--g-concept-glow)',
  file: 'var(--g-file-glow)',
  risk: 'var(--g-risk-glow)',
  preference: 'var(--g-preference-glow)',
  decision: 'var(--g-decision-glow)',
  fact: 'var(--g-fact-glow)',
  task: 'var(--g-task-glow)',
}
