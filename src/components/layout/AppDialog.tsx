// 全局确认/输入对话框组件：由 useAppDialog 驱动，展示在应用最上层。
// 输入框受控聚焦，Enter 确认、Esc 关闭；输入值在提交时回调 onFinish。
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import type { AppDialogState } from '@/hooks/useAppDialog'

export function AppDialog({
  dialog,
  onClose,
  onFinish,
}: {
  dialog: AppDialogState | null
  onClose: () => void
  onFinish: (value: string | boolean | null) => void
}) {
  const { t } = useI18n()
  const [value, setValue] = useState(dialog?.value || '')
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    setValue(dialog?.value || '')
  }, [dialog])

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!dialog) return
    onFinish(dialog.type === 'prompt' ? value.trim() : true)
  }

  return (
    <Dialog open={Boolean(dialog)} onOpenChange={(open) => !open && onClose()}>
      {dialog && (
        <DialogContent
          showCloseButton={false}
          className="max-w-[430px] gap-0 rounded-dialog border border-surface-highlight bg-popover p-[18px] text-popover-foreground shadow-dialog ring-0"
          onOpenAutoFocus={(event) => {
            if (dialog.type !== 'prompt') return
            event.preventDefault()
            inputRef.current?.focus()
            inputRef.current?.select()
          }}
        >
          <form onSubmit={submit}>
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0">
                <DialogTitle className="text-[17px] leading-6 font-bold tracking-[0]">
                  {dialog.title}
                </DialogTitle>
                <DialogDescription
                  className={
                    dialog.message ? 'mt-1.5 text-[13px] leading-5 text-content-muted' : 'sr-only'
                  }
                >
                  {dialog.message || dialog.title}
                </DialogDescription>
              </div>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                aria-label={t('common:ui.closeDialog')}
                title={t('common:ui.closeDialog')}
                className="size-8 shrink-0 text-content-muted"
                onClick={onClose}
              >
                <X className="size-[18px]" />
              </Button>
            </div>
            {dialog.type === 'prompt' && (
              <Label className="mt-4 block text-[12px] leading-5 font-semibold text-content-soft">
                {dialog.inputLabel || t('common:ui.name')}
                <Input
                  ref={inputRef}
                  className="mt-2 h-10 rounded-lg border-border bg-background px-3 text-[13px]"
                  value={value}
                  placeholder={dialog.placeholder}
                  maxLength={dialog.maxLength || 120}
                  onChange={(event) => setValue(event.target.value)}
                />
              </Label>
            )}
            <div className="mt-[18px] flex justify-end gap-2">
              <Button type="button" variant="secondary" onClick={onClose}>
                {t('common:ui.cancel')}
              </Button>
              <Button
                type="submit"
                variant={dialog.tone === 'danger' ? 'destructive' : 'secondary'}
                disabled={dialog.type === 'prompt' && !value.trim()}
              >
                {dialog.confirmLabel || t('common:ui.confirm')}
              </Button>
            </div>
          </form>
        </DialogContent>
      )}
    </Dialog>
  )
}
