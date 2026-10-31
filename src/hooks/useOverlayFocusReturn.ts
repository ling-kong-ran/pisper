// 浮层关闭后把焦点还给打开它的元素。
// Radix 模态对话框关闭时默认只把焦点还给 DialogTrigger；由侧栏按钮的点击回调、快捷键或别处代码打开
// （没有 DialogTrigger，例如 ⌘K、派发、配对）时焦点会落到 body，键盘与读屏用户丢失位置。
// 打开时（布局阶段，早于对话框把焦点移进内容）记下当时的焦点；关闭时只在焦点无处可去（body）时归还——
// 执行的命令若已把焦点交给新页面（例如打开会话后的输入框），不抢回来。
import { useCallback, useLayoutEffect, useRef } from 'react'

export function useOverlayFocusReturn(open: boolean) {
  const opener = useRef<HTMLElement | null>(null)
  useLayoutEffect(() => {
    if (!open) return
    const active = document.activeElement
    opener.current = active instanceof HTMLElement && active !== document.body ? active : null
  }, [open])
  // 交给 DialogContent / SheetContent 的 onCloseAutoFocus。
  return useCallback((event: Event) => {
    const target = opener.current
    opener.current = null
    const active = document.activeElement
    if (active && active !== document.body) return
    if (!target?.isConnected) return
    event.preventDefault()
    target.focus({ preventScroll: true })
  }, [])
}
