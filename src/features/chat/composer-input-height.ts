// 输入框随内容自适应高度：封顶与样式里的 max-h-[220px] 一致，超出部分在框内滚动。
export const COMPOSER_INPUT_MAX_HEIGHT = 220

type ResizableInput = {
  style: { height: string }
  scrollHeight: number
}

export function fitComposerInputHeight(element: ResizableInput) {
  element.style.height = 'auto'
  // 没有布局（display: none 或已脱离文档）时 scrollHeight 是 0：留空交给 CSS 最小高度，
  // 不写 0px，下次可见时再按内容量。
  if (!element.scrollHeight) {
    element.style.height = ''
    return
  }
  element.style.height = `${Math.min(element.scrollHeight, COMPOSER_INPUT_MAX_HEIGHT)}px`
}
