export const DEFAULT_SIDEBAR_WIDTH = 340
export const MIN_SIDEBAR_WIDTH = 240
export const MAX_SIDEBAR_WIDTH = 520

// 视口约束只用于渲染，不回写偏好，避免短暂缩窗永久改变用户选择。
export function clampSidebarWidth(preferred: unknown, availableWidth = Infinity): number {
  const width =
    typeof preferred === 'number' && Number.isFinite(preferred) ? preferred : DEFAULT_SIDEBAR_WIDTH
  return Math.max(MIN_SIDEBAR_WIDTH, Math.min(width, MAX_SIDEBAR_WIDTH, availableWidth - 240))
}
