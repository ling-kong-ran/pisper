// 应用外壳复刻夹具（第 4 期重写）：真实的 48px 标题栏（AppTitlebar）、手机底部标签栏（ShellTabBar）
// 与命令面板（CommandPalette）。旧的侧栏（AppSidebar）已不挂载，不再出现在这里。
// 页面状态只在内存里：选中的标签、派发次数与命令面板打开的目标都写进 <output>，由冒烟脚本读取。
import { useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Settings } from 'lucide-react'
import { AppProviders } from '@/app/providers'
import { AppTitlebar } from '@/app/AppTitlebar'
import { ShellTabBar } from '@/app/ShellTabBar'
import type { ShellTab } from '@/app/shell-tabs'
import { CommandPalette } from '@/components/layout/AppOverlays'
import { COMMAND_PALETTE_REQUESTED_EVENT, requestCommandPalette } from '@/features/chat/events'
import '@/index.css'

const params = new URLSearchParams(location.search)
const phone = params.get('phone') === '1'
const customTitlebar = params.get('custom') === '1'

const navigation: Array<[string, Array<[string, string, typeof Settings]>]> = [
  ['Fixture', [['config', 'Settings', Settings]]],
]

declare global {
  interface Window {
    shellControls?: {
      setActive: (tab: ShellTab | null) => void
      setHidden: (hidden: boolean) => void
    }
  }
}

export function Fixture() {
  const [open, setOpen] = useState(false)
  const [opened, setOpened] = useState('')
  const [active, setActive] = useState<ShellTab | null>('board')
  const [hidden, setHidden] = useState(false)
  const [newTasks, setNewTasks] = useState(0)
  window.shellControls = { setActive, setHidden }
  // “更多”页搜索框把关键词交给命令面板：与应用壳一样监听请求事件再打开。
  useEffect(() => {
    const show = () => setOpen(true)
    window.addEventListener(COMMAND_PALETTE_REQUESTED_EVENT, show)
    return () => window.removeEventListener(COMMAND_PALETTE_REQUESTED_EVENT, show)
  }, [])
  const select = (tab: ShellTab) => {
    setActive(tab)
    setOpened(`tab:${tab}`)
  }
  return (
    <AppProviders>
      <div
        className="app-shell flex min-h-0 flex-col bg-chrome text-content"
        style={{ height: '100dvh' }}
        data-custom-titlebar={customTitlebar || undefined}
      >
        {!phone && (
          <AppTitlebar
            activeTab={active}
            onSelectTab={select}
            onOpenPalette={() => setOpen(true)}
            onNewTask={() => setNewTasks((count) => count + 1)}
            fleet={<span data-testid="fleet" className="h-8 w-24 flex-none" aria-hidden="true" />}
          />
        )}
        <main className="flex min-h-0 flex-1 flex-col items-start gap-2 bg-background p-4">
          <button data-testid="launch" onClick={() => setOpen(true)}>
            Search
          </button>
          <button data-testid="launch-query" onClick={() => requestCommandPalette('Alpha')}>
            Search Alpha
          </button>
          <output data-testid="opened">{opened}</output>
          <output data-testid="new-tasks">{newTasks}</output>
        </main>
        {phone && (
          <ShellTabBar
            hidden={hidden}
            activeTab={active}
            onSelectTab={select}
            onNewTask={() => setNewTasks((count) => count + 1)}
          />
        )}
      </div>
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
