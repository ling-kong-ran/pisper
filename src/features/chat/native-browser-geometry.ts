// 原生子 WebView 盖在网页界面之上，CSS 层级管不到它：这里算它该放在哪、有没有被界面的弹层挡住。
export type NativeBrowserBounds = { x: number; y: number; width: number; height: number }
export type NativeBrowserPlacement = { bounds: NativeBrowserBounds; visible: boolean }

type RectLike = { left: number; top: number; width: number; height: number }
type Viewport = { width: number; height: number }

export function boundsFromRect({ left, top, width, height }: RectLike): NativeBrowserBounds {
  return { x: left, y: top, width, height }
}

// 页签隐藏时占位区是 0×0：只把原生视图藏起来，大小不动。缩到 0 宽网页会重新排版，切回来滚动位置就丢了。
export function placementFromRect(
  previous: NativeBrowserPlacement | null,
  rect: RectLike,
  occluded: boolean,
): NativeBrowserPlacement {
  const shown = rect.width > 0 && rect.height > 0
  return {
    bounds: shown || !previous ? boundsFromRect(rect) : previous.bounds,
    visible: shown && !occluded,
  }
}

// 小于半个像素的抖动不值得跨一次 IPC。
export function placementChanged(
  previous: NativeBrowserPlacement | null,
  next: NativeBrowserPlacement,
) {
  if (!previous || previous.visible !== next.visible) return true
  const before = previous.bounds
  const after = next.bounds
  return (
    Math.abs(before.x - after.x) >= 0.5 ||
    Math.abs(before.y - after.y) >= 0.5 ||
    Math.abs(before.width - after.width) >= 0.5 ||
    Math.abs(before.height - after.height) >= 0.5
  )
}

// 取 3×3 个点：对话框、抽屉和下拉菜单都能碰到，贴边路过的提示气泡碰不到，不会让网页闪一下。
export function occlusionProbePoints(rect: RectLike, viewport: Viewport) {
  const points: { x: number; y: number }[] = []
  for (const column of [0.12, 0.5, 0.88]) {
    for (const row of [0.12, 0.5, 0.88]) {
      const x = rect.left + rect.width * column
      const y = rect.top + rect.height * row
      if (x >= 0 && y >= 0 && x < viewport.width && y < viewport.height) points.push({ x, y })
    }
  }
  return points
}

// 下拉菜单和弹出面板可能小到碰不到探测点，要直接比矩形，否则会被原生视图盖住一截。
export function overlapsAny(rect: RectLike, layers: RectLike[]) {
  return layers.some(
    (layer) =>
      layer.left < rect.left + rect.width &&
      rect.left < layer.left + layer.width &&
      layer.top < rect.top + rect.height &&
      rect.top < layer.top + layer.height,
  )
}

// 只要有一个点命中的不是占位区就算被挡住。模态弹层打开时 Radix 给 body 设 pointer-events: none，
// 命中结果会变成根元素，同样算被挡住：这时网页本来也不该接收点击。
export function isOccluded(
  slot: unknown,
  points: { x: number; y: number }[],
  hitTest: (x: number, y: number) => unknown,
) {
  return points.some(({ x, y }) => hitTest(x, y) !== slot)
}
