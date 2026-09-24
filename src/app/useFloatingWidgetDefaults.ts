// 应用层将模板中的悬浮节点映射为组件默认值，不在组件领域复制模板状态。
import { useMemo } from 'react'
import { collectFloatingCanvasIslands, useChatLayoutStore } from '@/features/chat/layout/public'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { useIsMobileApp } from '@/stores/client-store'

export function useFloatingWidgetDefaults() {
  const mobileApp = useIsMobileApp()
  const phone = useIsPhoneViewport()
  const canvas = useChatLayoutStore((state) =>
    mobileApp || phone ? state.active.mobile.canvas : state.active.desktop.canvas,
  )
  return useMemo(
    () =>
      collectFloatingCanvasIslands(canvas).flatMap((node) =>
        node.componentId ? [node.componentId] : [],
      ),
    [canvas],
  )
}
