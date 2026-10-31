// 首次模型引导：只解释开始对话所需的最短路径，不承载配置表单。
// 各尺寸保持居中；内容过高时允许滚动，并避开屏幕边缘的系统安全区。
// 方向 A（规格 6.1、6.4，A6 第 4 批）：r-lg 实心弹层、与其他对话框同一层遮罩；两步说明是细线分隔的编号行，
// 不是两张带底色的小卡片；眉题用次要灰，强调色只留给主操作。
import { ArrowRight, Download, KeyRound, Server, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
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
        overlayClassName="z-[220]"
        className="z-[220] gap-0 rounded-[var(--r-lg)] border border-stroke bg-solid p-0"
        style={{
          maxWidth:
            'min(28rem, calc(100vw - max(1rem, env(safe-area-inset-left), env(safe-area-inset-right)) - max(1rem, env(safe-area-inset-left), env(safe-area-inset-right))))',
          maxHeight:
            'calc(100dvh - max(1rem, env(safe-area-inset-top), env(safe-area-inset-bottom)) - max(1rem, env(safe-area-inset-top), env(safe-area-inset-bottom)))',
        }}
        aria-describedby="model-onboarding-description"
      >
        {/* 品牌时刻：单色星标键帽，强调色只留给主操作。 */}
        <div className="relative px-5 pt-5 pb-2 max-[650px]:px-4 max-[650px]:pt-4">
          <DialogHeader className="gap-3 pr-9 text-left">
            <span aria-hidden="true" className="w-fit rounded-[13px] shadow-raise">
              <BrandLogo size={44} />
            </span>
            <div className="space-y-1.5">
              <p className="text-xs font-medium text-content-muted">
                {t('config:modelOnboarding.eyebrow')}
              </p>
              <DialogTitle className="text-[length:calc(var(--ui-font-size)*10/7)] leading-[1.3] font-semibold tracking-[-.015em]">
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
            className="absolute top-3 right-3 pointer-coarse:size-11"
            aria-label={t('common:ui.closeDialog')}
            title={t('common:ui.closeDialog')}
            onClick={onDismiss}
          >
            <X />
          </Button>
        </div>

        <div className="grid gap-3 px-5 pt-2 pb-4 max-[650px]:px-4">
          <ol className="m-0 list-none divide-y divide-stroke-soft border-y border-stroke-soft p-0">
            {[
              {
                Icon: Server,
                title: t('config:modelOnboarding.connectionTitle'),
                description: t('config:modelOnboarding.connectionDescription'),
              },
              {
                Icon: KeyRound,
                title: t('config:modelOnboarding.modelTitle'),
                description: t('config:modelOnboarding.modelDescription'),
              },
            ].map(({ Icon, title, description }, index) => (
              <li key={title} className="flex items-start gap-3 py-3">
                <span
                  aria-hidden="true"
                  className="grid size-7 shrink-0 place-items-center rounded-full bg-surface-subtle text-xs font-medium text-content-secondary tabular-nums"
                >
                  {index + 1}
                </span>
                <div className="min-w-0 space-y-0.5">
                  <strong className="flex items-center gap-1.5 text-sm font-medium text-content">
                    <Icon className="size-3.5 text-content-muted" aria-hidden="true" />
                    {title}
                  </strong>
                  <p className="text-xs leading-5 text-content-muted">{description}</p>
                </div>
              </li>
            ))}
          </ol>
          <Button
            type="button"
            variant="secondary"
            className="w-full max-[650px]:h-11"
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
          <Button type="button" variant="ghost" className="max-[650px]:h-11" onClick={onDismiss}>
            {t('config:modelOnboarding.later')}
          </Button>
          <Button type="button" className="max-[650px]:h-11" onClick={onOpenSettings}>
            {t('config:modelOnboarding.openSettings')}
            <ArrowRight />
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
