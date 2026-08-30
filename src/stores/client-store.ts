// 客户端形态识别：移动端壳的本地代理会注入 X-Pisper-Client 头，
// runtime 经 /api/client-info 回显。移动端下设置页只提供「服务器」切换，
// 桌面端才显示「远程访问」管理面（发码/吊销）。
import { create } from 'zustand'
import { apiJson } from '@/lib/api'

export type ClientKind = 'web' | 'mobile-app'

function isNativeMobileApp() {
  return typeof window !== 'undefined' && window.__PISPER_MOBILE_APP__ === true
}

type ClientState = {
  client: ClientKind
  loaded: boolean
  load: () => Promise<void>
}

export const useClientStore = create<ClientState>()((set) => ({
  client: isNativeMobileApp() ? 'mobile-app' : 'web',
  loaded: isNativeMobileApp(),
  load: async () => {
    const nativeMobileApp = isNativeMobileApp()
    if (nativeMobileApp) set({ client: 'mobile-app', loaded: true })
    try {
      const info = await apiJson<{ client?: string }>('/api/client-info')
      set({
        client: nativeMobileApp || info.client === 'mobile-app' ? 'mobile-app' : 'web',
        loaded: true,
      })
    } catch {
      // 移动壳标记比 API 握手更早且更可靠；桌面 Web 才在旧 Runtime 时降级为 Web。
      set({ client: nativeMobileApp ? 'mobile-app' : 'web', loaded: true })
    }
  },
}))

export function useIsMobileApp() {
  return useClientStore((state) => state.client === 'mobile-app')
}
