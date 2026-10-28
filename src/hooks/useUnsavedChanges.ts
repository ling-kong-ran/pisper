import { useCallback, useContext, useLayoutEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import {
  UnsavedChangesContext,
  type GuardedNavigate,
  type UnsavedChangesSource,
} from '@/app/contracts/unsaved-changes'

export function useUnsavedChanges(source: UnsavedChangesSource) {
  const context = useContext(UnsavedChangesContext)
  useLayoutEffect(() => {
    context?.update(source)
  })
  useLayoutEffect(() => () => context?.remove(source.id), [context, source.id])
  const requestClose = useCallback(
    (close: () => void) => {
      if (context) context.requestClose(source.id, close)
      else close()
    },
    [context, source.id],
  )
  return { requestClose }
}

export function useGuardedNavigation(): GuardedNavigate {
  const context = useContext(UnsavedChangesContext)
  const navigate = useNavigate()
  return useCallback(
    (to, options, onCommit) => {
      if (context) context.navigate(to, options, onCommit)
      else {
        void navigate(to, options)
        onCommit?.()
      }
    },
    [context, navigate],
  )
}
