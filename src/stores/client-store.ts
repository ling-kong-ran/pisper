// 客户端形态识别：移动端壳的本地代理会注入 X-Pisper-Client 头，
// runtime 经 /api/client-info 回显。移动端下设置页只提供「服务器」切换，
// 桌面端才显示「远程访问」管理面（发码/吊销）。
import { create } from 'zustand'
import { apiJson } from '@/lib/api'

export type ClientKind = 'web' | 'mobile-app'

type ClientState = {
  client: ClientKind
  loaded: boolean
  load: () => Promise<void>
}

export const useClientStore = create<ClientState>()((set) => ({
  client: 'web',
  loaded: false,
  load: async () => {
    try {
      const info = await apiJson<{ client?: string }>('/api/client-info')
      set({ client: info.client === 'mobile-app' ? 'mobile-app' : 'web', loaded: true })
    } catch {
      // 老版本 runtime 没有该接口：一律按 Web 处理。
      set({ client: 'web', loaded: true })
    }
  },
}))

export function useIsMobileApp() {
  return useClientStore((state) => state.client === 'mobile-app')
}
