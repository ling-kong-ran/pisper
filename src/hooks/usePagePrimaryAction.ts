// 把页面主操作注册进壳层：用 ref 持有最新 action，避免每次渲染都重新
// 注册/注销导致注册表抖动；页面卸载时自动注销。
import { useEffect, useRef } from 'react'

type RegisterPrimaryAction = (action: () => void) => () => void

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
