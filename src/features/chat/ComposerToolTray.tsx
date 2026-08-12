import { lazy, Suspense, type ReactNode } from 'react'

const AnimatedList = lazy(() =>
  import('@/components/react-bits/AnimatedList').then((module) => ({
    default: module.AnimatedList,
  })),
)

export function ComposerToolTray({
  open,
  children,
  label,
  trayId,
}: {
  open: boolean
  children: ReactNode
  label: string
  trayId: string
}) {
  const tray = open ? (
    <div id={trayId} className="composer-tool-tray" aria-label={label}>
      {children}
    </div>
  ) : null

  return (
    <Suspense fallback={tray}>
      <AnimatedList className="composer-tool-tray-motion">{tray}</AnimatedList>
    </Suspense>
  )
}
