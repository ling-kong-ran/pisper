// 输入框快捷栏的稳定布局模型：偏好只保存工具 ID，避免把会话状态或 React 节点写入存储。
export const COMPOSER_TOOL_IDS = [
  'attachment',
  'resource',
  'visual',
  'model',
  'permission',
  'run-mode',
  'commands',
  'compact-context',
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
const RESTORED_INLINE_TOOL_IDS = new Set<ComposerToolId>(['model', 'permission', 'run-mode'])

export const DEFAULT_COMPOSER_TOOLBAR_LAYOUT: ComposerToolbarLayout = {
  inline: ['permission', 'run-mode', 'model'],
  overflow: COMPOSER_TOOL_IDS.filter((id) => !['permission', 'run-mode', 'model'].includes(id)),
}

function validToolIds(value: unknown): ComposerToolId[] {
  if (!Array.isArray(value)) return []
  return value.filter(
    (item, index, items): item is ComposerToolId =>
      typeof item === 'string' && COMPOSER_TOOL_ID_SET.has(item) && items.indexOf(item) === index,
  )
}

// 旧版本、损坏数据和新增工具都在这里归一；移除的入口自动过滤，不改变其余工具的位置。
export function normalizeComposerToolbarLayout(value: unknown): ComposerToolbarLayout {
  const stored = value && typeof value === 'object' ? (value as Partial<ComposerToolbarLayout>) : {}
  if (!Array.isArray(stored.inline) && !Array.isArray(stored.overflow)) {
    return {
      inline: [...DEFAULT_COMPOSER_TOOLBAR_LAYOUT.inline],
      overflow: [...DEFAULT_COMPOSER_TOOLBAR_LAYOUT.overflow],
    }
  }
  const inline = validToolIds(stored.inline)
  const inlineSet = new Set(inline)
  const overflow = validToolIds(stored.overflow).filter((id) => !inlineSet.has(id))
  const known = new Set([...inline, ...overflow])

  for (const id of COMPOSER_TOOL_IDS) {
    if (known.has(id)) continue
    // 这三项曾被强制移到输入框外；恢复可配置按钮时保持可见，不改动已有工具位置。
    if (RESTORED_INLINE_TOOL_IDS.has(id)) inline.push(id)
    else overflow.push(id)
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

export function setAllComposerToolsLocation(
  layout: ComposerToolbarLayout,
  location: ComposerToolLocation,
): ComposerToolbarLayout {
  const normalized = normalizeComposerToolbarLayout(layout)
  const tools = [...normalized.inline, ...normalized.overflow]
  return location === 'inline' ? { inline: tools, overflow: [] } : { inline: [], overflow: tools }
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
  toolWidths: Partial<Record<ComposerToolId, number>> = {},
): ComposerToolbarAllocation {
  const normalized = normalizeComposerToolbarLayout(layout)
  const available = new Set(availableToolIds)
  const preferredInline = normalized.inline.filter((id) => available.has(id))
  const preferredOverflow = normalized.overflow.filter((id) => available.has(id))
  const capacity = Number.isFinite(inlineCapacity)
    ? Math.max(0, inlineCapacity)
    : Number.POSITIVE_INFINITY
  let used = 0
  let count = 0
  for (const id of preferredInline) {
    const width = toolWidths[id] ?? 1
    if (used + width > capacity) break
    used += width
    count += 1
  }
  const inline = preferredInline.slice(0, count)
  const automaticallyOverflowed = preferredInline.slice(count)

  return {
    inline,
    automaticallyOverflowed,
    overflow: [...automaticallyOverflowed, ...preferredOverflow],
  }
}
