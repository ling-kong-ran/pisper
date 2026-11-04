// 时间线里的一段 Bot 之间的来往：它找了另一个 Bot（“测试 · 在等回话 — 帮我跑一遍回归”），对方回了就合上；
// 来回太多次或发得太频繁时停下来，两边都写着“停在这里了”。对方说的话本身在对话里，这里不重复。
import { lazy, Suspense } from 'react'
import { Check, Hourglass, OctagonPause } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { SessionBotLink } from './session-agents'

const BotSessionChip = lazy(() => import('@/features/bots/public-session'))

export function BotLinkEvent({ link }: { link: SessionBotLink }) {
  const { t } = useI18n()
  const status =
    link.status === 'stopped'
      ? link.reason === 'rate'
        ? t('chat:botLink.stoppedRate')
        : t('chat:botLink.stoppedHops')
      : link.status === 'answered'
        ? t('chat:botLink.answered')
        : t('chat:botLink.waiting')
  return (
    <div
      data-bot-link={link.id}
      data-bot-link-status={link.status}
      className="mx-auto mb-[var(--chat-message-gap,32px)] flex min-h-10 w-full max-w-[var(--chat-content-width,1040px)] min-w-0 items-center gap-2 rounded-lg border border-stroke-soft bg-surface-subtle px-3 py-1.5 text-[13px]"
    >
      <span aria-hidden="true" className="grid size-4 flex-none place-items-center">
        {link.status === 'waiting' ? (
          <Hourglass size={14} className="text-content-muted" />
        ) : link.status === 'answered' ? (
          <Check size={14} className="text-st-done" />
        ) : (
          <OctagonPause size={14} className="text-content-muted" />
        )}
      </span>
      <span aria-hidden="true" className="flex flex-none items-center">
        <Suspense fallback={null}>
          <BotSessionChip botId={link.botId} part="mark" animate={false} />
        </Suspense>
      </span>
      <span className="min-w-0 flex-1 truncate">
        <span className="font-medium text-content">
          <Suspense fallback={null}>
            <BotSessionChip botId={link.botId} part="name" />
          </Suspense>
        </span>
        <span className="text-content-muted">
          {' · '}
          {status}
          {link.direction === 'out' && link.text ? ` — ${link.text}` : ''}
        </span>
      </span>
    </div>
  )
}
