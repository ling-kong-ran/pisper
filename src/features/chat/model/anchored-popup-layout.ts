type PopupBounds = { top: number; bottom: number; left: number; right: number }
type PopupViewport = { top: number; left: number; width: number; height: number }

export function resolveAnchoredPopupLayout({
  anchor,
  menu,
  viewport,
  placement = 'top',
  align = 'start',
  maxHeight = Number.POSITIVE_INFINITY,
}: {
  anchor: PopupBounds
  menu: { width: number; height: number }
  viewport: PopupViewport
  placement?: 'top' | 'bottom'
  align?: 'start' | 'end'
  maxHeight?: number
}) {
  const gutter = 8
  const gap = 8
  const visibleTop = viewport.top + gutter
  const visibleBottom = Math.max(visibleTop, viewport.top + viewport.height - gutter)
  const visibleLeft = viewport.left + gutter
  const visibleRight = Math.max(visibleLeft, viewport.left + viewport.width - gutter)
  const space = {
    top: Math.max(0, Math.min(anchor.top, visibleBottom) - visibleTop - gap),
    bottom: Math.max(0, visibleBottom - Math.max(anchor.bottom, visibleTop) - gap),
  }
  const desiredHeight = Math.min(menu.height, maxHeight)
  const opposite = placement === 'top' ? 'bottom' : 'top'
  const side =
    desiredHeight > space[placement] && space[opposite] > space[placement] ? opposite : placement
  const heightLimit = Math.min(maxHeight, space[side])
  const height = Math.min(desiredHeight, heightLimit)
  const width = Math.min(menu.width, visibleRight - visibleLeft)
  const left = Math.max(
    visibleLeft,
    Math.min(align === 'end' ? anchor.right - width : anchor.left, visibleRight - width),
  )
  const top = Math.max(
    visibleTop,
    Math.min(
      side === 'top' ? anchor.top - gap - height : anchor.bottom + gap,
      visibleBottom - height,
    ),
  )
  return { side, left, top, width, maxHeight: heightLimit }
}
