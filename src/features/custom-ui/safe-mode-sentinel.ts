// 安全模式哨兵的偏好键与“有没有哨兵”的同步判断（评审返修）：应用壳的轻量层（FloatingWidgets → placements.ts）
// 只靠它决定要不要懒加载安全模式询问，不经过 hosts.ts，也不把 native/*（htm、宿主核心）拉进来。
// 哨兵是否合法、要不要询问由 native/safe-mode.ts 的控制器判断；这里只看键在不在。
import { pageStateStorage } from '@/lib/page-state-storage'

export const SAFE_MODE_PREFERENCE_KEY = 'pisper-components-safe-mode'

export function hasSafeModeSentinel(): boolean {
  try {
    return pageStateStorage.getItem(SAFE_MODE_PREFERENCE_KEY) !== null
  } catch {
    // 存储不可用时控制器同样读不到哨兵（按“没有哨兵”处理），不必加载询问。
    return false
  }
}
