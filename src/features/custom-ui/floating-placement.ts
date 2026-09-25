export type FloatingPlacement = { x: number; y: number }
export type FloatingSize = { width: number; height: number }
export type FloatingAnchor = FloatingPlacement & FloatingSize
export type SavedFloatingPlacement = { key: string; position: FloatingPlacement }

export const DEFAULT_FLOATING_PLACEMENT: FloatingPlacement = { x: 0.5, y: 0 }
export const FLOATING_PLACEMENT_LIMIT = 128

const clamp = (value: number) => Math.min(1, Math.max(0, value))
const extent = (value: number) => (Number.isFinite(value) ? Math.max(0, value) : 0)

export function defaultFloatingPlacement(
  bounds: FloatingSize,
  widget: FloatingSize,
  anchor?: FloatingAnchor,
  stackIndex = 0,
): FloatingPlacement {
  if (!anchor && stackIndex === 0) return DEFAULT_FLOATING_PLACEMENT
  const width = Math.max(0, extent(bounds.width) - extent(widget.width))
  const height = Math.max(0, extent(bounds.height) - extent(widget.height))
  const center = anchor ? anchor.x + anchor.width / 2 : bounds.width / 2
  let x = center - widget.width / 2
  let y = anchor ? anchor.y + anchor.height / 2 - widget.height / 2 : 0
  if (stackIndex > 0) {
    const start = Math.max(0, anchor ? anchor.y + anchor.height + 12 : 76)
    const stride = widget.height + 12
    const fit = Math.max(0, Math.floor((height - start) / stride) + 1)
    if (stackIndex <= fit) y = start + (stackIndex - 1) * stride
    else {
      // 空间不足时错开工具条，避免所有未定位组件完全重叠。
      const overflow = stackIndex - fit
      x += 24 * overflow
      y = Math.max(0, height - (((overflow - 1) * 24) % Math.max(1, height)))
    }
  }
  return {
    x: width ? clamp(x / width) : 0.5,
    y: height ? clamp(y / height) : 0,
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
