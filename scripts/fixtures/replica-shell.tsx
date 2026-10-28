import { useEffect, useState, type CSSProperties } from 'react'
import { createRoot } from 'react-dom/client'
import { Settings } from 'lucide-react'
import { AppProviders } from '@/app/providers'
import { CommandPalette } from '@/components/layout/AppOverlays'
import { AppSidebar } from '@/components/layout/AppSidebar'
import { SidebarProvider, SidebarTrigger } from '@/components/ui/sidebar'
import { useUiStore } from '@/stores/ui-store'
import { clampSidebarWidth } from '@/lib/sidebar-width'
import '@/index.css'

const navigation: Array<[string, Array<[string, string, typeof Settings]>]> = [
  ['Fixture', [['config', 'Settings', Settings]]],
]
export function Fixture() {
  const [open, setOpen] = useState(false)
  const [opened, setOpened] = useState('')
  const [preview, setPreview] = useState<number | null>(null)
  const [viewportWidth, setViewportWidth] = useState(window.innerWidth)
  useEffect(() => {
    const update = () => setViewportWidth(window.innerWidth)
    window.addEventListener('resize', update)
    return () => window.removeEventListener('resize', update)
  }, [])
  const preferred = useUiStore((state) => state.sidebarWidth)
  const commit = useUiStore((state) => state.setSidebarWidth)
  const width = clampSidebarWidth(preview ?? preferred, viewportWidth - 52)
  return (
    <AppProviders>
      <button data-testid="launch" onClick={() => setOpen(true)}>
        Search
      </button>
      <output data-testid="opened">{opened}</output>
      <output data-testid="preferred">{preferred}</output>
      <SidebarProvider
        open
        style={{ '--sidebar-width': `${width}px`, height: 'calc(100vh - 32px)' } as CSSProperties}
        className="h-[calc(100vh-32px)]"
      >
        <SidebarTrigger data-testid="sidebar-toggle" aria-label="Toggle fixture sidebar" />
        <AppSidebar
          page="chat"
          configSection="interface"
          navigate={() => {}}
          navigateSettings={() => {}}
          onNewChat={() => {}}
          onSearch={() => setOpen(true)}
          update={{}}
          onOpenUpdates={() => {}}
          tasks={<p>Fixture conversations</p>}
          width={width}
          onWidthPreview={setPreview}
          onWidthCommit={commit}
        />
      </SidebarProvider>
      {open && (
        <CommandPalette
          navigation={navigation}
          onClose={() => setOpen(false)}
          onNavigate={setOpened}
          onNewChat={() => setOpened('new')}
          onOpenSession={(id) => {
            if (id === 'fail') return Promise.reject(new Error('Fixture failure'))
            setOpened(id)
            return Promise.resolve()
          }}
        />
      )}
    </AppProviders>
  )
}
createRoot(document.getElementById('root')!).render(<Fixture />)
