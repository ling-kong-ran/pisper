// 手机外壳判断与应用壳一致：手机 App 或手机视口，且不是桌面壳（窄化的桌面窗口永远不进手机外壳），
// 也不是平板（手机 App 在 ≥960×640 视口里走桌面布局，规格 7.6）。
// 看板与会话页的返回入口等需要按外壳换形态的组件共用这一份判断。
import { useIsPhoneViewport, useIsTabletViewport } from '@/hooks/use-mobile'
import { useClientStore } from '@/stores/client-store'

export function usePhoneShell() {
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  const phoneViewport = useIsPhoneViewport()
  const tablet = useIsTabletViewport() && mobileApp
  return (
    (mobileApp || phoneViewport) &&
    !tablet &&
    typeof window !== 'undefined' &&
    !window.pisperDesktop
  )
}

// 手机 App 的平板形态：桌面布局，但检查器始终是覆盖层、看板窄时只展开需要你与运行中。
export function useTabletApp() {
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  return useIsTabletViewport() && mobileApp
}
