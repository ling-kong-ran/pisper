// 应用只持摘要与动作，不接收领域草稿、正文或凭据。
import { createContext } from 'react'
import type { NavigateOptions, To } from 'react-router-dom'

export type UnsavedChangesSource = {
  id: string
  dirty: boolean
  saving: boolean
  discard: () => void
}
export type GuardedNavigate = (to: To, options?: NavigateOptions, onCommit?: () => void) => void
export type UnsavedChangesContract = {
  update: (source: UnsavedChangesSource) => void
  remove: (id: string) => void
  requestClose: (id: string, close: () => void) => void
  navigate: GuardedNavigate
}
export const UnsavedChangesContext = createContext<UnsavedChangesContract | null>(null)
export const GUARDED_NAVIGATION_STATE = 'pisperNavigationId'
export function navigationChangesContent(
  current: { pathname: string; search: string },
  next: { pathname: string; search: string },
) {
  return current.pathname !== next.pathname || current.search !== next.search
}
export function unsavedSummary(sources: Iterable<UnsavedChangesSource>) {
  const all = [...sources]
  return { dirty: all.some((source) => source.dirty), saving: all.some((source) => source.saving) }
}
