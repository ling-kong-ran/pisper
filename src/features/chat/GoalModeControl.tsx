// Composer 执行模式控制：默认按 Plan 单轮推进，也可以切换到 Goal 或多智能体 Team。
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Check, ListTodo, RefreshCw, Target, Users } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { formatTokenCount } from '@/lib/format'
import type { EntityRecord } from '@/types/chat'
import { AnchoredPopupMenu } from './AnchoredPopupMenu'

import { Button } from '@/components/ui/button'

const MIN_GOAL_TOKEN_BUDGET = 1_000
export type ComposerExecutionMode = 'plan' | 'goal' | 'team'

type ExecutionModeOption = {
  value: ComposerExecutionMode
  label: string
  description: string
  icon: typeof ListTodo
}

export function ExecutionModeControl({
  mode,
  goal,
  tokenBudget,
  onTokenBudgetChange,
  onSaveTokenBudget,
  onChange,
  teamAvailable = false,
  disabled = false,
}: {
  mode: ComposerExecutionMode
  goal?: EntityRecord | null
  tokenBudget: number | null
  onTokenBudgetChange: (tokenBudget: number | null) => void
  onSaveTokenBudget?: (tokenBudget: number | null) => Promise<void> | void
  onChange: (mode: ComposerExecutionMode) => void
  teamAvailable?: boolean
  disabled?: boolean
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [savingBudget, setSavingBudget] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  const activeGoal = goal?.status === 'active'
  const hasExistingGoal = Boolean(goal?.id)
  const selectedMode: ComposerExecutionMode = activeGoal
    ? goal?.mode === 'team' && teamAvailable
      ? 'team'
      : 'goal'
    : mode
  const rawBudget = selectedMode === 'team' ? goal?.teamTokenBudget : goal?.tokenBudget
  const currentBudget = rawBudget == null ? null : Number(rawBudget) > 0 ? Number(rawBudget) : null
  const budgetDirty = tokenBudget !== currentBudget
  const options: ExecutionModeOption[] = [
    {
      value: 'plan',
      label: t('chat:focusSession.planMode'),
      description: t('chat:focusSession.planModeDescription'),
      icon: ListTodo,
    },
    {
      value: 'goal',
      label: t('chat:focusSession.goalMode'),
      description: t('chat:focusSession.goalModeDescription'),
      icon: Target,
    },
    ...(teamAvailable
      ? [
          {
            value: 'team' as const,
            label: t('chat:focusSession.teamMode'),
            description: t('chat:focusSession.teamModeDescription'),
            icon: Users,
          },
        ]
      : []),
  ]
  const current = options.find((option) => option.value === selectedMode) || options[0]
  const CurrentIcon = current.icon
  const status =
    selectedMode === 'plan'
      ? current.description
      : activeGoal
        ? selectedMode === 'team'
          ? t('chat:focusSession.runningAsTeam')
          : t('chat:focusSession.runningAutonomously')
        : goal?.status === 'paused'
          ? t('chat:focusSession.theNextMessageWillResumeTheGoal')
          : goal?.status === 'complete'
            ? t('chat:focusSession.goalComplete')
            : goal?.status === 'budget_limited'
              ? t('chat:focusSession.goalReachedItsBudget')
              : t('chat:focusSession.theNextMessageWillStartAGoal')
  const detail = selectedMode === 'plan' ? current.description : status
  const budgetPlaceholder = t('chat:focusSession.goalTokenBudgetUnlimited')
  const usage = hasExistingGoal
    ? currentBudget == null
      ? t('chat:focusSession.usedUnlimitedGoalTokens', {
          used: formatTokenCount(goal?.tokensUsed),
        })
      : t('chat:focusSession.usedBudgetTokensUsed', {
          used: goal?.tokensUsed || 0,
          budget: currentBudget || 0,
        })
    : ''
  const label = [t('chat:focusSession.executionMode'), current.label, detail, usage]
    .filter(Boolean)
    .join(' · ')
  const closeMenu = useCallback(() => {
    setOpen(false)
    triggerRef.current?.focus()
  }, [])

  useLayoutEffect(() => {
    if (open) menuRef.current?.querySelector<HTMLButtonElement>('[aria-checked="true"]')?.focus()
  }, [open])

  useEffect(() => {
    if (!open) return undefined
    const close = (event: MouseEvent) => {
      const target = event.target instanceof Node ? event.target : null
      // 菜单通过 portal 离开了按钮容器，内部选项和预算输入仍属于本控件。
      if (!rootRef.current?.contains(target) && !menuRef.current?.contains(target)) setOpen(false)
    }
    document.addEventListener('mousedown', close)
    return () => {
      document.removeEventListener('mousedown', close)
    }
  }, [open])

  // 保存已存在 Goal 的预算；空输入会保存为 null，表示不限制 Token。
  const saveBudget = async () => {
    if (savingBudget || !budgetDirty || !hasExistingGoal || !onSaveTokenBudget) return
    setSavingBudget(true)
    try {
      await onSaveTokenBudget(tokenBudget)
    } finally {
      setSavingBudget(false)
    }
  }

  return (
    <div
      ref={rootRef}
      className={`task-execution-mode-select relative h-10 min-w-0 text-muted-foreground ${open ? 'open' : ''} ${selectedMode !== 'plan' ? 'active' : ''}`}
    >
      <button
        ref={triggerRef}
        type="button"
        className="task-execution-mode-trigger hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)] hover:text-[var(--star-strong)] [.task-execution-mode-select.open_&]:border-[var(--accent-border)] [.task-execution-mode-select.open_&]:bg-[var(--accent-soft)] [.task-execution-mode-select.open_&]:text-[var(--star-strong)] [.task-execution-mode-select.active_&]:text-[var(--star-strong)] inline-flex w-full h-full items-center justify-center gap-1.5 px-1.5 rounded-lg border-0 bg-transparent text-[13px] text-inherit cursor-pointer disabled:cursor-not-allowed disabled:opacity-[.55]"
        title={label}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((visible) => !visible)}
      >
        <CurrentIcon className="shrink-0 max-[650px]:hidden" size={15} />
        <span className="truncate">{current.label}</span>
      </button>
      {open && (
        <AnchoredPopupMenu
          open={open}
          anchorRef={triggerRef}
          menuRef={menuRef}
          placement="top"
          className="anchored-popup-menu task-execution-mode-menu [&_strong]:text-[length:var(--app-font-size)] [&_strong]:font-medium [&_small]:overflow-hidden [&_small]:text-[var(--text-secondary)] [&_small]:text-[length:var(--app-small-size)] [&_small]:font-normal [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_p]:m-[1px_7px_4px_47px] [&_p]:text-[var(--text-secondary)] [&_p]:text-[length:var(--app-small-size)] w-[min(270px,calc(100vw_-_28px))] overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--solid)] [padding:5px] shadow-[0_18px_42px_-18px_var(--menu-shadow)]"
          role="menu"
          ariaLabel={t('chat:focusSession.executionMode')}
          onClose={closeMenu}
        >
          <div className="[padding:6px_8px_4px] text-[var(--text-secondary)] text-[length:var(--app-small-size)] font-semibold">
            {t('chat:focusSession.executionMode')}
          </div>
          {options.map((option) => {
            const Icon = option.icon
            const selected = option.value === selectedMode
            return (
              <button
                type="button"
                role="menuitemradio"
                aria-checked={selected}
                className={`task-execution-mode-option hover:bg-[var(--accent-soft)] [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[2px] grid w-full min-h-[48px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[8px] border-0 rounded-[var(--r-sm)] bg-transparent [padding:6px_7px] text-left ${selected ? 'bg-[var(--accent-soft)]' : ''}`}
                key={option.value}
                onClick={() => {
                  onChange(option.value)
                  closeMenu()
                }}
              >
                <span className="grid w-[32px] h-[32px] place-items-center rounded-[var(--r-sm)] bg-[var(--surface-muted)] text-[var(--text-muted)]">
                  <Icon size={14} />
                </span>
                <span>
                  <strong>{option.label}</strong>
                  <small>{option.description}</small>
                </span>
                {selected && <Check size={13} />}
              </button>
            )
          })}
          {(selectedMode === 'goal' || selectedMode === 'team' || hasExistingGoal) && (
            <>
              {usage && <p>{usage}</p>}
              <div className="goal-mode-budget-row [&_label]:text-[var(--text-secondary)] [&_label]:text-[length:var(--app-small-size)] [&_label]:font-medium [&_input]:w-full [&_input]:min-w-0 [&_input]:[border:1px_solid_var(--stroke)] [&_input]:rounded-[var(--r-sm)] [&_input]:bg-[var(--surface-muted)] [&_input]:p-[5px_7px] [&_input]:text-inherit [&_input]:text-[length:var(--app-font-size)] [&_input:focus]:border-[var(--focus)] [&_input:focus]:[outline:none] grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[6px] [margin:2px_7px_2px]">
                <label htmlFor="goal-token-budget-input">
                  {selectedMode === 'team'
                    ? t('chat:focusSession.teamTokenBudget')
                    : t('chat:focusSession.goalTokenBudget')}
                </label>
                <input
                  id="goal-token-budget-input"
                  type="number"
                  min={MIN_GOAL_TOKEN_BUDGET}
                  step={1000}
                  value={tokenBudget ?? ''}
                  placeholder={budgetPlaceholder}
                  aria-label={
                    selectedMode === 'team'
                      ? t('chat:focusSession.teamTokenBudget')
                      : t('chat:focusSession.goalTokenBudget')
                  }
                  disabled={savingBudget}
                  onChange={(event) => {
                    const raw = event.target.value.trim()
                    if (!raw) {
                      onTokenBudgetChange(null)
                      return
                    }
                    const next = Number(raw)
                    if (Number.isFinite(next))
                      onTokenBudgetChange(Math.max(MIN_GOAL_TOKEN_BUDGET, Math.round(next)))
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
              <small className="block [margin:0_7px_5px] text-[var(--text-secondary)] text-[length:var(--app-small-size)]">
                {selectedMode === 'team'
                  ? t('chat:focusSession.teamTokenBudgetHint')
                  : hasExistingGoal
                    ? t('chat:focusSession.goalTokenBudgetUpdateHint')
                    : t('chat:focusSession.goalTokenBudgetHint')}
              </small>
            </>
          )}
        </AnchoredPopupMenu>
      )}
    </div>
  )
}
