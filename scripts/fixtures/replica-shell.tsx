// 应用外壳复刻夹具（规格 v2 第 2.1 节，A1 重写）：真实的侧栏外壳（ShellSidebar，268 / 60 窄轨）、
// 共享页头（PageHeader，Windows 自绘标题栏时右侧让出 144px）、手机底部标签栏（ShellTabBar）与命令面板。
// 会话分组与窄轨计数是看板领域的懒加载块，这里换成固定的占位行，只验外壳本身的结构、尺寸与键盘行为；
// 分组的真实数据与快捷处理在 smoke-board-ui / smoke-sidebar 里覆盖。
// 图形图例：侧栏底色上的 16px 运行中 / 排队 / 今天完成（排队分组要等 Runtime 声明 sessionQueue，
// 真实侧栏里还没有排队行），供冒烟在 1x 截图上逐像素比较三者是否可区分（规格 9.2）。
// 页面状态只在内存里：选中的入口、派发次数与命令面板打开的目标都写进 <output>，由冒烟脚本读取。
import { useEffect, useRef, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Settings } from 'lucide-react'
import { AppProviders } from '@/app/providers'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { ShellSidebar, type ShellSidebarEntry } from '@/app/ShellSidebar'
import { ShellTabBar } from '@/app/ShellTabBar'
import type { ShellTab } from '@/app/shell-tabs'
import { CommandPalette } from '@/components/layout/AppOverlays'
import { PageHeader } from '@/components/layout/PageHeader'
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
  const [entry, setEntry] = useState<ShellSidebarEntry | null>('board')
  const [hidden, setHidden] = useState(false)
  const [newTasks, setNewTasks] = useState(0)
  const searchInputRef = useRef<HTMLInputElement>(null)
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
  const selectEntry = (next: ShellSidebarEntry) => {
    setEntry(next)
    setOpened(`entry:${next}`)
  }
  return (
    <AppProviders>
      <div
        className="app-shell flex min-h-0 flex-col bg-chrome text-content"
        style={{ height: '100dvh' }}
        data-custom-titlebar={customTitlebar || undefined}
      >
        <div className="app-body flex min-h-0 flex-1 overflow-hidden">
          {!phone && (
            <ShellSidebar
              active={entry}
              onNewTask={() => setNewTasks((count) => count + 1)}
              onSearch={() => setOpen(true)}
              onOpenBoard={() => selectEntry('board')}
              onOpenFeatures={() => selectEntry('features')}
              onOpenSettings={() => selectEntry('settings')}
              content={
                <ul data-testid="groups-slot" className="m-0 flex list-none flex-col gap-px p-0">
                  {['修复 CI 失败', '重构登录流程', '升级 Vite 到 7'].map((title) => (
                    <li key={title}>
                      <a href={`#${title}`} className="flex h-9 items-center px-2.5 text-sm">
                        {title}
                      </a>
                    </li>
                  ))}
                </ul>
              }
              railCounts={<span data-testid="rail-counts-slot" className="size-10" />}
            />
          )}
          <main className="relative flex min-w-0 flex-1 flex-col bg-panel">
            {!phone && (
              <PageHeader
                meta={['资产', '']}
                page="assets"
                query=""
                setQuery={() => {}}
                configSection="models"
                onMenu={() => {}}
                onPrimary={() => setOpened('primary')}
                searchInputRef={searchInputRef}
                workflowActions={null}
                desktopPlatform=""
                terminalOpen={false}
                onToggleTerminal={() => {}}
              />
            )}
            <div className="flex flex-col items-start gap-2 p-4">
              <button data-testid="launch" onClick={() => setOpen(true)}>
                Search
              </button>
              <button data-testid="launch-query" onClick={() => requestCommandPalette('Alpha')}>
                Search Alpha
              </button>
              <div data-testid="glyph-legend" className="flex gap-4 bg-sidebar p-2">
                {/* 与侧栏行一致：运行中 phase 0、不转（截图时动画也会停在起点）。 */}
                <StatusGlyph data-legend="run" status="run" spin={false} />
                <StatusGlyph data-legend="queued" status="queued" />
                <StatusGlyph data-legend="done" status="done" />
              </div>
              <output data-testid="opened">{opened}</output>
              <output data-testid="new-tasks">{newTasks}</output>
            </div>
          </main>
        </div>
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
          onToggleTheme={() => setOpened('theme')}
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
