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
      className={`task-execution-mode-select relative h-7 min-w-0 max-[650px]:h-11 pointer-coarse:h-11 ${open ? 'open' : ''} ${selectedMode !== 'plan' ? 'active' : ''}`}
    >
      {/* 收进「+」托盘时与其他菜单行对齐：图标占同宽图标列（svg 加宽后图形居中），文字从同一列起。 */}
      <button
        ref={triggerRef}
        type="button"
        className="task-execution-mode-trigger [.task-execution-mode-select.open_&]:bg-surface-hover [.task-execution-mode-select.open_&]:text-content [.task-execution-mode-select.active_&]:border-star-border [.task-execution-mode-select.active_&]:bg-star-soft [.task-execution-mode-select.active_&]:text-star-strong inline-flex h-full w-full min-w-7 cursor-pointer items-center justify-center gap-1.5 rounded-sm border border-stroke bg-panel px-2 text-[13px] text-content-secondary transition-colors hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-[.55] max-[650px]:min-w-11 max-[650px]:rounded-md max-[650px]:border-transparent max-[650px]:bg-transparent max-[650px]:px-2.5 max-[650px]:text-sm [.composer-tool-tray_&]:justify-start [.composer-tool-tray_&]:gap-2 [.composer-tool-tray_&]:border-transparent [.composer-tool-tray_&]:pl-0 [.composer-tool-tray_&]:text-[13px] [.composer-tool-tray_&]:text-content [.composer-tool-tray_&>svg]:w-[30px] max-[650px]:[.composer-tool-tray_&>svg]:w-11 pointer-coarse:[.composer-tool-tray_&>svg]:w-11"
        title={label}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((visible) => !visible)}
      >
        <CurrentIcon className="shrink-0 opacity-80" size={15} />
        <span className="truncate @max-[440px]/composer-footer:hidden">{current.label}</span>
      </button>
      {open && (
        <AnchoredPopupMenu
          open={open}
          anchorRef={triggerRef}
          menuRef={menuRef}
          placement="top"
          className="anchored-popup-menu task-execution-mode-menu [&_strong]:text-[13px] [&_strong]:font-medium [&_small]:overflow-hidden [&_small]:text-xs [&_small]:font-normal [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_small]:text-content-muted [&_p]:mx-2 [&_p]:mt-px [&_p]:mb-1 [&_p]:text-xs [&_p]:text-content-muted [&_p]:tabular-nums w-[min(270px,calc(100vw_-_28px))] overflow-hidden rounded-md bg-solid p-1 text-content shadow-float"
          role="menu"
          ariaLabel={t('chat:focusSession.executionMode')}
          onClose={closeMenu}
        >
          <div className="px-2 pt-1.5 pb-1 text-xs font-medium text-content-muted">
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
                className={`task-execution-mode-option grid min-h-11 w-full cursor-pointer grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 rounded-sm px-2 py-1.5 text-left hover:bg-surface-hover [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-0.5 [&_>_svg]:text-accent-fg ${selected ? 'bg-surface-hover' : ''}`}
                key={option.value}
                onClick={() => {
                  onChange(option.value)
                  closeMenu()
                }}
              >
                <span className="grid size-7 place-items-center rounded-sm bg-surface-muted text-content-secondary">
                  <Icon size={14} />
                </span>
                <span>
                  <strong>{option.label}</strong>
                  <small>{option.description}</small>
                </span>
                {selected && <Check size={15} strokeWidth={2.25} />}
              </button>
            )
          })}
          {(selectedMode === 'goal' || selectedMode === 'team' || hasExistingGoal) && (
            <>
              {usage && <p>{usage}</p>}
              <div className="goal-mode-budget-row [&_label]:text-xs [&_label]:font-medium [&_label]:text-content-secondary [&_input]:h-8 [&_input]:w-full [&_input]:min-w-0 [&_input]:rounded-sm [&_input]:border [&_input]:border-control [&_input]:bg-panel [&_input]:px-2 [&_input]:text-[13px] [&_input]:text-inherit [&_input]:tabular-nums [&_input:focus]:border-focus [&_input:focus]:[outline:none] [&_input:focus]:ring-3 [&_input:focus]:ring-focus-ring mx-2 my-1 grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-1.5">
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
              <small className="mx-2 mb-1.5 block text-xs text-content-muted">
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
