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
