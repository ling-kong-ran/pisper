// 安全模式询问（计划 §4.3）：上次加载组件时页面可能卡死（哨兵还在），这次启动先不加载任何组件，
// 由这里问一次：保持安全模式 / 正常加载 / 全部停用。只在 mode === 'pending' 时出现；
// 应用壳的轻量层（FloatingWidgets）在有已启用实例且偏好里有哨兵时才懒加载本文件（独立的动态入口，
// 不经过 hosts.ts、不加载宿主核心与 htm），选择之前各宿主一直显示安全模式占位卡。
import { useState, useSyncExternalStore } from 'react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { componentSafeMode } from '@/features/custom-ui/native/safe-mode-instance'

export function SafeModePrompt({
  notify,
  disableAll: disableAllComponents,
}: {
  notify: (message: string) => void
  // 停用全部已启用的原生组件，返回是否全部成功（请求与缓存作废由轻量层 placements.ts 提供：
  // 询问本身只依赖控制器与对话框，懒加载时不再拆出额外的共享块）。
  disableAll: () => Promise<boolean>
}) {
  const { t } = useI18n()
  const controller = componentSafeMode()
  const mode = useSyncExternalStore(controller.subscribe, controller.mode, controller.mode)
  const [busy, setBusy] = useState(false)
  if (mode !== 'pending') return null
  const disableAll = async () => {
    setBusy(true)
    try {
      // 逐个停用：一个失败不影响其他组件；失败如实提示，不当作全部成功。
      if (!(await disableAllComponents())) notify(t('custom-ui:safeMode.disableFailed'))
      controller.resolve('normal')
    } catch {
      notify(t('custom-ui:safeMode.disableFailed'))
    } finally {
      setBusy(false)
    }
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        // Esc / 点遮罩 = 保持安全模式：不在用户没选的情况下重新加载可能卡死的组件。
        if (!open && !busy) controller.resolve('safe')
      }}
    >
      <DialogContent data-components-safe-mode-prompt>
        <DialogHeader>
          <DialogTitle>{t('custom-ui:safeMode.promptTitle')}</DialogTitle>
          <DialogDescription>{t('custom-ui:safeMode.promptHint')}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={() => void disableAll()}>
            {t('custom-ui:safeMode.disableAll')}
          </Button>
          <Button variant="outline" disabled={busy} onClick={() => controller.resolve('normal')}>
            {t('custom-ui:safeMode.loadNormally')}
          </Button>
          <Button disabled={busy} onClick={() => controller.resolve('safe')}>
            {t('custom-ui:safeMode.keepSafe')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
