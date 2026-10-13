// Dock 分屏视图：dockview 多会话面板布局（含样式）。桌面端按需懒加载；
// 移动端走单会话视图，不下载/执行本模块。
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
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
import { LoaderCircle, Plus } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { DesktopTitlebarHost } from '@/components/layout/DesktopTitlebar'
import { useShortcutLabel } from '@/lib/ui/shortcuts'
import { ChatDockWatermark, SessionDockPanel } from '@/features/chat/components/ChatDock'
import { ChatDockTabContextMenu } from '@/features/chat/components/ChatDockTabContextMenu'
import { WebPreviewDockPanel } from '@/features/chat/components/WebPreviewDockPanel'

type ChatDockViewProps = {
  compactDock: boolean
  onDockReady: (event: DockviewReadyEvent) => void
  getTabContextMenuItems: (
    params: Pick<GetTabContextMenuItemsParams, 'panel'>,
  ) => Array<BuiltInContextMenuItem | ReactContextMenuItemConfig>
  createSession: (targetGroup?: DockviewGroupPanel) => Promise<string>
}

function DockHeaderTools({ activePanel, containerApi, group }: IDockviewHeaderActionsProps) {
  const native = Boolean(window.pisperDesktop?.customTitlebar)
  const rootRef = useRef<HTMLDivElement>(null)
  const actionsRef = useRef<HTMLDivElement>(null)
  const [ownsTitlebar, setOwnsTitlebar] = useState(false)
  const [overflowHost, setOverflowHost] = useState<HTMLElement | null>(null)
  const [actionsWidth, setActionsWidth] = useState(0)
  useLayoutEffect(() => {
    if (!native) return
    const header = rootRef.current?.closest<HTMLElement>('.dv-tabs-and-actions-container')
    header?.setAttribute('data-window-drag-region', '')
    // 多分屏时只有最上方、最右侧的分组接管标题按钮，不随网页预览焦点改变。
    const update = () => {
      const candidates = containerApi.groups
        .map((item) => ({ id: item.id, box: item.api.boundingBox }))
        .filter((item) => item.box && item.box.width > 0 && item.box.height > 0)
        .sort((a, b) => {
          if (!a.box || !b.box) return 0
          return a.box.top - b.box.top || b.box.left + b.box.width - (a.box.left + a.box.width)
        })
      setOwnsTitlebar(candidates[0]?.id === group.id)
    }
    update()
    const subscription = containerApi.onDidLayoutChange(update)
    return () => {
      subscription.dispose()
      header?.removeAttribute('data-window-drag-region')
    }
  }, [containerApi, group, native])
  useLayoutEffect(() => {
    if (!native || !ownsTitlebar) return
    const viewport = rootRef.current?.closest<HTMLElement>('[data-chat-dock-viewport]')
    const host = viewport?.querySelector<HTMLElement>('[data-chat-dock-titlebar-overflow]')
    if (!viewport || !host) return
    const update = () => {
      const box = group.api.boundingBox
      // 分组总最小宽度可能超过窗口；窗口按钮始终留在主区域的可见右沿。
      setOverflowHost(box && box.left + box.width > containerApi.width + 1 ? host : null)
    }
    update()
    const subscription = containerApi.onDidLayoutChange(update)
    const observer = new ResizeObserver(update)
    observer.observe(viewport)
    const groupElement = rootRef.current?.closest('.dv-groupview')
    if (groupElement) observer.observe(groupElement)
    return () => {
      subscription.dispose()
      observer.disconnect()
    }
  }, [containerApi, group, native, ownsTitlebar])
  useLayoutEffect(() => {
    const root = actionsRef.current
    if (!native || !ownsTitlebar || !root) return
    const previousMinimumWidth = group.minimumWidth
    let previousWidth = 0
    // 固定窗口按钮不可随标签裁切；给当前宿主保留工具、加号和一个可读标签的宽度。
    const update = () => {
      setActionsWidth(Math.ceil(root.getBoundingClientRect().width))
      const width = Math.max(
        previousMinimumWidth,
        Math.ceil(root.getBoundingClientRect().width) + 36 + 80,
      )
      if (width === previousWidth) return
      previousWidth = width
      group.api.setConstraints({ minimumWidth: width })
      containerApi.layout(containerApi.width, containerApi.height, true)
    }
    const observer = new ResizeObserver(update)
    observer.observe(root)
    update()
    return () => {
      observer.disconnect()
      // Dockview 忽略 undefined；恢复数值才能释放旧分组的临时标题栏空间。
      group.api.setConstraints({ minimumWidth: previousMinimumWidth })
      if (containerApi.groups.includes(group))
        containerApi.layout(containerApi.width, containerApi.height, true)
    }
  }, [containerApi, group, native, ownsTitlebar, overflowHost])
  const actions = (
    <div ref={actionsRef} className="flex h-full w-max shrink-0 items-center bg-background">
      <DesktopTitlebarHost />
      <DesktopTitlebarHost targetId="window-controls-chat" />
    </div>
  )
  return (
    <div
      ref={rootRef}
      className="flex h-full shrink-0 items-center"
      style={native && ownsTitlebar && overflowHost ? { width: actionsWidth } : undefined}
    >
      {native
        ? ownsTitlebar && (overflowHost ? createPortal(actions, overflowHost) : actions)
        : activePanel && <DesktopTitlebarHost targetId={activePanel.id} />}
    </div>
  )
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
    <div
      ref={containerRef}
      data-chat-dock-viewport
      className="relative h-full w-full min-h-0 min-w-0"
    >
      <ChatDockTabContextMenu api={dockApi} getItems={getTabContextMenuItems}>
        <DockviewReact
          className="dockview-theme-light dockview-theme-pisper"
          components={dockComponents}
          watermarkComponent={Watermark}
          leftHeaderActionsComponent={DockNewSessionAction}
          rightHeaderActionsComponent={DockHeaderTools}
          onReady={handleReady}
          getTabContextMenuItems={getTabContextMenuItems}
          disableFloatingGroups
          disableDnd={compactDock}
          noPanelsOverlay="watermark"
        />
      </ChatDockTabContextMenu>
      <div data-chat-dock-titlebar-overflow className="absolute right-0 top-0 z-20 h-[35px]" />
    </div>
  )
}
