import React, { useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { i18n } from '@/app/i18n'
import { applyUiPreferenceAttributes, resolveDarkTheme } from '@/app/ui-preferences'
import { useUiStore } from '@/stores/ui-store'
import { PageFrame } from '@/components/app/PageFrame'
import MarkdownMessage from '@/components/MarkdownMessage'
import { MemoryRouter } from 'react-router-dom'
import { ModelsSettings } from '@/features/config/ModelsSettings'
import { TooltipProvider } from '@/components/ui/tooltip'
import { MemoryPage } from '@/features/memory/MemoryPage'
import { InterfaceSettings } from '@/features/config/InterfaceSettings'
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
      {mounted && (
        <MemoryPage
          notify={notify}
          registerPrimaryAction={registerPrimaryAction}
          requestConfirm={() => Promise.resolve(true)}
        />
      )}
    </>
  )
}
const params = new URLSearchParams(location.search)
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
