// 浮窗放置的轻量层（计划 §4.5 第 5 项）：由应用壳持有，路由与会话切换只更新页面内容，不重建组件与计时器。
// 启动时总会加载，所以这里只用 placements.ts（一次 GET /api/custom-ui/instances）：
// floating 实例数 > 0 且不在手机外壳（负责人决定：390 宽会挡内容，实例保留、桌面与平板照常显示）时才懒加载宿主；
// 有任何已启用实例时才懒加载安全模式询问（上次加载组件可能卡死时出现）。没有实例时什么都不加载。
import { Suspense, useEffect, type RefObject } from 'react'
import {
  LazyFloatingComponents,
  LazySafeModePrompt,
  useComponentInstances,
  useEnabledComponentInstanceCount,
} from '@/features/custom-ui/placements'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { pageStateStorage } from '@/lib/page-state-storage'

// 旧专注岛 / iframe 组件的“悬浮显示”开关（已不再读）。负责人决定之前的组件都删除：偏好里的旧值清掉一次。
// 键仍登记在 shared/browser-preferences.mjs：旧快照带着它时解析不至于整体失败。
const LEGACY_FLOATING_KEY = 'pisper-floating-widgets'

function useLegacyFloatingCleanup() {
  useEffect(() => {
    try {
      // 只在有旧值时写一次，避免每次启动都向 /api/local 镜像发一次删除。
      if (pageStateStorage.getItem(LEGACY_FLOATING_KEY) !== null)
        pageStateStorage.removeItem(LEGACY_FLOATING_KEY)
    } catch {
      // 存储不可用时留着它：已经没有读取方，只是一条孤儿键。
    }
  }, [])
}

export function FloatingWidgets({
  anchorRef,
  notify,
}: {
  anchorRef: RefObject<HTMLElement | null>
  notify: (message: string) => void
}) {
  const phone = usePhoneShell()
  const floating = useComponentInstances('floating')
  const enabledCount = useEnabledComponentInstanceCount()
  useLegacyFloatingCleanup()
  return (
    <>
      {enabledCount > 0 && (
        <Suspense fallback={null}>
          <LazySafeModePrompt notify={notify} />
        </Suspense>
      )}
      {!phone && floating.placed.length > 0 && (
        <Suspense fallback={null}>
          <LazyFloatingComponents placed={floating.placed} anchorRef={anchorRef} notify={notify} />
        </Suspense>
      )}
    </>
  )
}
