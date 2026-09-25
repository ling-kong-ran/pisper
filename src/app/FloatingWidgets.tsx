// 悬浮组件由应用壳持有；路由与会话切换只更新页面内容，不重建沙箱和计时器。
import { useMemo, type RefObject } from 'react'
import { useI18n } from '@/app/use-i18n'
import { useFloatingWidgetDefaults } from '@/app/useFloatingWidgetDefaults'
import { FloatingCustomUi } from '@/features/custom-ui/floating'
import {
  resolveFloatingWidgetIds,
  useFloatingWidgetsStore,
} from '@/features/custom-ui/floating-preferences'

export function FloatingWidgets({
  anchorRef,
  notify,
}: {
  anchorRef: RefObject<HTMLElement | null>
  notify: (message: string) => void
}) {
  const { t } = useI18n()
  const defaults = useFloatingWidgetDefaults()
  const prefs = useFloatingWidgetsStore((state) => state.prefs)
  const widgets = useMemo(
    () =>
      resolveFloatingWidgetIds(defaults, prefs).map((componentId) => ({
        id: componentId,
        componentId,
      })),
    [defaults, prefs],
  )
  return (
    <FloatingCustomUi
      widgets={widgets}
      anchorRef={anchorRef}
      notify={notify}
      onClose={(componentId) => {
        try {
          useFloatingWidgetsStore.getState().setVisible(componentId, false)
          if (useFloatingWidgetsStore.getState().storageError)
            notify(t('custom-ui:floating.storageFailed'))
        } catch {
          notify(t('custom-ui:floating.storageFailed'))
        }
      }}
    />
  )
}
