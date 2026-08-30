// 输入框工具栏：React Bits 弹性浮层 + 工具项爆发式逐个入场，装饰动画按需加载。
import { lazy, Suspense, type ReactNode } from 'react'

const AnimatedContent = lazy(() =>
  import('@/components/react-bits/AnimatedContent').then((module) => ({
    default: module.AnimatedContent,
  })),
)
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
  return (
    <Suspense fallback={null}>
      <AnimatedContent
        show={open}
        direction="vertical"
        distance={16}
        initialOpacity={0}
        scale={0.82}
        reveal
        spring
        allowOverflow
        className="absolute bottom-[calc(100%+12px)] left-0 z-40 w-max max-w-full [transform-origin:left_bottom]"
      >
        <div className="composer-tool-tray-shell relative isolate w-max max-w-full overflow-visible rounded-[var(--r-md)] bg-[var(--solid)] p-0.5 shadow-[0_24px_60px_-16px_var(--floating-shadow),0_0_28px_-8px_var(--brand-blue)]">
          <span
            className="pointer-events-none absolute inset-0 overflow-hidden rounded-[inherit]"
            aria-hidden="true"
          >
            <i className="absolute inset-[-160%] [background:conic-gradient(from_90deg,var(--brand-blue),var(--star),var(--success),var(--amber),var(--brand-blue))] [animation:composer-energy-spin_2.2s_linear_infinite]" />
          </span>
          <div
            id={trayId}
            className="composer-tool-tray relative z-10 flex min-h-14 w-max max-w-full min-w-0 flex-wrap items-center gap-1.5 overflow-visible rounded-[calc(var(--r-md)-2px)] bg-[color-mix(in_srgb,var(--solid)_92%,transparent)] p-1.5 [backdrop-filter:blur(18px)_saturate(1.35)]"
            aria-label={label}
          >
            <AnimatedList
              animateInitial
              delegateClicks
              variant="burst"
              className="!size-11 !w-11 !flex-none cursor-pointer rounded-[var(--r-sm)] border border-[color-mix(in_srgb,currentColor_16%,transparent)] shadow-[0_7px_18px_-12px_currentColor] transition-[border-color,box-shadow] duration-200 nth-[4n+1]:bg-[var(--brand-blue-soft)] nth-[4n+1]:text-[var(--brand-blue-strong)] nth-[4n+2]:bg-[var(--violet-soft)] nth-[4n+2]:text-[var(--violet-strong)] nth-[4n+3]:bg-[var(--success-soft)] nth-[4n+3]:text-[var(--success-strong)] nth-[4n+4]:bg-[var(--warning-soft)] nth-[4n+4]:text-[var(--warning-strong)] hover:border-current hover:shadow-[0_12px_24px_-10px_currentColor] [&>*]:!size-full [&>*]:!min-w-0 [&>*]:!bg-transparent [&>*]:!text-inherit [&>*>button]:!size-full [&>*>button]:!bg-transparent [&_.git-changes-trigger>i]:!right-0.5 [&_.git-changes-trigger>i]:!top-0.5"
            >
              {children}
            </AnimatedList>
          </div>
        </div>
      </AnimatedContent>
    </Suspense>
  )
}
