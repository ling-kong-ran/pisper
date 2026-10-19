import { useEffect, useRef, useState } from 'react'
import { Check, Copy, LoaderCircle } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'

export function MessageCopyButton({ text, copyLabel }: { text: string; copyLabel?: string }) {
  const { t } = useI18n()
  const [state, setState] = useState<'idle' | 'pending' | 'copied' | 'failed'>('idle')
  const request = useRef(0)
  const resetTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    setState('idle')
    return () => {
      // 切换消息或离开页面后，旧的剪贴板请求不能更新新消息的反馈。
      request.current += 1
      if (resetTimer.current !== null) clearTimeout(resetTimer.current)
    }
  }, [text])

  const copy = async () => {
    if (state === 'pending') return
    const current = ++request.current
    if (resetTimer.current !== null) clearTimeout(resetTimer.current)
    setState('pending')
    try {
      await navigator.clipboard.writeText(text)
      if (current !== request.current) return
      setState('copied')
      resetTimer.current = setTimeout(() => setState('idle'), 1800)
    } catch {
      if (current === request.current) setState('failed')
    }
  }

  const label =
    state === 'copied'
      ? t('common:markdownMessage.copied')
      : (copyLabel ?? t('chat:chatMessage.copyReply'))
  return (
    <>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={label}
            disabled={state === 'pending'}
            onClick={() => void copy()}
          >
            {state === 'copied' ? (
              <Check size={16} />
            ) : state === 'pending' ? (
              <LoaderCircle size={16} className="animate-spin" />
            ) : (
              <Copy size={16} />
            )}
          </Button>
        </TooltipTrigger>
        <TooltipContent>{label}</TooltipContent>
      </Tooltip>
      <span
        role="status"
        className={
          state === 'failed' ? 'order-last basis-full text-xs text-destructive' : 'sr-only'
        }
      >
        {state === 'failed'
          ? t('chat:chatMessage.copyReplyFailed')
          : state === 'copied'
            ? t('common:markdownMessage.copied')
            : ''}
      </span>
    </>
  )
}
