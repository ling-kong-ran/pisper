// 全局 Provider 组装：React Query（服务端状态缓存）+ 语言偏好 + Tooltip。
// 查询默认 15 秒内不重复请求、失败仅重试一次，窗口失焦不自动刷新，
// 因为桌面端是常驻窗口，频繁 refetch 只会干扰正在进行的 Agent 会话。
import { QueryClientProvider } from '@tanstack/react-query'
import { useEffect, type PropsWithChildren } from 'react'
import { installStartupQueryEvents, queryClient } from '@/lib/startup-queries'
import { TooltipProvider } from '@/components/ui/tooltip'
import { LanguageProvider } from './i18n-provider'

export function AppProviders({ children }: PropsWithChildren) {
  useEffect(() => installStartupQueryEvents(window), [])
  return (
    <QueryClientProvider client={queryClient}>
      <LanguageProvider>
        <TooltipProvider delayDuration={300}>{children}</TooltipProvider>
      </LanguageProvider>
    </QueryClientProvider>
  )
}
