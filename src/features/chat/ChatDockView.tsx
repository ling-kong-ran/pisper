// Dock 分屏视图：dockview 多会话面板布局（含样式）。桌面端按需懒加载；
// 移动端 App 走单会话视图（MobileSessionPanel），不下载/执行本模块。
import { useMemo } from 'react'
import 'dockview-react/dist/styles/dockview.css'
import {
  DockviewReact,
  type BuiltInContextMenuItem,
  type DockviewGroupPanel,
  type DockviewReadyEvent,
  type GetTabContextMenuItemsParams,
  type IDockviewHeaderActionsProps,
  type ReactContextMenuItemConfig,
} from 'dockview-react'
import { Plus } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { ChatDockWatermark, SessionDockPanel } from './ChatDock'
import { WebPreviewDockPanel } from './WebPreviewDockPanel'

type ChatDockViewProps = {
  compactDock: boolean
  onDockReady: (event: DockviewReadyEvent) => void
  getTabContextMenuItems: (
    params: GetTabContextMenuItemsParams,
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
  const DockNewSessionAction = useMemo(
    () =>
      function DockNewSessionAction({ group }: IDockviewHeaderActionsProps) {
        const label = t('navigation:pageHeader.newChat')
        return (
          <button
            type="button"
            className="dock-new-session hover:bg-[var(--surface-hover)] hover:text-[var(--text)] focus-visible:relative focus-visible:z-[1] focus-visible:[outline:2px_solid_var(--focus)] focus-visible:[outline-offset:-2px] grid w-[36px] h-[35px] flex-none place-items-center border-0 [border-left:1px_solid_var(--stroke-soft)] bg-transparent text-[var(--text-muted)] cursor-pointer"
            title={label}
            aria-label={label}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={(event) => {
              event.stopPropagation()
              void createSession(group)
            }}
          >
            <Plus size={15} />
          </button>
        )
      },
    [createSession, t],
  )
  const dockComponents = useMemo(
    () => ({ session: SessionDockPanel, webPreview: WebPreviewDockPanel }),
    [],
  )
  return (
    <DockviewReact
      className="dockview-theme-light dockview-theme-pisper"
      components={dockComponents}
      watermarkComponent={ChatDockWatermark}
      leftHeaderActionsComponent={DockNewSessionAction}
      onReady={onDockReady}
      getTabContextMenuItems={getTabContextMenuItems}
      disableFloatingGroups
      disableDnd={compactDock}
      noPanelsOverlay="watermark"
    />
  )
}
