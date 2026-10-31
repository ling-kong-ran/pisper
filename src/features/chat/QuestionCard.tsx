// 代理的提问卡（ask_user，规格 v2 第 8.4、4.2 节）：放在审批卡同一位置（输入框上方、与输入框同宽），琥珀描边 + 浅琥珀底。
// 选项是可以直接点的按钮：单个问题且单选时点一下即提交；多个问题或多选时逐个点选，全部问题都有答案后“提交”才可用；
// 每个问题都可以点“其他…”写文字（≤2000）。焦点在卡片里时数字键 1–6 选当前问题（第一个还没答的）的选项，↵ 提交。
// 有审批同时在等时，审批卡先处理（一屏只突出一张），这里只留一行“还有 N 个问题等你回答”。
// 回答失败（网络、已在别处回答）时把提示写在卡片里，答案留着可以再提交。
import { useState, type KeyboardEvent } from 'react'
import { Check, MessageCircleQuestion } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import {
  answersComplete,
  QUESTION_UI_LIMITS,
  type PendingQuestion,
  type QuestionAnswer,
} from './session-questions'

type QuestionCardProps = {
  question: PendingQuestion
  // 还在排队的提问数（含这一个之外的）。
  more: number
  // 审批在前：只显示一行提示。
  collapsed: boolean
  onAnswer: (
    questionId: string,
    body: { answers: QuestionAnswer[] } | { skip: true },
  ) => Promise<void>
}

