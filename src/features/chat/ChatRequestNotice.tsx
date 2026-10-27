import { useState } from 'react'
import { ChevronDown, Info, LoaderCircle, RotateCcw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Alert } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { cn } from '@/lib/utils'

export function ChatRequestNotice({
  error,
  title,
  pending = false,
  onRetry,
  className,
}: {
  error: string
  title?: string
  pending?: boolean
  onRetry?: () => Promise<void> | void
  className?: string
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [retrying, setRetrying] = useState(false)
  const Icon = pending ? LoaderCircle : Info

  return (
    <Alert
      role="group"
      className={cn(
        '@container min-w-0 border-[var(--stroke-soft)] bg-[var(--surface-subtle)] px-3 py-2.5 text-[var(--text-secondary)]',
        className,
      )}
    >
      <Collapsible open={open} onOpenChange={setOpen} className="min-w-0">
        <div className="flex min-w-0 flex-col gap-2 @min-[420px]:flex-row @min-[420px]:items-center @min-[420px]:gap-3">
          <div
            role="status"
            className="flex min-w-0 flex-1 items-start gap-2 text-[13px] leading-6"
          >
            <Icon
              aria-hidden="true"
              className={cn(
                'mt-1 size-4 shrink-0 text-[var(--text-muted)]',
                pending && 'animate-spin motion-reduce:animate-none',
              )}
            />
            <span className="min-w-0 [overflow-wrap:anywhere]">
              {title || t('chat:requestNotice.incomplete')}
            </span>
          </div>
          <div className="flex min-w-0 flex-wrap items-center justify-end gap-1 @min-[420px]:shrink-0">
            {onRetry && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="min-h-11 shrink-0 gap-1 px-2 text-xs text-[var(--text-secondary)]"
                disabled={pending || retrying}
                onClick={async () => {
                  if (retrying) return
                  setRetrying(true)
                  try {
                    await onRetry()
                  } catch {
                    // 重试失败由会话请求状态呈现；按钮在这里恢复为可操作。
                  } finally {
                    setRetrying(false)
                  }
                }}
              >
                <RotateCcw
                  aria-hidden="true"
                  className={cn('size-3.5', retrying && 'animate-spin')}
                />
                {t('chat:chatMessage.retry')}
              </Button>
            )}
            {error && (
              <CollapsibleTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="min-h-11 shrink-0 gap-1 px-2 text-xs text-[var(--text-muted)]"
                >
                  {open ? t('chat:requestNotice.hideDetails') : t('chat:requestNotice.showDetails')}
                  <ChevronDown
                    aria-hidden="true"
                    className={cn('size-3 transition-transform', open && 'rotate-180')}
                  />
                </Button>
              </CollapsibleTrigger>
            )}
          </div>
        </div>
        <CollapsibleContent>
          <pre
            tabIndex={0}
            aria-label={t('chat:requestNotice.details')}
            className="mt-2 max-h-48 overflow-auto rounded-md border border-[var(--stroke-soft)] bg-[var(--surface-muted)] p-3 text-xs leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          >
            {error}
          </pre>
        </CollapsibleContent>
      </Collapsible>
    </Alert>
  )
}
