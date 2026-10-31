import React, { useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { ensureBoardMessages, ensureCustomUiMessages, i18n } from '@/app/i18n'
import { ChatAppearancePage } from '@/app/ChatAppearancePage'
import { applyUiPreferenceAttributes, resolveDarkTheme } from '@/app/ui-preferences'
import { useUiStore } from '@/stores/ui-store'
import { PageFrame } from '@/components/app/PageFrame'
import MarkdownMessage from '@/components/MarkdownMessage'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { ModelsSettings } from '@/features/config/ModelsSettings'
import { TooltipProvider } from '@/components/ui/tooltip'
import { MemoryPage } from '@/features/memory/MemoryPage'
import { InterfaceSettings } from '@/features/config/InterfaceSettings'
import { ShortcutSettings } from '@/features/config/ShortcutSettings'
import { requestConfigCardHighlight, useConfigCardHighlight } from '@/features/config/config-search'

const notices: string[] = []
Object.assign(window, { replicaNotices: notices })
const notify = (message: string) => {
  notices.push(message)
}
const registerPrimaryAction = () => () => {}
export function SearchFixture() {
  const target = useConfigCardHighlight('models')
  const [open, setOpen] = useState(false)
  const [ready, setReady] = useState(false)
  useEffect(() => {
    if (target?.expandGroup === 'models-advanced') setOpen(true)
  }, [target])
  return (
    <>
      <button
        onClick={() =>
          requestConfigCardHighlight('models-advanced', 'models', {
            targetId: 'models-runtime-policy',
            expandGroup: 'models-advanced',
            targetText: 'policy',
          })
        }
      >
        Search policy
      </button>
      <button onClick={() => setReady(true)}>Finish loading</button>
      <button
        onClick={() =>
          requestConfigCardHighlight('models-current-model', 'models', {
            targetId: 'models-current-model',
          })
        }
      >
        Search current
      </button>
      <div data-config-card="section">
        <div data-config-target="models-current-model">
          <input aria-label="Current model" />
        </div>
        {open && ready && (
          <div data-config-target="models-runtime-policy">
            <input aria-label="Runtime policy" />
          </div>
        )}
      </div>
    </>
  )
}
export function ModelFixture() {
  const target = useConfigCardHighlight('models')
  return (
    <MemoryRouter>
      <button
        onClick={() =>
          requestConfigCardHighlight('models-advanced', 'models', {
            targetId: 'models-runtime-policy',
            expandGroup: 'models-advanced',
          })
        }
      >
        Locate real policy
      </button>
      <button
        onClick={() =>
          requestConfigCardHighlight('models-visual', 'models', {
            targetId: 'models-visual',
            expandGroup: 'models-advanced',
          })
        }
      >
        Locate real visual
      </button>
      <ModelsSettings
        notify={notify}
        registerPrimaryAction={registerPrimaryAction}
        requestConfirm={() => Promise.resolve(true)}
        defaultAdvancedExpanded={target?.expandGroup === 'models-advanced'}
        navigationKey={target?.navigationKey}
      />
    </MemoryRouter>
  )
}
export function MemoryFixture() {
  const [mounted, setMounted] = useState(true)
  return (
    <>
      <button onClick={() => setMounted(false)}>Unmount memory</button>
      {mounted && <MemoryPage notify={notify} requestConfirm={() => Promise.resolve(true)} />}
    </>
  )
}
// 第 4 期设置：界面页顶部“组件已移到 组件 页 →”（取代“独立组件”标签，旧 ?view=widgets 链接重定向到组件页）。
export function InterfacePageFixture() {
  const legacy = params.get('legacy') === 'widgets'
  return (
    <MemoryRouter initialEntries={[`/config/interface${legacy ? '?view=widgets' : ''}`]}>
      <Routes>
        <Route
          path="/config/interface"
          element={
            <PageFrame variant="settings">
              <ChatAppearancePage appearance={<InterfaceSettings notify={notify} />} />
            </PageFrame>
          }
        />
        <Route path="/components" element={<p data-testid="components-route">components</p>} />
      </Routes>
    </MemoryRouter>
  )
}
const params = new URLSearchParams(location.search)
// 新动作（代理面板、检查器、切换代理、插话、速查、打开看板）的文案在看板命名空间，组件跳转在组件命名空间：
// 与设置路由一样先加载再渲染。
await Promise.all([ensureBoardMessages(), ensureCustomUiMessages()])
if (params.get('longLabels') === '1') {
  // 仅隔离 fixture 的文案压力输入，不改变产品资源或把测试文案当成参考取证。
  const labels = {
    'interfaceSettings.followSystem': '跟随系统自动切换并同步当前设备的深浅外观',
    'interfaceSettings.light': '浅色模式，适合明亮环境与较长中文标签',
    'interfaceSettings.dark': '深色模式，适合夜间阅读与较长中文标签',
    'interfaceSettings.colorMode': '当前界面颜色模式与系统外观偏好（长中文标签验证）',
    'interfaceSettings.accentColor': '强调色在当前界面中的显示效果（长中文标签验证）',
  }
  for (const [key, value] of Object.entries(labels)) i18n.addResource('zh-CN', 'config', key, value)
}
export function Fixture() {
  const preferences = useUiStore()
  const {
    accent,
    density,
    fontScale,
    motion,
    radius,
    customAccent,
    theme,
    uiFontSize,
    codeFontSize,
  } = preferences
  useEffect(() => {
    applyUiPreferenceAttributes({
      accent,
      density,
      fontScale,
      motion,
      radius,
      customAccent,
      uiFontSize,
      codeFontSize,
    })
  }, [accent, density, fontScale, motion, radius, customAccent, uiFontSize, codeFontSize])
  useEffect(() => {
    const media = matchMedia('(prefers-color-scheme: dark)')
    const applyTheme = () => {
      const dark = resolveDarkTheme(theme, media.matches)
      document.documentElement.dataset.theme = dark ? 'dark' : 'light'
      document.documentElement.classList.toggle('dark', dark)
    }
    applyTheme()
    if (theme === 'system') {
      media.addEventListener('change', applyTheme)
      return () => media.removeEventListener('change', applyTheme)
    }
    if (theme === 'scheduled') {
      const timer = setInterval(applyTheme, 60_000)
      return () => clearInterval(timer)
    }
  }, [theme])
  const view = params.get('view')
  return (
    <TooltipProvider>
      {view === 'models' ? (
        <ModelFixture />
      ) : view === 'interface-page' ? (
        <InterfacePageFixture />
      ) : view === 'shortcuts' ? (
        <PageFrame variant="settings">
          <ShortcutSettings requestConfirm={() => Promise.resolve(true)} />
        </PageFrame>
      ) : view === 'search' ? (
        <SearchFixture />
      ) : view === 'appearance' || view === 'typography' ? (
        <PageFrame variant="settings">
          <InterfaceSettings notify={notify} />
          {view === 'typography' && (
            <div data-testid="typography-sample" className="text-[length:var(--font-content-size)]">
              <MarkdownMessage>
                {'中文正文与 English content.\n\n```\nconst value = 42\n```'}
              </MarkdownMessage>
            </div>
          )}
        </PageFrame>
      ) : (
        <MemoryFixture />
      )}
    </TooltipProvider>
  )
}
createRoot(document.getElementById('root')!).render(<Fixture />)