export function QuestionCard({ question, more, collapsed, onAnswer }: QuestionCardProps) {
  const { t } = useI18n()
  const [answers, setAnswers] = useState<QuestionAnswer[]>(() =>
    question.questions.map(() => ({ selected: [] })),
  )
  const [otherOpen, setOtherOpen] = useState<boolean[]>(() => question.questions.map(() => false))
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const single = question.questions.length === 1 && !question.questions[0].multiSelect
  const complete = answersComplete(question, answers)

  if (collapsed)
    return (
      <p
        data-question-more
        className="flex min-h-8 min-w-0 items-center gap-2 rounded-(--r-md) border border-st-attn-border px-3 text-xs text-st-attn"
      >
        <span aria-hidden="true" className="size-2 flex-none rounded-full bg-st-attn-fill" />
        {t('board:question.waiting', { count: more + 1 })}
      </p>
    )

  const submit = async (body: { answers: QuestionAnswer[] } | { skip: true }) => {
    if (pending) return
    setPending(true)
    setError('')
    try {
      await onAnswer(question.id, body)
    } catch {
      setError(t('board:question.failed'))
      setPending(false)
    }
  }
  const finalAnswers = (list: QuestionAnswer[]) =>
    list.map((answer, index) => {
      const other = otherOpen[index] ? answer.other?.trim() : ''
      return other ? { selected: answer.selected, other } : { selected: answer.selected }
    })

  const choose = (index: number, label: string) => {
    const item = question.questions[index]
    const next = answers.map((answer, position) => {
      if (position !== index) return answer
      if (!item.multiSelect) return { ...answer, selected: [label] }
      const selected = answer.selected.includes(label)
        ? answer.selected.filter((entry) => entry !== label)
        : [...answer.selected, label]
      return { ...answer, selected }
    })
    setAnswers(next)
    // 一题单选：点一下即提交。
    if (single) void submit({ answers: finalAnswers(next) })
  }

  // 数字键选“当前问题”（第一个还没有答案的；都答了时是最后一个）的选项，↵ 提交。
  const keyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.altKey || event.metaKey || event.ctrlKey) return
    const target = event.target as HTMLElement
    if (target.closest('textarea, input')) return
    const current = answers.findIndex((answer) => !answer.selected.length && !answer.other?.trim())
    const index = current < 0 ? question.questions.length - 1 : current
    const digit = Number(event.key)
    if (Number.isInteger(digit) && digit >= 1 && digit <= 6) {
      const option = question.questions[index]?.options[digit - 1]
      if (!option) return
      event.preventDefault()
      choose(index, option.label)
    } else if (event.key === 'Enter' && !target.closest('button') && complete) {
      event.preventDefault()
      void submit({ answers: finalAnswers(answers) })
    }
  }

  return (
    <div
      data-question-card={question.id}
      role="group"
      aria-label={t('board:question.label')}
      aria-busy={pending || undefined}
      onKeyDown={keyDown}
      className="flex max-h-[min(60vh,560px)] min-w-0 flex-col gap-3 overflow-y-auto rounded-(--r-lg) border border-st-attn-border bg-st-attn-soft px-3.5 py-3 text-foreground"
    >
      <p className="flex items-center gap-2 text-xs font-medium text-warning-strong">
        <MessageCircleQuestion className="size-4 flex-none" aria-hidden="true" />
        {t('board:question.title')}
        {more > 0 && (
          <span className="font-normal text-content-muted">
            {t('board:question.moreAfter', { count: more })}
          </span>
        )}
      </p>
      {question.questions.map((item, index) => {
        const answer = answers[index]
        const groupLabel = item.header ? `${item.header}：${item.question}` : item.question
        return (
          <fieldset
            key={`${question.id}-${index}`}
            data-question-item={index}
            className="flex min-w-0 flex-col gap-1.5"
          >
            <legend className="mb-1.5 flex min-w-0 flex-col gap-0.5">
              {item.header && (
                <span className="text-xs font-medium text-content-muted">{item.header}</span>
              )}
              <span className="text-sm leading-5 text-content">{item.question}</span>
            </legend>
            <div
              role={item.multiSelect ? 'group' : 'radiogroup'}
              aria-label={groupLabel}
              className="flex flex-wrap gap-1.5"
            >
              {item.options.map((option, optionIndex) => {
                const selected = answer.selected.includes(option.label)
                return (
                  <button
                    key={option.label}
                    type="button"
                    role={item.multiSelect ? 'checkbox' : 'radio'}
                    aria-checked={selected}
                    disabled={pending}
                    data-question-option={option.label}
                    title={option.description || undefined}
                    className={cn(
                      'flex min-h-9 max-w-full min-w-0 items-center gap-1.5 rounded-(--r-md) border px-2.5 py-1 text-left text-[13px] transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus disabled:opacity-60 pointer-coarse:min-h-11',
                      selected
                        ? 'border-content bg-content text-panel'
                        : 'border-stroke-strong bg-panel text-content hover:bg-surface-hover',
                    )}
                    onClick={() => choose(index, option.label)}
                  >
                    <span
                      aria-hidden="true"
                      className="font-mono text-[11px] opacity-60 pointer-coarse:hidden"
                    >
                      {optionIndex + 1}
                    </span>
                    <span className="flex min-w-0 flex-col">
                      <span className="break-words">{option.label}</span>
                      {option.description && (
                        <span
                          className={cn(
                            'text-xs break-words',
                            selected ? 'opacity-80' : 'text-content-muted',
                          )}
                        >
                          {option.description}
                        </span>
                      )}
                    </span>
                    {selected && item.multiSelect && (
                      <Check className="size-3.5 flex-none" aria-hidden="true" />
                    )}
                  </button>
                )
              })}
              <button
                type="button"
                aria-expanded={otherOpen[index]}
                disabled={pending}
                data-question-other={index}
                className={cn(
                  'flex min-h-9 items-center rounded-(--r-md) border border-dashed border-stroke-strong px-2.5 text-[13px] text-content-secondary hover:bg-surface-hover focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus pointer-coarse:min-h-11',
                  otherOpen[index] && 'border-solid bg-panel',
                )}
                onClick={() =>
                  setOtherOpen((open) =>
                    open.map((value, position) => (position === index ? !value : value)),
                  )
                }
              >
                {t('board:question.other')}
              </button>
            </div>
            {otherOpen[index] && (
              <textarea
                aria-label={t('board:question.otherLabel', { question: item.question })}
                maxLength={QUESTION_UI_LIMITS.other}
                rows={2}
                disabled={pending}
                value={answer.other ?? ''}
                placeholder={t('board:question.otherPlaceholder')}
                className="min-h-16 w-full resize-y rounded-(--r-md) border border-control bg-panel px-2.5 py-1.5 text-sm text-content outline-none focus:border-focus focus:ring-3 focus:ring-focus-ring max-[650px]:text-[16px]"
                onChange={(event) => {
                  const value = event.target.value
                  setAnswers((list) =>
                    list.map((entry, position) =>
                      position === index ? { ...entry, other: value } : entry,
                    ),
                  )
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && (event.metaKey || event.ctrlKey) && complete) {
                    event.preventDefault()
                    void submit({ answers: finalAnswers(answers) })
                  }
                }}
              />
            )}
          </fieldset>
        )
      })}
      {error && (
        <p role="alert" className="text-xs text-[var(--danger)]">
          {error}
        </p>
      )}
      <div className="flex flex-wrap items-center justify-end gap-2">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={pending}
          data-question-skip
          className="pointer-coarse:min-h-11"
          onClick={() => void submit({ skip: true })}
        >
          {t('board:question.skip')}
        </Button>
        {(!single || otherOpen[0]) && (
          <Button
            type="button"
            size="sm"
            disabled={pending || !complete}
            loading={pending}
            data-question-submit
            className="pointer-coarse:min-h-11"
            onClick={() => void submit({ answers: finalAnswers(answers) })}
          >
            {t('board:question.submit')}
          </Button>
        )}
      </div>
    </div>
  )
}
