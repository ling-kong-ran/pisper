// 真实资产页面；只在 HTTP 边界提供合成数据，不启动 Runtime。
import { createRoot } from 'react-dom/client'
import { useCallback, useRef, useState } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { AssetsPage } from '@/features/assets/AssetsPage'
import { i18n } from '@/app/i18n'
import { applyUiPreferenceAttributes } from '@/app/ui-preferences'
import type { ChatAttachment } from '@/types/chat'
import '@/index.css'

const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
const evidence: { used: ChatAttachment[]; notices: string[] } = { used: [], notices: [] }
Object.assign(window, { assetEvidence: evidence })
export function Fixture() {
  const [query, setQuery] = useState('')
  const [session, setSession] = useState<string | null>('a')
  const [mounted, setMounted] = useState(true)
  const primary = useRef<() => void>(() => {})
  const register = useCallback((action: () => void) => {
    primary.current = action
    return () => {
      primary.current = () => {}
    }
  }, [])
  Object.assign(window, {
    assetFixture: {
      setQuery,
      appearance: (large: boolean, dark: boolean) => {
        applyUiPreferenceAttributes({
          accent: 'neutral',
          density: 'comfortable',
          fontScale: large ? 'large' : 'default',
          uiFontSize: large ? 16 : 14,
          motion: 'full',
          radius: 'default',
        })
        document.documentElement.dataset.theme = dark ? 'dark' : 'light'
        document.documentElement.classList.toggle('dark', dark)
      },
    },
  })
  return (
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <div className="flex h-dvh min-h-0 flex-col">
          <header className="flex shrink-0 flex-wrap gap-2 p-2">
            <input
              aria-label="Fixture search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <button onClick={() => setSession('a')}>Session A</button>
            <button onClick={() => setSession('b')}>Session B</button>
            <button onClick={() => setSession(null)}>No session</button>
            <button onClick={() => primary.current()}>New link</button>
            <button onClick={() => setMounted((v) => !v)}>Mount</button>
            <button
              onClick={() => {
                void i18n.changeLanguage(i18n.language === 'zh-CN' ? 'en-US' : 'zh-CN')
              }}
            >
              Language
            </button>
            <button
              onClick={() => {
                const dark = document.documentElement.dataset.theme !== 'dark'
                document.documentElement.dataset.theme = dark ? 'dark' : 'light'
                document.documentElement.classList.toggle('dark', dark)
              }}
            >
              Theme
            </button>
          </header>
          <main className="min-h-0 flex-1 overflow-hidden">
            {mounted && (
              <AssetsPage
                query={query}
                activeSessionId={session}
                notify={(message) => evidence.notices.push(message)}
                registerPrimaryAction={register}
                onUse={(asset) => evidence.used.push(asset)}
                requestConfirm={() => Promise.resolve(true)}
              />
            )}
          </main>
        </div>
      </TooltipProvider>
    </QueryClientProvider>
  )
}
createRoot(document.getElementById('root')!).render(<Fixture />)
