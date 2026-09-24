// 应用壳负责把聊天布局偏好映射到通用导航；通用 Sidebar 不依赖聊天领域。
import { useEffect, useLayoutEffect } from 'react'
import { useChatLayoutStore, type DesktopChatLayout } from '@/features/chat/layout/public'
import { useUiStore } from '@/stores/ui-store'

export function ChatLayoutNavigation({
  onChange,
}: {
  onChange: (layout: DesktopChatLayout) => void
}) {
  const desktop = useChatLayoutStore((state) => state.active.desktop)
  useLayoutEffect(() => onChange(desktop), [desktop, onChange])
  useEffect(
    () =>
      useChatLayoutStore.subscribe((state, previous) => {
        // 模板的折叠选项只在应用时执行一次，后续手动展开仍以 ui-store 为准。
        if (
          state.revision === previous.revision ||
          state.active.desktop.navigationCollapsed === null
        )
          return
        useUiStore.getState().setSidebarCollapsed(state.active.desktop.navigationCollapsed)
      }),
    [],
  )
  return null
}
