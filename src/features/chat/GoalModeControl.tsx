// 目标模式控制：执行模式（交互/自动）切换与当前模式指示。
import { useEffect, useRef, useState } from 'react'
import { Check, RefreshCw, Target } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSwitch as Toggle } from '@/components/ui/app-primitives'
import { formatTokenCount } from '@/lib/format'
import type { EntityRecord } from '@/types/chat'
import { useViewportMenuOffset } from './use-viewport-menu-offset'

import { Button } from '@/components/ui/button'

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
  const menuRef = useRef<HTMLDivElement>(null)
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

  useViewportMenuOffset(open, menuRef)

  // 保存 token 预算：有脏改且已存在目标模式时回调保存，防重入。
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
      className={`goal-mode-select [.composer-tool-tray_&]:w-[38px] [.composer-tool-tray_&]:min-w-[38px] [.composer-tool-tray_&]:h-[38px] [.composer-tool-tray_&]:flex-none @max-[700px]:[.composer-tool-tray_&]:w-[32px] @max-[700px]:[.composer-tool-tray_&]:min-w-[32px] @max-[700px]:[.composer-tool-tray_&]:h-[32px] @max-[700px]:[.composer-tool-tray_&]:p-0 @max-[470px]:[.composer-tool-tray_&]:w-[28px] @max-[470px]:[.composer-tool-tray_&]:min-w-[28px] @max-[470px]:[.composer-tool-tray_&]:h-[28px] relative w-[38px] h-[38px] text-[var(--text-tertiary)] ${open ? 'open' : ''}    ${active || armed ? 'active' : ''}`}
    >
      <button
        type="button"
        className="goal-mode-trigger hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)] hover:text-[var(--star-strong)] [.goal-mode-select.open_&]:border-[var(--accent-border)] [.goal-mode-select.open_&]:bg-[var(--accent-soft)] [.goal-mode-select.open_&]:text-[var(--star-strong)] [.goal-mode-select.active_&]:text-[var(--star-strong)] @max-[700px]:[.composer-tool-tray_&]:w-[32px] @max-[700px]:[.composer-tool-tray_&]:h-[32px] @max-[470px]:[.composer-tool-tray_&]:w-[28px] @max-[470px]:[.composer-tool-tray_&]:h-[28px] grid w-full h-full place-items-center [border:1px_solid_transparent] rounded-[var(--r-sm)] bg-[var(--surface-muted)] text-inherit cursor-pointer"
        title={label}
        aria-label={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((visible) => !visible)}
      >
        <Target size={14} />
      </button>
      {open && (
        <div
          ref={menuRef}
          className="goal-mode-menu [translate:var(--menu-x-offset,_0px)_0] [&_strong]:text-[12px] [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_p]:m-[1px_7px_4px_47px] [&_p]:text-[var(--text-muted)] [&_p]:text-[11px] max-[650px]:[.focus-composer_&]:right-[auto] max-[650px]:[.focus-composer_&]:left-0 max-[650px]:[.focus-composer_&]:w-[min(250px,calc(100vw_-_76px))] absolute z-[35] [bottom:calc(100%_+_8px)] left-0 w-[min(250px,calc(100vw_-_28px))] overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--solid)] [padding:5px] shadow-[0_18px_42px_-18px_var(--menu-shadow)]"
          role="dialog"
          aria-label={t('chat:focusSession.goalMode')}
        >
          <div className="goal-mode-menu-row hover:bg-[var(--accent-soft)] [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[2px] grid min-h-[48px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[8px] rounded-[var(--r-sm)] [padding:6px_7px]">
            <span className="goal-mode-menu-icon [.goal-mode-select.active_&]:bg-[var(--star-soft)] [.goal-mode-select.active_&]:text-[var(--star-strong)] grid w-[32px] h-[32px] place-items-center rounded-[var(--r-sm)] bg-[var(--surface-muted)] text-[var(--text-muted)]">
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
          <div className="goal-mode-budget-row [&_label]:text-[var(--text-secondary)] [&_label]:text-[11px] [&_label]:font-[600] [&_input]:w-full [&_input]:min-w-0 [&_input]:[border:1px_solid_var(--stroke)] [&_input]:rounded-[var(--r-sm)] [&_input]:bg-[var(--surface-muted)] [&_input]:p-[5px_7px] [&_input]:text-inherit [&_input]:text-[12px] [&_input:focus]:border-[var(--focus)] [&_input:focus]:[outline:none] grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[6px] [margin:2px_7px_2px]">
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
              <Button
                type="button"
                variant="outline"
                className="bg-surface-subtle"
                disabled={savingBudget}
                onClick={() => void saveBudget()}
              >
                {savingBudget ? (
                  <RefreshCw className="animate-spin" size={12} />
                ) : (
                  <Check size={12} />
                )}
                {t('chat:focusSession.goalBudgetSave')}
              </Button>
            )}
          </div>
          <small className="block [margin:0_7px_5px] text-[var(--text-muted)] text-[10px]">
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
