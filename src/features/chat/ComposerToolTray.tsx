import { lazy, Suspense, type ReactNode } from 'react'

const AnimatedContent = lazy(() =>
  import('@/components/react-bits/AnimatedContent').then((module) => ({
    default: module.AnimatedContent,
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
    <Suspense fallback={null}>
      <AnimatedContent
        show={open}
        direction="horizontal"
        reverse
        distance={18}
        duration={0.24}
        initialOpacity={0}
        scale={0.96}
        className="composer-tool-tray-motion"
      >
        {tray}
      </AnimatedContent>
    </Suspense>
  )
}
