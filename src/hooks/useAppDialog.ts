// 应用级确认/输入对话框 hook：以 Promise 形式暴露 confirm/prompt，
// 供非组件代码（工具调用、SSE 事件处理）同步等待用户输入。
// 打开新对话框时先释放上一个 pending resolver（返回 null）。
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

// 应用对话框（确认/输入）hook：以 Promise 暴露 confirm/prompt，
// 非组件代码可 await 用户输入；连续打开时旧 pending 返回 null。
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
