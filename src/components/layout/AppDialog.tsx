// 全局确认/输入对话框组件：由 useAppDialog 驱动，展示在应用最上层。
// 输入框受控聚焦，Enter 确认、Esc 关闭；输入值在提交时回调 onFinish。
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import type { AppDialogState } from '@/hooks/useAppDialog'

export function AppDialog({
  dialog,
  onClose,
  onFinish,
  busy = false,
}: {
  dialog: AppDialogState | null
  onClose: () => void
  onFinish: (value: string | boolean | null) => void | Promise<void>
  busy?: boolean
}) {
  const { t } = useI18n()
  const [value, setValue] = useState(dialog?.value || '')
  const inputRef = useRef<HTMLInputElement>(null)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState('')
  const pendingRef = useRef<AppDialogState | null>(null)
  const dialogRef = useRef(dialog)
  dialogRef.current = dialog
  const isBusy = busy || submitting

  useEffect(
    () => () => {
      dialogRef.current = null
    },
    [],
  )

  useEffect(() => {
    setValue(dialog?.value || '')
    setError('')
    setSubmitting(false)
    pendingRef.current = null
  }, [dialog])

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!dialog || isBusy || pendingRef.current === dialog) return
    if (dialog.type === 'prompt' && !value.trim()) return
    const submittedDialog = dialog
    pendingRef.current = dialog
    setSubmitting(true)
    setError('')
    try {
      const result = onFinish(dialog.type === 'prompt' ? value.trim() : true)
      if (result) {
        await result
        if (dialogRef.current === submittedDialog) onClose()
      }
    } catch {
      if (dialogRef.current === submittedDialog) setError(t('common:ui.actionFailed'))
    } finally {
      if (pendingRef.current === submittedDialog) pendingRef.current = null
      if (dialogRef.current === submittedDialog) setSubmitting(false)
    }
  }

  return (
    <Dialog open={Boolean(dialog)} busy={isBusy} onOpenChange={(open) => !open && onClose()}>
      {dialog && (
        <DialogContent
          showCloseButton={false}
          size="compact"
          surface="confirmation"
          scrollMode="body"
          className="gap-0 p-6"
          onOpenAutoFocus={(event) => {
            if (dialog.type !== 'prompt') return
            event.preventDefault()
            inputRef.current?.focus()
            inputRef.current?.select()
          }}
        >
          <form className="flex min-h-0 flex-col" onSubmit={(event) => void submit(event)}>
            <div className="flex shrink-0 items-start justify-between gap-3">
              <div className="min-w-0">
                <DialogTitle className="text-lg leading-6 font-medium tracking-[0]">
                  {dialog.title}
                </DialogTitle>
              </div>
              <Button
                type="button"
                variant="ghost"
                size="toolbar"
                aria-label={t('common:ui.closeDialog')}
                title={t('common:ui.closeDialog')}
                className="size-6 shrink-0 px-0 text-muted-foreground"
                disabled={isBusy}
                onClick={onClose}
              >
                <X className="size-[18px]" />
              </Button>
            </div>
            <DialogBody>
              <DialogDescription
                className={
                  dialog.message ? 'mt-2 text-sm leading-5 text-muted-foreground' : 'sr-only'
                }
              >
                {dialog.message || dialog.title}
              </DialogDescription>
              {dialog.type === 'prompt' && (
                <Label className="mt-4 block text-[12px] leading-5 font-semibold text-content-soft">
                  {dialog.inputLabel || t('common:ui.name')}
                  <Input
                    ref={inputRef}
                    className="mt-2 h-10 rounded-lg border-border bg-background px-3 text-[13px]"
                    disabled={isBusy}
                    value={value}
                    placeholder={dialog.placeholder}
                    maxLength={dialog.maxLength || 120}
                    onChange={(event) => setValue(event.target.value)}
                  />
                </Label>
              )}
              {error && (
                <p role="alert" className="mt-3 text-sm text-danger">
                  {error}
                </p>
              )}
            </DialogBody>
            <div className="mt-6 flex shrink-0 justify-end gap-2">
              <Button
                type="button"
                variant="secondary"
                size="dialogAction"
                disabled={isBusy}
                onClick={onClose}
              >
                {t('common:ui.cancel')}
              </Button>
              <Button
                type="submit"
                size="dialogAction"
                loading={isBusy}
                variant={dialog.tone === 'danger' ? 'destructive' : 'default'}
                className={dialog.tone === 'danger' ? undefined : 'text-primary-foreground'}
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
