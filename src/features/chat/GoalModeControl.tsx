import { useEffect, useRef, useState } from 'react'
import { Check, RefreshCw, Target } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSwitch as Toggle } from '@/components/ui/app-primitives'
import { formatTokenCount } from '@/lib/format'
import type { EntityRecord } from '@/types/chat'

const MIN_GOAL_TOKEN_BUDGET = 1_000
export const DEFAULT_GOAL_TOKEN_BUDGET = 30_000

export function GoalModeControl({
  goal,
  armed,
  tokenBudget,
  onTokenBudgetChange,
  onSaveTokenBudget,
  onChange,
}: {
  goal?: EntityRecord | null
  armed: boolean
  tokenBudget: number
  onTokenBudgetChange: (tokenBudget: number) => void
  onSaveTokenBudget?: (tokenBudget: number) => Promise<void> | void
  onChange: (enabled: boolean) => void
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [savingBudget, setSavingBudget] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const active = goal?.status === 'active'
  const enabled = active || armed
  const hasExistingGoal = Boolean(goal?.id)
  const currentBudget = Number(goal?.tokenBudget) || DEFAULT_GOAL_TOKEN_BUDGET
  const budgetDirty = tokenBudget !== currentBudget
  const status = active
    ? t('chat:focusSession.runningAutonomously')
    : armed
      ? goal?.status === 'paused'
        ? t('chat:focusSession.theNextMessageWillResumeTheGoal')
        : t('chat:focusSession.theNextMessageWillStartAGoal')
      : goal?.status === 'complete'
        ? t('chat:focusSession.goalComplete')
        : goal?.status === 'budget_limited'
          ? t('chat:focusSession.goalReachedItsBudget')
          : goal?.status === 'paused'
            ? t('chat:focusSession.goalPaused')
            : t('chat:focusSession.enableForTheNextMessageOnly')
  const objective = String(goal?.objective || '')
    .replace(/\s+/g, ' ')
    .trim()
  const detail = armed ? status : objective || status
  const usage = hasExistingGoal
    ? t('chat:focusSession.usedBudgetTokensUsed', {
        used: goal?.tokensUsed || 0,
        budget: goal?.tokenBudget || 0,
      })
    : ''
  const label = [t('chat:focusSession.goalMode'), detail, usage].filter(Boolean).join(' · ')

  useEffect(() => {
    if (!open) return undefined
    const close = (event: MouseEvent) => {
      const target = event.target instanceof Node ? event.target : null
      if (!rootRef.current?.contains(target)) setOpen(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('keydown', escape)
    }
  }, [open])

  const saveBudget = async () => {
    if (savingBudget || !budgetDirty) return
    if (hasExistingGoal && onSaveTokenBudget) {
      setSavingBudget(true)
      try {
        await onSaveTokenBudget(tokenBudget)
      } finally {
        setSavingBudget(false)
      }
    }
  }

  return (
    <div
      ref={rootRef}
      className={`goal-mode-select ${open ? 'open' : ''} ${active || armed ? 'active' : ''}`}
    >
      <button
        type="button"
        className="goal-mode-trigger"
        title={label}
        aria-label={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((visible) => !visible)}
      >
        <Target size={14} />
      </button>
      {open && (
        <div className="goal-mode-menu" role="dialog" aria-label={t('chat:focusSession.goalMode')}>
          <div className="goal-mode-menu-row">
            <span className="goal-mode-menu-icon">
              <Target size={15} />
            </span>
            <span>
              <strong>{t('chat:focusSession.goalMode')}</strong>
              <small title={detail}>{detail}</small>
            </span>
            <Toggle
              value={enabled}
              onChange={(next) => {
                onChange(next)
                setOpen(false)
              }}
              ariaLabel={label}
              title={label}
            />
          </div>
          {usage && <p>{usage}</p>}
          <div className="goal-mode-budget-row">
            <label htmlFor="goal-token-budget-input">
              {t('chat:focusSession.goalTokenBudget')}
            </label>
            <input
              id="goal-token-budget-input"
              type="number"
              min={MIN_GOAL_TOKEN_BUDGET}
              step={1000}
              value={tokenBudget}
              disabled={savingBudget}
              onChange={(event) => {
                const next = Number(event.target.value)
                if (Number.isFinite(next)) {
                  onTokenBudgetChange(Math.max(MIN_GOAL_TOKEN_BUDGET, Math.round(next)))
                }
              }}
            />
            {hasExistingGoal && budgetDirty && (
              <button
                type="button"
                className="button secondary tiny"
                disabled={savingBudget}
                onClick={() => void saveBudget()}
              >
                {savingBudget ? <RefreshCw className="spin" size={12} /> : <Check size={12} />}
                {t('chat:focusSession.goalBudgetSave')}
              </button>
            )}
          </div>
          <small className="goal-mode-budget-hint">
            {hasExistingGoal
              ? t('chat:focusSession.goalTokenBudgetUpdateHint')
              : t('chat:focusSession.goalTokenBudgetHint', {
                  min: formatTokenCount(MIN_GOAL_TOKEN_BUDGET),
                })}
          </small>
        </div>
      )}
    </div>
  )
}
