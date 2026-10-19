import { useLayoutEffect, useRef } from 'react'
import { appBackStack } from '@/lib/back-navigation'
export function useBackLayer(enabled: boolean, priority: number, onBack: () => void) {
  const action = useRef(onBack)
  action.current = onBack
  useLayoutEffect(
    () => (enabled ? appBackStack.register(priority, () => action.current()) : undefined),
    [enabled, priority],
  )
}
