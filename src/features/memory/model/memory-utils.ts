// 记忆页纯逻辑：标签和时间格式化。
type Translate = (message: string, values?: Record<string, unknown>) => string
import type { MemorySpace, MemoryType } from './memory-types'
import { MEMORY_TYPES } from './memory-galaxy-constants'

export function memoryTypeLabel(type: MemoryType, t: Translate) {
  if (type === 'file') return t('memory:memoryPage.fileType')
  if (type === 'risk') return t('memory:memoryPage.riskType')
  if (type === 'preference') return t('memory:memoryPage.preferenceType')
  if (type === 'decision') return t('memory:memoryPage.decisionType')
  if (type === 'fact') return t('memory:memoryPage.factType')
  if (type === 'task') return t('memory:memoryPage.taskType')
  return t('memory:memoryPage.conceptType')
}

export function formatMemoryTime(value: string | undefined, locale = 'zh-CN') {
  if (!value) return ''
  try {
    return new Intl.DateTimeFormat(locale, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    }).format(new Date(value))
  } catch {
    return ''
  }
}

export function spaceLabel(
  space: MemorySpace | null | undefined,
  t: Translate = (value) => value,
) {
  if (!space) return ''
  if (space.kind === 'global') return t('memory:memoryPage.globalSpace')
  return space.name
}

export { MEMORY_TYPES }
