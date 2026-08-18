// 把页面主操作注册进壳层：用 ref 持有最新 action，避免每次渲染都重新
// 注册/注销导致注册表抖动；页面卸载时自动注销。
import { useEffect, useRef } from 'react'

type RegisterPrimaryAction = (action: () => void) => () => void

// 把页面主操作注册到壳层：用 ref 始终触发最新 action，
// 卸载时自动注销（注册表返回的清理函数）。
export function usePagePrimaryAction(
  registerPrimaryAction: RegisterPrimaryAction | undefined,
  action: (() => void) | undefined,
) {
  const actionRef = useRef(action)
  actionRef.current = action

  useEffect(() => {
    if (!registerPrimaryAction) return undefined
    return registerPrimaryAction(() => actionRef.current?.())
  }, [registerPrimaryAction])
}
