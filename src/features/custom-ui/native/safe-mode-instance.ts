// 安全模式控制器的页面级单例：宿主核心（host-runtime → ComponentHost）与安全模式询问共用同一个实例。
// 单独成文件，询问组件只依赖它与 safe-mode.ts，不必加载模块加载器、SDK 与 htm（评审返修，计划 §4.3）。
// 惰性创建（首次调用时），模块导入没有副作用。
import { pageStateStorage } from '@/lib/page-state-storage'
import { createSafeModeController, type SafeModeController } from './safe-mode'

let safeMode: SafeModeController | null = null

export function componentSafeMode(): SafeModeController {
  safeMode ??= createSafeModeController({
    storage: pageStateStorage,
    search: typeof window === 'undefined' ? '' : window.location.search,
  })
  return safeMode
}
