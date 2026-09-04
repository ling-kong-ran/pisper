// 输入框快捷栏的稳定布局模型：偏好只保存工具 ID，避免把会话状态或 React 节点写入存储。
export const COMPOSER_TOOL_IDS = [
  'attachment',
  'resource',
  'visual',
  'model',
  'permission',
  'run-mode',
  'thinking',
  'commands',
  'git-changes',
  'compact-context',
  'session-actions',
] as const

export type ComposerToolId = (typeof COMPOSER_TOOL_IDS)[number]
export type ComposerToolLocation = 'inline' | 'overflow'

export type ComposerToolbarLayout = {
  inline: ComposerToolId[]
  overflow: ComposerToolId[]
}

export type ComposerToolbarAllocation = {
  inline: ComposerToolId[]
  overflow: ComposerToolId[]
  automaticallyOverflowed: ComposerToolId[]
}

const COMPOSER_TOOL_ID_SET = new Set<string>(COMPOSER_TOOL_IDS)

export const DEFAULT_COMPOSER_TOOLBAR_LAYOUT: ComposerToolbarLayout = {
  inline: [...COMPOSER_TOOL_IDS],
  overflow: [],
}

function validToolIds(value: unknown): ComposerToolId[] {
  if (!Array.isArray(value)) return []
  return value.filter(
    (item, index, items): item is ComposerToolId =>
      typeof item === 'string' && COMPOSER_TOOL_ID_SET.has(item) && items.indexOf(item) === index,
  )
}

// 旧版本、损坏数据和新增工具都在这里归一，保证每个已知工具恰好出现一次。
export function normalizeComposerToolbarLayout(value: unknown): ComposerToolbarLayout {
  const stored = value && typeof value === 'object' ? (value as Partial<ComposerToolbarLayout>) : {}
  const inline = validToolIds(stored.inline)
  const inlineSet = new Set(inline)
  const overflow = validToolIds(stored.overflow).filter((id) => !inlineSet.has(id))
  const known = new Set([...inline, ...overflow])

  for (const id of COMPOSER_TOOL_IDS) {
    if (!known.has(id)) inline.push(id)
  }

  return { inline, overflow }
}

export function setComposerToolLocation(
  layout: ComposerToolbarLayout,
  id: ComposerToolId,
  location: ComposerToolLocation,
): ComposerToolbarLayout {
  const normalized = normalizeComposerToolbarLayout(layout)
  const inline = normalized.inline.filter((toolId) => toolId !== id)
  const overflow = normalized.overflow.filter((toolId) => toolId !== id)
  if (location === 'inline') inline.push(id)
  else overflow.push(id)
  return { inline, overflow }
}

export function moveComposerTool(
  layout: ComposerToolbarLayout,
  id: ComposerToolId,
  direction: -1 | 1,
): ComposerToolbarLayout {
  const normalized = normalizeComposerToolbarLayout(layout)
  const location = normalized.inline.includes(id) ? 'inline' : 'overflow'
  const target = [...normalized[location]]
  const index = target.indexOf(id)
  const nextIndex = index + direction
  if (index < 0 || nextIndex < 0 || nextIndex >= target.length) return normalized
  ;[target[index], target[nextIndex]] = [target[nextIndex], target[index]]
  return { ...normalized, [location]: target }
}

// 用户主动收纳的工具始终留在收纳区；空间不足时再从常驻区尾部临时回退。
export function allocateComposerToolbar(
  layout: ComposerToolbarLayout,
  availableToolIds: readonly ComposerToolId[],
  inlineCapacity: number,
): ComposerToolbarAllocation {
  const normalized = normalizeComposerToolbarLayout(layout)
  const available = new Set(availableToolIds)
  const preferredInline = normalized.inline.filter((id) => available.has(id))
  const preferredOverflow = normalized.overflow.filter((id) => available.has(id))
  const capacity = Number.isFinite(inlineCapacity)
    ? Math.max(0, Math.floor(inlineCapacity))
    : preferredInline.length
  const inline = preferredInline.slice(0, capacity)
  const automaticallyOverflowed = preferredInline.slice(capacity)

  return {
    inline,
    automaticallyOverflowed,
    overflow: [...automaticallyOverflowed, ...preferredOverflow],
  }
}
