// 输入框工具栏：附件/资源/命令等按钮的容器，装饰动画懒加载。
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
  mobile = false,
}: {
  open: boolean
  children: ReactNode
  label: string
  trayId: string
  mobile?: boolean
}) {
  const tray = open ? (
    <div
      id={trayId}
      className={`composer-tool-tray @max-[700px]:gap-[3px] @max-[470px]:gap-[2px] flex min-w-0 items-center gap-[5px] [padding-left:1px] ${mobile ? 'w-full overflow-x-auto overscroll-x-contain [scrollbar-width:none] [&::-webkit-scrollbar]:hidden [&>*]:!size-9 [&>*]:!min-w-9 [&>*]:!flex-none [&>*>button]:!size-full' : ''}`}
      aria-label={label}
    >
      {children}
    </div>
  ) : null

  if (mobile) return tray

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
        className="min-w-0 max-w-[calc(100%_-_40px)] flex-1 [transform-origin:left_center]"
      >
        {tray}
      </AnimatedContent>
    </Suspense>
  )
}
