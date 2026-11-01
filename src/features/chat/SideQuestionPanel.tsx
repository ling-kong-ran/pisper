// 旁问面板：输入框上方显示“/btw”的问题与流式回答。回答不进会话，关闭即丢弃；
// 主会话运行中也能问，面板和主转录互不影响。
import { MessageCircleQuestion, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import MarkdownMessage from '@/components/MarkdownMessage'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import type { SideQuestionState } from './useSideQuestion'

export function SideQuestionPanel({
  state,
  cwd,
  onDismiss,
}: {
  state: SideQuestionState
  cwd?: string
  onDismiss: () => void
}) {
  const { t } = useI18n()
  const streaming = state.status === 'streaming'
  return (
    <section
      aria-label={t('chat:sideQuestion.label')}
      aria-busy={streaming}
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return
        event.preventDefault()
        event.stopPropagation()
        onDismiss()
      }}
      className="flex max-h-[min(45vh,420px)] min-w-0 flex-col rounded-lg border border-stroke bg-panel shadow-float"
    >
      <header className="flex min-w-0 items-start gap-2 border-b border-stroke-soft px-3 py-2">
        <MessageCircleQuestion
          size={15}
          aria-hidden="true"
          className="mt-0.5 shrink-0 text-content-muted"
        />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="text-xs text-content-muted">{t('chat:sideQuestion.label')}</span>
          <p className="m-0 line-clamp-2 text-[13px] text-content" title={state.question}>
            {state.question}
          </p>
        </div>
        <Button
          variant="ghost"
          size="icon-xs"
          className="shrink-0 pointer-coarse:size-11"
          aria-label={t('chat:sideQuestion.close')}
          title={t('chat:sideQuestion.close')}
          onClick={onDismiss}
        >
          <X />
        </Button>
      </header>
      <div className="min-h-0 overflow-y-auto overscroll-contain px-3 py-2 text-[13px]">
        {state.answer ? (
          <MarkdownMessage cwd={cwd} streaming={streaming}>
            {state.answer}
          </MarkdownMessage>
        ) : streaming ? (
          <p role="status" className="m-0 flex items-center gap-2 text-content-muted">
            <OrbitSpinner className="size-4" />
            {t('chat:sideQuestion.answering')}
          </p>
        ) : state.status === 'done' ? (
          <p className="m-0 text-content-muted">
            {state.toolCallIgnored
              ? t('chat:sideQuestion.toolIgnored')
              : t('chat:sideQuestion.empty')}
          </p>
        ) : null}
        {state.status === 'error' && (
          <p role="alert" className="m-0 mt-1 text-[var(--danger)]">
            {t('chat:sideQuestion.failed', { error: state.error })}
          </p>
        )}
      </div>
      <footer className="border-t border-stroke-soft px-3 py-1.5 text-xs text-content-muted">
        {t('chat:sideQuestion.note')}
      </footer>
    </section>
  )
}
