// 真实聊天页与会话目录；网络只访问测试服务器，不读取用户配置或发送模型请求。
import { useCallback, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { ChatPage } from '@/features/chat/pages/ChatPage'
import { useSessionCatalog } from '@/features/chat/hooks/use-session-catalog'

const noop = () => {}
const notify = (message) => window.evidence.notices.push(message)
const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
window.evidence = { notices: [], listOptions: [], configOptions: [], mounts: 0 }

export function CatalogProbe() {
  const catalog = useSessionCatalog({ notify })
  window.controls = catalog
  return (
    <>
      <output data-session-ids={catalog.sessions.map(({ id }) => id).join(',')}>
        {catalog.sessions.map(({ name }) => name).join(',')}
      </output>
      <output data-loading={catalog.loading} data-error={catalog.initializationError} />
    </>
  )
}
export function Fixture() {
  const [mounted, setMounted] = useState(true)
  window.setMounted = setMounted
  const registerPrimaryAction = useCallback(() => noop, [])
  return (
    <QueryClientProvider client={queryClient}>
      {mounted &&
        (location.search.includes('catalog') ? (
          <CatalogProbe />
        ) : (
          <ChatPage
            notify={notify}
            navigate={noop}
            registerPrimaryAction={registerPrimaryAction}
            reportActiveSessionRun={noop}
            pendingAsset={null}
            onAssetConsumed={noop}
            requestText={() => Promise.resolve(null)}
            requestConfirm={() => Promise.resolve(false)}
          />
        ))}
    </QueryClientProvider>
  )
}
createRoot(document.getElementById('root')).render(<Fixture />)
