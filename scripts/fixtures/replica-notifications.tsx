import { createRoot } from 'react-dom/client'
import { MemoryRouter } from 'react-router-dom'
import { useState } from 'react'
import { NotificationSettings } from '@/features/config/NotificationSettings'
import { TooltipProvider } from '@/components/ui/tooltip'
import { PageFrame } from '@/components/app/PageFrame'
import type { NotificationSettingsData } from '@/types/notifications'

const evidence = {
  snapshots: [] as NotificationSettingsData[],
  notices: [] as string[],
  systemTests: [] as unknown[],
}
Object.assign(window, {
  notificationEvidence: evidence,
  pisperDesktop: {
    platform: 'darwin',
    getNotificationStatus: () => Promise.resolve({ permission: 'granted', supported: true }),
    showNotification: (input: unknown) => {
      evidence.systemTests.push(input)
      return Promise.resolve({ shown: true })
    },
  },
})
const notify = (message: string) => {
  evidence.notices.push(message)
}
const onSettingsChange = (data: NotificationSettingsData) => {
  evidence.snapshots.push(structuredClone(data))
}
export function Fixture() {
  const [mounted, setMounted] = useState(true)
  return (
    <MemoryRouter>
      <TooltipProvider>
        <div className="flex h-full min-h-0 flex-col">
          <button type="button" onClick={() => setMounted(false)}>
            Unmount notifications
          </button>
          {mounted && (
            <PageFrame variant="settings">
              <NotificationSettings
                notify={notify}
                onBrowserNotificationChange={onSettingsChange}
              />
            </PageFrame>
          )}
        </div>
      </TooltipProvider>
    </MemoryRouter>
  )
}
createRoot(document.getElementById('root')!).render(<Fixture />)
