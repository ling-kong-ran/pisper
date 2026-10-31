// 安全模式询问（计划 §4.3）：上次加载组件时页面可能卡死（哨兵还在），这次启动先不加载任何组件，
// 由这里问一次：保持安全模式 / 正常加载 / 全部停用。只在 mode === 'pending' 时出现；
// 应用壳的轻量层（FloatingWidgets）在有已启用实例时才懒加载本文件，选择之前各宿主一直显示安全模式占位卡。
import { useState, useSyncExternalStore } from 'react'
import { useQueryClient } from '@tanstack/react-query'
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
import { listComponentCatalog, setComponentEnabled } from '@/features/custom-ui/custom-ui-api'
import { componentSafeMode } from '@/features/custom-ui/native/host-runtime'
import { customUiQueryKeys } from '@/features/custom-ui/query-keys'

export function SafeModePrompt({ notify }: { notify: (message: string) => void }) {
  const { t } = useI18n()
  const client = useQueryClient()
  const controller = componentSafeMode()
  const mode = useSyncExternalStore(controller.subscribe, controller.mode, controller.mode)
  const [busy, setBusy] = useState(false)
  if (mode !== 'pending') return null
  const disableAll = async () => {
    setBusy(true)
    try {
      const catalog = await listComponentCatalog()
      const enabled = catalog.components.filter((row) => row.kind === 'native' && row.enabled)
      // 逐个停用：一个失败不影响其他组件；失败的数量如实提示，不当作全部成功。
      const results = await Promise.allSettled(
        enabled.map((row) => setComponentEnabled(row.id, false)),
      )
      if (results.some((result) => result.status === 'rejected'))
        notify(t('custom-ui:safeMode.disableFailed'))
      controller.resolve('normal')
      void client.invalidateQueries({ queryKey: customUiQueryKeys.all })
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
