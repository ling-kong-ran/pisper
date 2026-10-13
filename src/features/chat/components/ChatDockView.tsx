// Dock 分屏视图：dockview 多会话面板布局（含样式）。桌面端按需懒加载；
// 移动端走单会话视图，不下载/执行本模块。
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import 'dockview-react/dist/styles/dockview.css'
import {
  DockviewReact,
  type BuiltInContextMenuItem,
  type DockviewApi,
  type DockviewGroupPanel,
  type DockviewReadyEvent,
  type GetTabContextMenuItemsParams,
  type IDockviewHeaderActionsProps,
  type ReactContextMenuItemConfig,
} from 'dockview-react'
import { LoaderCircle, MoreHorizontal, Plus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useI18n } from '@/app/i18n/use-i18n'
import { useShortcutLabel } from '@/lib/ui/shortcuts'
import { ChatDockWatermark, SessionDockPanel } from '@/features/chat/components/ChatDock'
import { WebPreviewDockPanel } from '@/features/chat/components/WebPreviewDockPanel'

type ChatDockViewProps = {
  compactDock: boolean
  onDockReady: (event: DockviewReadyEvent) => void
  getTabContextMenuItems: (
    params: Pick<GetTabContextMenuItemsParams, 'panel'>,
  ) => Array<BuiltInContextMenuItem | ReactContextMenuItemConfig>
  createSession: (targetGroup?: DockviewGroupPanel) => Promise<string>
}

export function ChatDockView({
  compactDock,
  onDockReady,
  getTabContextMenuItems,
  createSession,
}: ChatDockViewProps) {
  const { t } = useI18n()
  const containerRef = useRef<HTMLDivElement>(null)
  const [dockApi, setDockApi] = useState<DockviewApi | null>(null)
  const handleReady = useCallback(
    (event: DockviewReadyEvent) => {
      setDockApi(event.api)
      onDockReady(event)
    },
    [onDockReady],
  )
  useLayoutEffect(() => {
    const container = containerRef.current
    if (!container || !dockApi) return
    // 侧栏拖动会改变父容器而非窗口尺寸；在绘制前同步网格，避免默认异步缩放裁掉会话。
    const layout = () => {
      const { clientWidth: width, clientHeight: height } = container
      if (width > 0 && height > 0) dockApi.layout(width, height)
    }
    layout()
    const observer = new ResizeObserver(layout)
    observer.observe(container)
    return () => observer.disconnect()
  }, [dockApi])
  const primaryShortcut = useShortcutLabel('primaryAction')
  const DockGroupActions = useMemo(
    () =>
      function DockGroupActions({ activePanel }: IDockviewHeaderActionsProps) {
        const items = activePanel ? getTabContextMenuItems({ panel: activePanel }) : []
        return (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                className="size-9 rounded-none"
                aria-label={t('chat:focusSession.chatActions')}
                title={t('chat:focusSession.chatActions')}
                disabled={!activePanel}
              >
                <MoreHorizontal size={16} />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {items.map((item, index) =>
                item === 'separator' ? (
                  <DropdownMenuSeparator key={index} />
                ) : typeof item === 'object' ? (
                  <DropdownMenuItem
                    key={item.label}
                    disabled={item.disabled}
                    onSelect={item.action}
                  >
                    {item.label}
                  </DropdownMenuItem>
                ) : null,
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        )
      },
    [getTabContextMenuItems, t],
  )
  const DockNewSessionAction = useMemo(
    () =>
      function DockNewSessionAction({ group }: IDockviewHeaderActionsProps) {
        const label = t('navigation:pageHeader.newChat')
        const [creating, setCreating] = useState(false)
        const creatingRef = useRef(false)
        const mountedRef = useRef(true)
        useEffect(() => {
          mountedRef.current = true
          return () => {
            mountedRef.current = false
          }
        }, [])
        return (
          <button
            type="button"
            className="dock-new-session hover:bg-[var(--surface-hover)] hover:text-[var(--text)] focus-visible:relative focus-visible:z-[1] focus-visible:[outline:2px_solid_var(--focus)] focus-visible:[outline-offset:-2px] grid w-[36px] h-[35px] flex-none place-items-center border-0 [border-left:1px_solid_var(--stroke-soft)] bg-transparent text-[var(--text-muted)] cursor-pointer"
            title={label}
            aria-label={label}
            aria-busy={creating}
            disabled={creating}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={(event) => {
              event.stopPropagation()
              if (creatingRef.current) return
              creatingRef.current = true
              setCreating(true)
              void createSession(group).finally(() => {
                creatingRef.current = false
                if (mountedRef.current) setCreating(false)
              })
            }}
          >
            {creating ? <LoaderCircle size={15} className="animate-spin" /> : <Plus size={15} />}
          </button>
        )
      },
    [createSession, t],
  )
  const dockComponents = useMemo(
    () => ({ session: SessionDockPanel, webPreview: WebPreviewDockPanel }),
    [],
  )
  // 空 Dock 的水印页带上新建会话入口：dockview 只接受组件类型，
  // 这里用闭包把 createSession 与快捷键提示传给水印组件。
  const Watermark = useMemo(
    () =>
      function ChatDockViewWatermark() {
        return (
          <ChatDockWatermark
            onNewSession={() => createSession()}
            newSessionShortcut={primaryShortcut}
          />
        )
      },
    [createSession, primaryShortcut],
  )
  return (
    <div ref={containerRef} className="h-full w-full min-h-0 min-w-0">
      <DockviewReact
        className="dockview-theme-light dockview-theme-pisper"
        components={dockComponents}
        watermarkComponent={Watermark}
        leftHeaderActionsComponent={DockNewSessionAction}
        rightHeaderActionsComponent={DockGroupActions}
        onReady={handleReady}
        getTabContextMenuItems={getTabContextMenuItems}
        disableFloatingGroups
        disableDnd={compactDock}
        noPanelsOverlay="watermark"
      />
    </div>
  )
}
