import { useState } from 'react'
import { ChevronDown, Info, RotateCcw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Alert } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
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

  return (
    <Alert
      role="group"
      className={cn(
        'min-w-0 rounded-md border-stroke bg-surface-subtle px-3 py-2 text-content-secondary',
        className,
      )}
    >
      <Collapsible open={open} onOpenChange={setOpen}>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <div role="status" className="flex min-w-0 flex-1 items-center gap-2 text-[13px]">
            {/* 等待中用品牌轨道指示，静止提示用中性信息图标。 */}
            {pending ? (
              <OrbitSpinner className="size-4" />
            ) : (
              <Info aria-hidden="true" className="size-4 shrink-0 text-content-muted" />
            )}
            <span>{title || t('chat:requestNotice.incomplete')}</span>
          </div>
          {onRetry && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 shrink-0 gap-1 px-2 text-xs text-content-secondary max-[650px]:min-h-11 pointer-coarse:min-h-11"
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
                className="h-7 shrink-0 gap-1 px-2 text-xs text-content-muted max-[650px]:min-h-11 pointer-coarse:min-h-11"
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
        <CollapsibleContent>
          <pre
            tabIndex={0}
            aria-label={t('chat:requestNotice.details')}
            className="mt-2 max-h-48 overflow-auto rounded-sm border border-stroke-soft bg-code p-[8px_10px] font-mono text-[12px] leading-[1.6] whitespace-pre-wrap text-content-soft [overflow-wrap:anywhere] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
          >
            {error}
          </pre>
        </CollapsibleContent>
      </Collapsible>
    </Alert>
  )
}
