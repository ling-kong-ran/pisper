// 浮窗放置的轻量层（计划 §4.5 第 5 项）：由应用壳持有，路由与会话切换只更新页面内容，不重建组件与计时器。
// 启动时总会加载，所以这里只用 placements.ts（一次 GET /api/custom-ui/instances）：
// floating 实例数 > 0 且不在手机外壳（负责人决定：390 宽会挡内容，实例保留、桌面与平板照常显示）时才懒加载宿主；
// 有已启用实例、且偏好里留着安全模式哨兵（上次加载组件可能卡死）时才懒加载询问。没有实例时什么都不加载。
import { Suspense, useEffect, useState } from 'react'
import { useLocation } from 'react-router-dom'
import {
  hasSafeModeSentinel,
  LazyFloatingComponents,
  LazySafeModePrompt,
  useComponentInstances,
  useDisableAllComponents,
  useEnabledComponentInstanceCount,
} from '@/features/custom-ui/placements'
import { PAGE_PATHS } from '@/app/routes'
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

export function FloatingWidgets({ notify }: { notify: (message: string) => void }) {
  const phone = usePhoneShell()
  // 组件工作室里浮窗层隐藏（仍挂着，状态与计时器不丢）：工作室有自己的浮窗预览档位，右下角是设置栏的
  // [放置到… ▾]，浮窗压在正在编辑的组件与操作上（评审返修）。离开工作室原样出现。
  const pathname = useLocation().pathname
  const inStudio =
    pathname === PAGE_PATHS.components || pathname.startsWith(`${PAGE_PATHS.components}/`)
  const floating = useComponentInstances('floating')
  const enabledCount = useEnabledComponentInstanceCount()
  // 只看启动时：之后的哨兵是本页宿主自己写的，询问只针对上一次启动留下的。
  const [staleSentinel] = useState(hasSafeModeSentinel)
  const disableAllComponents = useDisableAllComponents()
  useLegacyFloatingCleanup()
  return (
    <>
      {staleSentinel && enabledCount > 0 && (
        <Suspense fallback={null}>
          <LazySafeModePrompt notify={notify} disableAll={disableAllComponents} />
        </Suspense>
      )}
      {!phone && floating.placed.length > 0 && (
        <Suspense fallback={null}>
          <LazyFloatingComponents placed={floating.placed} hidden={inStudio} notify={notify} />
        </Suspense>
      )}
    </>
  )
}
