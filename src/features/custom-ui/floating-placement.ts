export type FloatingPlacement = { x: number; y: number }
export type FloatingSize = { width: number; height: number }
export type SavedFloatingPlacement = { key: string; position: FloatingPlacement }

export const FLOATING_PLACEMENT_LIMIT = 128

const clamp = (value: number) => Math.min(1, Math.max(0, value))
const extent = (value: number) => (Number.isFinite(value) ? Math.max(0, value) : 0)

// 没有保存位置的浮窗停在可用区的右下角、依次向上叠，一列放不下再向左错开一列（评审返修）：
// 页头、全部代理第一屏的“需要你”、工作台页头与提示词卡片都在上方，右下角最少挡主要内容（规格 6.1）。
// 原来沿用专注岛的“页头中线下方居中”，组件浮窗比岛大得多，默认就压在需要你的第一行上。
// 底部再让出输入框区（运行条 + 输入框约 140px）：贴底时正好压住发送键与模型选择（A7 截图核对时发现）。
// 盖住可滚动的对话正文比盖住操作控件代价小；可用高度不足时退到顶部，不出界。
const FLOATING_INSET = 8
const FLOATING_GAP = 12
const FLOATING_BOTTOM_RESERVE = 160

export function defaultFloatingPlacement(
  bounds: FloatingSize,
  widget: FloatingSize,
  stackIndex = 0,
): FloatingPlacement {
  const width = Math.max(0, extent(bounds.width) - extent(widget.width))
  const height = Math.max(0, extent(bounds.height) - extent(widget.height))
  const base = Math.max(0, height - FLOATING_BOTTOM_RESERVE)
  const usable = Math.max(0, base - FLOATING_INSET)
  const stride = extent(widget.height) + FLOATING_GAP
  const perColumn = stride ? Math.floor(usable / stride) + 1 : 1
  const index = Math.max(0, Math.floor(stackIndex))
  const column = Math.floor(index / perColumn)
  const row = index % perColumn
  const x = width - FLOATING_INSET - column * (extent(widget.width) + FLOATING_GAP)
  const y = base - row * stride
  return {
    x: width ? clamp(x / width) : 1,
    y: height ? clamp(y / height) : 1,
  }
}

export function floatingPlacementKey(nodeId: string, mobile: boolean): string {
  return `${mobile ? 'mobile' : 'desktop'}:${nodeId}`
}

export function floatingPositionPixels(
  position: FloatingPlacement,
  bounds: FloatingSize,
  island: FloatingSize,
): FloatingPlacement {
  return {
    x: clamp(position.x) * Math.max(0, extent(bounds.width) - extent(island.width)),
    y: clamp(position.y) * Math.max(0, extent(bounds.height) - extent(island.height)),
  }
}

export function moveFloatingPlacement(
  position: FloatingPlacement,
  delta: FloatingPlacement,
  bounds: FloatingSize,
  island: FloatingSize,
): FloatingPlacement {
  const pixels = floatingPositionPixels(position, bounds, island)
  const width = Math.max(0, extent(bounds.width) - extent(island.width))
  const height = Math.max(0, extent(bounds.height) - extent(island.height))
  return {
    x: width ? clamp((pixels.x + delta.x) / width) : clamp(position.x),
    y: height ? clamp((pixels.y + delta.y) / height) : clamp(position.y),
  }
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

export function restoreFloatingPlacements(value: unknown): SavedFloatingPlacement[] {
  if (!Array.isArray(value)) return []
  const entries = new Map<string, FloatingPlacement>()
  for (const entry of value) {
    if (
      !record(entry) ||
      typeof entry.key !== 'string' ||
      !/^(desktop|mobile):[a-zA-Z0-9][a-zA-Z0-9._:-]{0,95}$/.test(entry.key) ||
      !record(entry.position) ||
      typeof entry.position.x !== 'number' ||
      !Number.isFinite(entry.position.x) ||
      typeof entry.position.y !== 'number' ||
      !Number.isFinite(entry.position.y)
    )
      continue
    entries.delete(entry.key)
    entries.set(entry.key, { x: clamp(entry.position.x), y: clamp(entry.position.y) })
    if (entries.size > FLOATING_PLACEMENT_LIMIT) entries.delete(entries.keys().next().value ?? '')
  }
  return [...entries].map(([key, position]) => ({ key, position }))
}

export function saveFloatingPlacement(
  entries: SavedFloatingPlacement[],
  key: string,
  position: FloatingPlacement,
): SavedFloatingPlacement[] {
  return restoreFloatingPlacements([
    ...entries.filter((entry) => entry.key !== key),
    { key, position },
  ])
}
