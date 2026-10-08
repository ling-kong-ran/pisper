// 手机壳只在窄屏用整页切换；宽且高的 Pad 同时露出三栏，避免横屏仍要来回滑动。
export const MOBILE_SHELL_PANES = ['sessions', 'chat', 'context'] as const

export type MobileShellPane = (typeof MOBILE_SHELL_PANES)[number]
export type MobileShellMode = 'phone' | 'pad'
export type MobileContextTab = 'assets' | 'changes' | 'extensions' | 'files' | 'terminal'

const PAD_MIN_WIDTH = 960
const PAD_MIN_HEIGHT = 640
const SWIPE_THRESHOLD = 48

export function resolveMobileShellMode(width: number, height: number): MobileShellMode {
  if (width >= PAD_MIN_WIDTH && height >= PAD_MIN_HEIGHT) return 'pad'
  return 'phone'
}

export function mobileShellPaneIndex(pane: MobileShellPane) {
  const index = MOBILE_SHELL_PANES.indexOf(pane)
  return index < 0 ? 1 : index
}

export function moveMobileShellPane(pane: MobileShellPane, direction: -1 | 1): MobileShellPane {
  const next = mobileShellPaneIndex(pane) + direction
  return MOBILE_SHELL_PANES[Math.min(MOBILE_SHELL_PANES.length - 1, Math.max(0, next))]
}

// 纵向滚动优先。只有明确的水平位移才切屏，避免对话列表被误判成滑动。
export function swipeDeltaToDirection(dx: number, dy: number): -1 | 1 | 0 {
  if (Math.abs(dx) < SWIPE_THRESHOLD || Math.abs(dx) < Math.abs(dy) * 1.2) return 0
  return dx < 0 ? 1 : -1
}

export function shouldIgnoreMobileSwipe(target: EventTarget | null) {
  if (!(target instanceof Element)) return false
  return Boolean(
    target.closest(
      'input, textarea, select, [contenteditable="true"], [data-swipe-ignore], [role="dialog"], [role="slider"]',
    ),
  )
}
