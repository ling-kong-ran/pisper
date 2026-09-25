import type { Notify } from '@/app/route-context'
import { useFloatingWidgetDefaults } from '@/app/useFloatingWidgetDefaults'
import { ChatLayoutSwitcher } from '@/features/chat/layout/switcher'
import { FloatingWidgetControls } from '@/features/custom-ui/floating-controls'

export function ChatLayoutMenu({ notify, onManage }: { notify: Notify; onManage: () => void }) {
  const defaults = useFloatingWidgetDefaults()
  return (
    <ChatLayoutSwitcher
      notify={notify}
      onManage={onManage}
      widgetActionsSlot={<FloatingWidgetControls defaults={defaults} notify={notify} />}
    />
  )
}
