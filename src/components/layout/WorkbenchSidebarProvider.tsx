// 工作台拥有侧栏宽度偏好；拖动草稿只留在布局层，松手后才持久化。
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ComponentProps,
  type CSSProperties,
} from 'react'
import { useI18n } from '@/app/i18n/use-i18n'
import { SidebarProvider, useSidebar } from '@/components/ui/sidebar'
import {
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
  useUiStore,
} from '@/stores/ui-store'
import { cn } from '@/lib/utils'

type WidthControl = {
  width: number
  maximum: number
  preview: (width: number | null) => void
  commit: (width: number) => void
}
const WidthContext = createContext<WidthControl | null>(null)
const COLLAPSED_WIDTH = 64

export function WorkbenchSidebarProvider({
  className,
  style,
  ...props
}: ComponentProps<typeof SidebarProvider>) {
  const savedWidth = useUiStore((state) => state.sidebarWidth)
  const saveWidth = useUiStore((state) => state.setSidebarWidth)
  const [draft, preview] = useState<number | null>(null)
  const [viewport, setViewport] = useState(() => window.innerWidth)
  useEffect(() => {
    const update = () => setViewport(window.innerWidth)
    window.addEventListener('resize', update)
    return () => window.removeEventListener('resize', update)
  }, [])
  // 窄窗口也至少保留 55% 宽度给内容；自动收窄不会覆盖用户保存的偏好。
  const maximum = Math.max(
    SIDEBAR_MIN_WIDTH,
    Math.min(SIDEBAR_MAX_WIDTH, Math.floor(viewport * 0.45)),
  )
  // 拖动草稿允许经过图标栏与展开下限之间的宽度，避免刚移动鼠标就跳到 220px。
  const width = Math.min(
    maximum,
    Math.max(draft === null ? SIDEBAR_MIN_WIDTH : COLLAPSED_WIDTH, draft ?? savedWidth),
  )
  const commit = useCallback(
    (value: number) => {
      saveWidth(Math.min(maximum, Math.max(SIDEBAR_MIN_WIDTH, value)))
      preview(null)
    },
    [maximum, saveWidth],
  )
  return (
    <WidthContext.Provider value={{ width, maximum, preview, commit }}>
      <SidebarProvider
        {...props}
        style={{ ...style, '--sidebar-width': `${width}px` } as CSSProperties}
        className={cn(
          className,
          draft !== null &&
            'select-none cursor-col-resize [&_.sidebar]:overflow-x-hidden [&_[data-slot=sidebar-gap]]:transition-none [&_[data-slot=sidebar-container]]:transition-none',
        )}
      />
    </WidthContext.Provider>
  )
}

export function WorkbenchSidebarResizeHandle() {
  const control = useContext(WidthContext)
  const { open, setOpen, isMobile, persistent } = useSidebar()
  const { t } = useI18n()
  const drag = useRef<{
    pointer: number
    x: number
    width: number
    next: number
    expanded: boolean
  } | null>(null)
  const preview = control?.preview
  const enabled = !isMobile && !persistent
  useEffect(() => {
    if (!enabled) {
      drag.current = null
      preview?.(null)
    }
    return () => {
      drag.current = null
      preview?.(null)
    }
  }, [enabled, preview])
  if (!control || !enabled) return null
  const { width, maximum, commit } = control
  const cancel = () => {
    const current = drag.current
    drag.current = null
    control.preview(null)
    if (current) setOpen(current.expanded)
  }
  const collapse = () => {
    drag.current = null
    control.preview(null)
    // 复用现有收起动作，保留已保存的展开宽度，不把临界宽度写回偏好。
    setOpen(false)
  }
  return (
    <div
      role="separator"
      tabIndex={0}
      aria-orientation="vertical"
      aria-label={t('navigation:appSidebar.resizeSidebar')}
      aria-controls="workbench-sidebar"
      aria-valuemin={COLLAPSED_WIDTH}
      aria-valuemax={maximum}
      aria-valuenow={open ? width : COLLAPSED_WIDTH}
      title={t('navigation:appSidebar.resizeSidebarHint')}
      className="absolute inset-y-0 -right-1 z-30 w-2 touch-none cursor-col-resize outline-none [-webkit-app-region:no-drag] after:absolute after:inset-y-0 after:left-1/2 after:w-0.5 hover:after:bg-primary/50 focus-visible:after:bg-primary"
      onDoubleClick={() => {
        setOpen(true)
        commit(SIDEBAR_DEFAULT_WIDTH)
      }}
      onPointerDown={(event) => {
        if (event.button !== 0 || !event.isPrimary) return
        event.preventDefault()
        event.currentTarget.focus()
        event.currentTarget.setPointerCapture(event.pointerId)
        const startWidth = open ? width : COLLAPSED_WIDTH
        drag.current = {
          pointer: event.pointerId,
          x: event.clientX,
          width: startWidth,
          next: startWidth,
          expanded: open,
        }
        control.preview(startWidth)
      }}
      onPointerMove={(event) => {
        const current = drag.current
        if (!current || current.pointer !== event.pointerId) return
        if (!open && event.clientX - current.x < 4) return
        const requestedWidth = current.width + event.clientX - current.x
        if (current.expanded && requestedWidth <= SIDEBAR_MIN_WIDTH && event.clientX < current.x) {
          event.currentTarget.releasePointerCapture(event.pointerId)
          collapse()
          return
        }
        current.next = Math.round(
          Math.min(
            maximum,
            Math.max(current.expanded ? SIDEBAR_MIN_WIDTH : COLLAPSED_WIDTH, requestedWidth),
          ),
        )
        if (!open) setOpen(true)
        control.preview(current.next)
      }}
      onPointerUp={(event) => {
        const current = drag.current
        if (!current || current.pointer !== event.pointerId) return
        drag.current = null
        // 短距离试探松手后回到图标栏；拖过中点才吸附到最小可用展开宽度。
        if (!current.expanded && current.next < (COLLAPSED_WIDTH + SIDEBAR_MIN_WIDTH) / 2)
          collapse()
        else commit(current.next)
        event.currentTarget.releasePointerCapture(event.pointerId)
      }}
      onLostPointerCapture={cancel}
      onPointerCancel={cancel}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          cancel()
          return
        }
        if (!open && event.key === 'ArrowLeft') {
          event.preventDefault()
          return
        }
        const next =
          event.key === 'ArrowLeft'
            ? width - 10
            : event.key === 'ArrowRight'
              ? open
                ? width + 10
                : width
              : event.key === 'Home'
                ? SIDEBAR_MIN_WIDTH
                : event.key === 'End'
                  ? maximum
                  : event.key === 'Enter'
                    ? SIDEBAR_DEFAULT_WIDTH
                    : null
        if (next === null) return
        event.preventDefault()
        if (event.key === 'ArrowLeft' && next <= SIDEBAR_MIN_WIDTH) {
          collapse()
          return
        }
        setOpen(true)
        commit(next)
      }}
    />
  )
}
