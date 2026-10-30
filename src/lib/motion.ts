// 减少动效的统一判断：界面设置里的动效偏好（<html data-motion="reduced|full">）优先，
// 没有显式设置时跟随系统的 prefers-reduced-motion。全局样式已把动画压到瞬时，这里给需要在 JS 里
// 决定“播不播”的地方用（FLIP、退场后再卸载等），避免为一段 0.01ms 的动画等待事件。
export function motionReduced() {
  if (typeof document === 'undefined') return true
  const motion = document.documentElement.dataset.motion
  if (motion === 'reduced') return true
  if (motion === 'full') return false
  return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
}
