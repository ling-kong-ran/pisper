// 首次模型引导：只解释开始对话所需的最短路径，不承载配置表单。
// 各尺寸保持居中；内容过高时允许滚动，并避开屏幕边缘的系统安全区。
import { ArrowRight, Bot, Download, KeyRound, Server, X } from 'lucide-react'
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

type ModelOnboardingDialogProps = {
  open: boolean
  onDismiss: () => void
  onOpenImport: () => void
  onOpenSettings: () => void
}

export function ModelOnboardingDialog({
  open,
  onDismiss,
  onOpenImport,
  onOpenSettings,
}: ModelOnboardingDialogProps) {
  const { t } = useI18n()

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onDismiss()}>
      <DialogContent
        showCloseButton={false}
        overlayClassName="z-[220] bg-black/40"
        className="z-[220] gap-0 p-0"
        style={{
          maxWidth:
            'min(28rem, calc(100vw - max(1rem, env(safe-area-inset-left), env(safe-area-inset-right)) - max(1rem, env(safe-area-inset-left), env(safe-area-inset-right))))',
          maxHeight:
            'calc(100dvh - max(1rem, env(safe-area-inset-top), env(safe-area-inset-bottom)) - max(1rem, env(safe-area-inset-top), env(safe-area-inset-bottom)))',
        }}
        aria-describedby="model-onboarding-description"
      >
        <div className="relative border-b border-border bg-[var(--accent-soft)] px-5 pt-5 pb-4 max-[650px]:px-4 max-[650px]:pt-4">
          <DialogHeader className="gap-3 pr-9 text-left">
            <div className="grid size-11 place-items-center rounded-xl bg-primary text-primary-foreground shadow-sm">
              <Bot className="size-5" />
            </div>
            <div className="space-y-2">
              <p className="text-xs font-semibold tracking-wide text-primary uppercase">
                {t('config:modelOnboarding.eyebrow')}
              </p>
              <DialogTitle className="text-xl leading-7 font-semibold">
                {t('config:modelOnboarding.title')}
              </DialogTitle>
              <DialogDescription id="model-onboarding-description" className="leading-6">
                {t('config:modelOnboarding.description')}
              </DialogDescription>
            </div>
          </DialogHeader>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="absolute top-3 right-3"
            aria-label={t('common:ui.closeDialog')}
            title={t('common:ui.closeDialog')}
            onClick={onDismiss}
          >
            <X />
          </Button>
        </div>

        <div className="grid gap-2.5 px-5 py-4 max-[650px]:px-4">
          <div className="flex items-start gap-3 rounded-xl border border-border bg-muted/50 p-3">
            <span className="grid size-8 shrink-0 place-items-center rounded-lg border border-border bg-background text-primary">
              <Server className="size-4" />
            </span>
            <div className="min-w-0 space-y-0.5">
              <strong className="block text-sm font-semibold">
                {t('config:modelOnboarding.connectionTitle')}
              </strong>
              <p className="text-xs leading-5 text-muted-foreground">
                {t('config:modelOnboarding.connectionDescription')}
              </p>
            </div>
          </div>
          <div className="flex items-start gap-3 rounded-xl border border-border bg-muted/50 p-3">
            <span className="grid size-8 shrink-0 place-items-center rounded-lg border border-border bg-background text-primary">
              <KeyRound className="size-4" />
            </span>
            <div className="min-w-0 space-y-0.5">
              <strong className="block text-sm font-semibold">
                {t('config:modelOnboarding.modelTitle')}
              </strong>
              <p className="text-xs leading-5 text-muted-foreground">
                {t('config:modelOnboarding.modelDescription')}
              </p>
            </div>
          </div>
          <Button
            type="button"
            variant="outline"
            className="w-full max-[650px]:h-10"
            onClick={onOpenImport}
          >
            <Download />
            {t('config:modelOnboarding.findExisting')}
          </Button>
        </div>

        <DialogFooter
          className="m-0 rounded-none px-5 pt-3 max-[650px]:px-4"
          style={{ paddingBottom: 'max(env(safe-area-inset-bottom), 1rem)' }}
        >
          <Button type="button" variant="ghost" className="max-[650px]:h-10" onClick={onDismiss}>
            {t('config:modelOnboarding.later')}
          </Button>
          <Button type="button" className="max-[650px]:h-10" onClick={onOpenSettings}>
            {t('config:modelOnboarding.openSettings')}
            <ArrowRight />
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
