// 在导入持久化 Store 前恢复 Runtime 偏好，与真实应用启动顺序保持一致。
import React from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClientProvider } from '@tanstack/react-query'
import { restorePageState } from '@/lib/storage/page-state-storage'

await restorePageState()
const [{ SidebarRecentSessions }, { SidebarProvider }, startup, events] = await Promise.all([
  import('@/components/layout/SidebarRecentSessions'),
  import('@/components/ui/sidebar'),
  import('@/lib/startup/startup-queries'),
  import('@/features/chat/model/events'),
])
const state = window.sidebarFixture
const uninstall = startup.installStartupQueryEvents(window)
state.deleted = events.announceSessionsUpdated
state.refresh = () => startup.fetchStartupQuery('sessions', true)
window.addEventListener(events.SESSION_CREATE_REQUESTED_EVENT, (event) =>
  state.creations.push(event.detail),
)
window.addEventListener(
  'pagehide',
  () => {
    uninstall()
    startup.queryClient.clear()
  },
  { once: true },
)
createRoot(document.getElementById('root')).render(
  <QueryClientProvider client={startup.queryClient}>
    <SidebarProvider mobile={false}>
      <SidebarRecentSessions
        navigate={(page) => state.navigation.push(page)}
        requestText={() => Promise.resolve(state.nextName)}
        requestConfirm={(options) => {
          state.confirmations.push(options)
          return Promise.resolve(state.approve)
        }}
        notify={(message, tone) => state.notices.push({ message, tone })}
      />
    </SidebarProvider>
  </QueryClientProvider>,
)
