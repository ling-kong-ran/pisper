import { useCallback, useEffect, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'

export type DialogTone = 'primary' | 'danger'
export type AppDialogState = {
  type: 'confirm' | 'prompt'
  title: string
  message?: string
  inputLabel?: string
  placeholder?: string
  confirmLabel: string
  tone: DialogTone
  value?: string
  maxLength?: number
}

export type ConfirmDialogOptions = Partial<Omit<AppDialogState, 'type'>>
export type PromptDialogOptions = Partial<Omit<AppDialogState, 'type'>>

export function useAppDialog() {
  const { t } = useI18n()
  const [dialog, setDialog] = useState<AppDialogState | null>(null)
  const resolver = useRef<((value: string | boolean | null) => void) | null>(null)

  const finish = useCallback((value: string | boolean | null) => {
    resolver.current?.(value)
    resolver.current = null
    setDialog(null)
  }, [])

  const open = useCallback(
    (next: AppDialogState) =>
      new Promise<string | boolean | null>((resolve) => {
        resolver.current?.(null)
        resolver.current = resolve
        setDialog(next)
      }),
    [],
  )

  const confirm = useCallback(
    (options: ConfirmDialogOptions = {}) =>
      open({
        type: 'confirm',
        title: t('common:useAppDialog.confirmAction'),
        confirmLabel: t('common:useAppDialog.confirm'),
        tone: 'danger',
        ...options,
      }).then((value) => value === true),
    [open, t],
  )
  const prompt = useCallback(
    (options: PromptDialogOptions = {}) =>
      open({
        type: 'prompt',
        title: t('common:useAppDialog.enterContent'),
        confirmLabel: t('common:useAppDialog.save'),
        tone: 'primary',
        value: '',
        ...options,
      }).then((value) => (typeof value === 'string' ? value : null)),
    [open, t],
  )
  const close = useCallback(() => finish(null), [finish])

  useEffect(() => () => resolver.current?.(null), [])
  return { dialog, confirm, prompt, close, finish }
}
