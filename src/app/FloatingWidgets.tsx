// 悬浮组件由应用壳持有；路由与会话切换只更新页面内容，不重建沙箱和计时器。
import { useMemo, type RefObject } from 'react'
import { useI18n } from '@/app/i18n/use-i18n'
import { FloatingCustomUi } from '@/features/custom-ui/model/floating'
import {
  resolveFloatingWidgetIds,
  useFloatingWidgetsStore,
} from '@/features/custom-ui/model/floating-preferences'

export function FloatingWidgets({
  anchorRef,
  notify,
}: {
  anchorRef: RefObject<HTMLElement | null>
  notify: (message: string) => void
}) {
  const { t } = useI18n()
  const prefs = useFloatingWidgetsStore((state) => state.prefs)
  const widgets = useMemo(
    () =>
      resolveFloatingWidgetIds([], prefs).map((componentId) => ({
        id: componentId,
        componentId,
      })),
    [prefs],
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
