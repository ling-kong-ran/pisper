// Dock 分屏视图：dockview 多会话面板布局（含样式）。桌面端按需懒加载；
// 移动端 App 走单会话视图（MobileSessionPanel），不下载/执行本模块。
import { useLayoutEffect, useMemo, useRef } from 'react'
import 'dockview-react/dist/styles/dockview.css'
import {
  DockviewReact,
  type BuiltInContextMenuItem,
  type DockviewGroupPanel,
  type DockviewReadyEvent,
  type DockviewApi,
  type GetTabContextMenuItemsParams,
  type IDockviewHeaderActionsProps,
  type ReactContextMenuItemConfig,
} from 'dockview-react'
import { Plus } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useShortcutLabel } from '@/lib/shortcuts'
import { ChatDockWatermark, SessionDockPanel } from './ChatDock'
import { WebPreviewDockPanel } from './WebPreviewDockPanel'

type ChatDockViewProps = {
  compactDock: boolean
  onDockReady: (event: DockviewReadyEvent) => void
  onDockDispose: (api: DockviewApi) => void
  getTabContextMenuItems: (
    params: GetTabContextMenuItemsParams,
  ) => Array<BuiltInContextMenuItem | ReactContextMenuItemConfig>
  createSession: (targetGroup?: DockviewGroupPanel) => Promise<string>
}

export function ChatDockView({
  compactDock,
  onDockReady,
  onDockDispose,
  getTabContextMenuItems,
  createSession,
}: ChatDockViewProps) {
  const apiRef = useRef<DockviewApi | null>(null)
  useLayoutEffect(
    () => () => {
      if (apiRef.current) onDockDispose(apiRef.current)
    },
    [onDockDispose],
  )
  const { t } = useI18n()
  const primaryShortcut = useShortcutLabel('primaryAction')
  const DockNewSessionAction = useMemo(
    () =>
      function DockNewSessionAction({ group }: IDockviewHeaderActionsProps) {
        const label = t('navigation:pageHeader.newChat')
        return (
          <button
            type="button"
            className="dock-new-session grid h-[35px] w-9 flex-none cursor-pointer place-items-center border-0 border-l border-stroke-soft bg-transparent text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:relative focus-visible:z-[1] focus-visible:-outline-offset-2"
            title={label}
            aria-label={label}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={(event) => {
              event.stopPropagation()
              void createSession(group)
            }}
          >
            <Plus size={16} />
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
    <DockviewReact
      className="dockview-theme-light dockview-theme-pisper"
      components={dockComponents}
      watermarkComponent={Watermark}
      leftHeaderActionsComponent={DockNewSessionAction}
      onReady={(event) => {
        apiRef.current = event.api
        onDockReady(event)
      }}
      getTabContextMenuItems={getTabContextMenuItems}
      disableFloatingGroups
      disableDnd={compactDock}
      noPanelsOverlay="watermark"
    />
  )
}
