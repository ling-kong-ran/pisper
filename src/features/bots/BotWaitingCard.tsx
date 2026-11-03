// 任务干到一半在等你：审批一个操作，或回答一个问题。直接在对话里处理，不用打开会话，也不弹窗。
// 安全规则与会话页一致：高风险（critical）要在卡片里再确认一次才允许；参数被截断时这里看不全，
// 只能拒绝，允许要去完整记录里看完整参数。多项问题与多选题也去完整记录里答。
import { useState, type FormEvent } from 'react'
import { MessageCircleQuestion, ShieldAlert } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  answerBotQuestion,
  resolveBotApproval,
  type Bot,
  type BotWaitingApproval,
  type BotWaitingQuestion,
} from './bots-api'

type Props = {
  sessionId: string
  onDone: () => void
  onError: (message: string) => void
  openSession: React.ReactNode
}

function useSubmit({ onDone, onError }: Pick<Props, 'onDone' | 'onError'>) {
  const [busy, setBusy] = useState(false)
  const run = async (action: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await action()
      onDone()
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }
  return { busy, run }
}

export function BotApprovalCard({
  approval,
  sessionId,
  onDone,
  onError,
  openSession,
}: Props & { approval: BotWaitingApproval }) {
  const { t } = useI18n()
  const { busy, run } = useSubmit({ onDone, onError })
  const [confirming, setConfirming] = useState(false)
  const critical = approval.severity === 'critical'
  const resolve = (approved: boolean) =>
    run(() => resolveBotApproval(sessionId, approval.id, approved))
  return (
    <div
      data-bot-approval={approval.id}
      data-severity={approval.severity}
      className="mt-2 overflow-hidden rounded-xl border border-st-attn-border bg-surface-raised ring-[3px] ring-st-attn-soft"
    >
      <div className="flex items-center gap-2 px-3.5 py-2.5 text-[13px]">
        <ShieldAlert className="size-3.5 text-st-attn" aria-hidden="true" />
        <span className="min-w-0 flex-1 font-semibold text-content">
          {t('bots:waiting.approval', { tool: approval.toolName })}
        </span>
      </div>
      <pre className="m-0 max-h-48 overflow-auto border-t border-stroke bg-surface-subtle px-3.5 py-2.5 font-mono text-xs whitespace-pre-wrap text-content-secondary">
        {approval.argsPreview}
      </pre>
      {(approval.reason || critical || approval.argsTruncated) && (
        <div className="flex flex-col gap-1 border-t border-stroke px-3.5 py-2 text-xs text-content-muted">
          {approval.reason && <p>{approval.reason}</p>}
          {critical && <p className="text-st-attn">{t('bots:waiting.critical')}</p>}
          {approval.argsTruncated && <p>{t('bots:waiting.truncated')}</p>}
        </div>
      )}
      <div className="flex flex-wrap items-center justify-end gap-2 border-t border-stroke px-3.5 py-2.5">
        {openSession}
        {confirming ? (
          <>
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => setConfirming(false)}
            >
              {t('bots:waiting.cancel')}
            </Button>
            <Button
              size="sm"
              variant="destructive"
              disabled={busy}
              onClick={() => void resolve(true)}
            >
              {t('bots:waiting.confirmAllow')}
            </Button>
          </>
        ) : (
          <>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => void resolve(false)}>
              {t('bots:waiting.deny')}
            </Button>
            {!approval.argsTruncated && (
              <Button
                size="sm"
                disabled={busy}
                onClick={() => (critical ? setConfirming(true) : void resolve(true))}
              >
                {t('bots:waiting.allow')}
              </Button>
            )}
          </>
        )}
      </div>
    </div>
  )
}

export function BotQuestionCard({
  bot,
  question,
  sessionId,
  onDone,
  onError,
  openSession,
}: Props & { bot: Bot; question: BotWaitingQuestion }) {
  const { t } = useI18n()
  const { busy, run } = useSubmit({ onDone, onError })
  const [other, setOther] = useState('')
  // 单题单选在这里答；多项或多选去完整记录，避免在卡片里再造一套答题器。
  const simple = question.count === 1 && !question.multiSelect
  const answer = (selected: string[], text = '') =>
    run(() =>
      answerBotQuestion(sessionId, question.id, {
        selected,
        ...(text.trim() ? { other: text.trim() } : {}),
      }),
    )
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (other.trim()) void answer([], other)
  }
  return (
    <div
      data-bot-question={question.id}
      className="mt-2 overflow-hidden rounded-xl border border-st-attn-border bg-surface-raised ring-[3px] ring-st-attn-soft"
    >
      <div className="flex items-center gap-2 px-3.5 pt-2.5 text-xs text-content-muted">
        <MessageCircleQuestion className="size-3.5 text-st-attn" aria-hidden="true" />
        <span>{question.header || t('bots:waiting.question', { name: bot.name })}</span>
      </div>
      <p className="px-3.5 pt-1 pb-2.5 text-[14px] font-medium whitespace-pre-wrap text-content">
        {question.prompt}
      </p>
      {simple ? (
        <div className="flex flex-col gap-2 border-t border-stroke px-3.5 py-2.5">
          {question.options.length > 0 && (
            <div className="flex flex-wrap gap-2">
              {question.options.map((option) => (
                <Button
                  key={option}
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => void answer([option])}
                >
                  {option}
                </Button>
              ))}
            </div>
          )}
          <form className="flex items-center gap-2" onSubmit={submit}>
            <Input
              value={other}
              maxLength={2000}
              aria-label={t('bots:waiting.otherLabel')}
              placeholder={t('bots:waiting.otherPlaceholder')}
              disabled={busy}
              onChange={(event) => setOther(event.target.value)}
            />
            <Button type="submit" size="sm" disabled={busy || !other.trim()}>
              {t('bots:waiting.answer')}
            </Button>
          </form>
          <div className="flex justify-end gap-2">
            {openSession}
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() =>
                void run(() => answerBotQuestion(sessionId, question.id, { skip: true }))
              }
            >
              {t('bots:waiting.skip')}
            </Button>
          </div>
        </div>
      ) : (
        <div className="flex flex-wrap items-center gap-2 border-t border-stroke px-3.5 py-2.5 text-xs text-content-muted">
          <span className="flex-1">{t('bots:waiting.answerInLog')}</span>
          {openSession}
        </div>
      )}
    </div>
  )
}
