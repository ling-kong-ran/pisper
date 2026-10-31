// 列表行的处理动效（侧栏会话行、全部代理行共用）：处理后 scaleY + 透明度收起，下一行顶上来。
// 只动 transform / opacity；减少动效或页面隐藏时直接跳过（Promise 立即完成），从不循环。
// 是否减少动效的判断在 lib/motion（会话页的底部表单退场也用同一份），这里转出给看板各处沿用。
import { motionReduced } from '@/lib/motion'

export { motionReduced }

// 侧栏行 160ms（规格 6.6），全部代理行 180ms（规格 3.3）。
export const SIDEBAR_ROW_COLLAPSE_MS = 160
export const OVERVIEW_ROW_COLLAPSE_MS = 180

export function motionAllowed() {
  return !motionReduced() && document.visibilityState !== 'hidden'
}

// 收起动画播完（或被跳过）后才继续后续动作；fill: forwards 让行停在收起状态，直到调用方把它藏起来或取消动画。
export function collapseRow(
  element: HTMLElement | null,
  { duration, scale }: { duration: number; scale: number },
) {
  if (!element || !motionAllowed() || typeof element.animate !== 'function')
    return Promise.resolve()
  element.style.transformOrigin = 'top center'
  return element
    .animate(
      [
        { transform: 'scaleY(1)', opacity: 1 },
        { transform: `scaleY(${scale})`, opacity: 0 },
      ],
      { duration, easing: 'ease-in', fill: 'forwards' },
    )
    .finished.then(
      () => undefined,
      () => undefined,
    )
}
