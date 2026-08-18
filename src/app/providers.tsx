// 全局 Provider 组装：React Query（服务端状态缓存）+ 语言偏好 + Tooltip。
// 查询默认 15 秒内不重复请求、失败仅重试一次，窗口失焦不自动刷新，
// 因为桌面端是常驻窗口，频繁 refetch 只会干扰正在进行的 Agent 会话。
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { PropsWithChildren } from 'react'
import { TooltipProvider } from '@/components/ui/tooltip'
import { LanguageProvider } from './i18n-provider'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      retry: 1,
      staleTime: 15_000,
    },
    mutations: {
      retry: 0,
    },
  },
})

export function AppProviders({ children }: PropsWithChildren) {
  return (
    <QueryClientProvider client={queryClient}>
      <LanguageProvider>
        <TooltipProvider delayDuration={300}>{children}</TooltipProvider>
      </LanguageProvider>
    </QueryClientProvider>
  )
}
