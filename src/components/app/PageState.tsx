import type { ReactNode } from 'react'
import { LoaderCircle } from 'lucide-react'

export function PageState({
  kind,
  title,
  description,
  action,
}: {
  kind: 'loading' | 'empty' | 'error'
  title: ReactNode
  description?: ReactNode
  action?: ReactNode
}) {
  return (
    <div
      role={kind === 'error' ? 'alert' : 'status'}
      aria-busy={kind === 'loading'}
      className="flex min-h-64 w-full min-w-0 flex-col items-center justify-center gap-3 px-4 py-8 text-center"
    >
      {kind === 'loading' && (
        <LoaderCircle className="size-5 animate-spin text-content-muted" aria-hidden="true" />
      )}
      <h2 className="text-base font-medium text-content">{title}</h2>
      {description && (
        <p className="max-w-md break-words text-sm text-content-muted">{description}</p>
      )}
      {action}
    </div>
  )
}
