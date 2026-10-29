// 手机外壳判断与应用壳一致：手机 App 或手机视口，且不是桌面壳（窄化的桌面窗口永远不进手机外壳）。
// 看板与会话页的返回入口等需要按外壳换形态的组件共用这一份判断。
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { useClientStore } from '@/stores/client-store'

export function usePhoneShell() {
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  const phoneViewport = useIsPhoneViewport()
  return (mobileApp || phoneViewport) && typeof window !== 'undefined' && !window.pisperDesktop
}
