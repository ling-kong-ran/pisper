// 输入框左侧的盾牌芯片（规格 v2 第 2.5 / 4.3 节）：一个弹层里选审批模式与执行方式（Plan / Goal / Team）。
// 原来工具条上的审批芯片与“Plan”按钮合并到这里；执行方式不是默认的 Plan 时，芯片前面写出模式名，
// 例如“Goal · 审批后写入”。⇧Tab（输入框里）在可用的执行方式之间切换，见 nextComposerRunMode。
// 芯片只用中性色：琥珀只表示“需要你”，完全访问靠图标与文字区分，不再染警示色（规格 6.1）。
import { useState, type ReactNode } from 'react'
import {
  BadgeCheck,
  Check,
  ChevronDown,
  FileCheck2,
  ListTodo,
  RefreshCw,
  Shield,
  ShieldAlert,
  Target,
  Users,
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Kbd } from '@/components/ui/kbd'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { formatTokenCount } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { EntityRecord } from '@/types/chat'
import { type ComposerExecutionMode } from './composer-run-mode'

const MIN_GOAL_TOKEN_BUDGET = 1_000
type Option<T extends string> = { value: T; label: string; description: string; icon: LucideIcon }

function OptionRow<T extends string>({
  option,
  selected,
  disabled,
  onSelect,
}: {
  option: Option<T>
  selected: boolean
  disabled?: boolean
  onSelect: (value: T) => void
}) {
  const Icon = option.icon
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      disabled={disabled}
      className={cn(
        'grid min-h-11 w-full cursor-pointer grid-cols-[auto_minmax(0,1fr)_16px] items-center gap-2.5 rounded-sm px-2 py-1.5 text-left text-content hover:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus disabled:cursor-default disabled:opacity-50 disabled:hover:bg-transparent',
        selected && 'bg-surface-hover',
      )}
      data-mode-option={option.value}
      onClick={() => onSelect(option.value)}
    >
      <span className="grid size-7 place-items-center rounded-sm bg-surface-muted text-content-secondary">
        <Icon size={14} aria-hidden="true" />
      </span>
      <span className="flex min-w-0 flex-col gap-0.5">
        <strong className="text-[13px] font-medium">{option.label}</strong>
        <small className="text-xs leading-normal text-content-muted">{option.description}</small>
      </span>
      {selected ? <Check size={15} strokeWidth={2.25} aria-hidden="true" /> : <span />}
    </button>
  )
}

function Section({
  label,
  hint,
  children,
}: {
  label: string
  hint?: ReactNode
  children: ReactNode
}) {
  return (
    <div role="radiogroup" aria-label={label} className="flex flex-col gap-0.5">
      <div className="flex items-center justify-between px-2 pt-1.5 pb-1 text-xs font-medium text-content-muted">
        <span>{label}</span>
        {hint}
      </div>
      {children}
    </div>
  )
}

export function ComposerModeChip({
  approvalMode,
  onApprovalModeChange,
  approvalDisabled = false,
  runMode,
  onRunModeChange,
  runModeDisabled = false,
  goalsAvailable,
  teamAvailable,
  goal,
  tokenBudget,
  onTokenBudgetChange,
  onSaveTokenBudget,
}: {
  approvalMode: string
  onApprovalModeChange: (mode: string) => void
  approvalDisabled?: boolean
  runMode: ComposerExecutionMode
  onRunModeChange: (mode: ComposerExecutionMode) => void
  runModeDisabled?: boolean
  goalsAvailable: boolean
  teamAvailable: boolean
  goal?: EntityRecord | null
  tokenBudget: number | null
  onTokenBudgetChange: (tokenBudget: number | null) => void
  onSaveTokenBudget?: (tokenBudget: number | null) => Promise<void> | void
}) {
  const { t } = useI18n()
  const [savingBudget, setSavingBudget] = useState(false)
  const [open, setOpen] = useState(false)
  const approvalOptions: Option<string>[] = [
    {
      value: 'approval-required',
      label: t('chat:focusSession.approvalRequired'),
      description: t('chat:focusSession.approvalRequiredShowsADiffBeforeWriting'),
      icon: FileCheck2,
    },
    {
      value: 'workspace-write',
      label: t('chat:focusSession.workspaceWrite'),
      description: t('chat:focusSession.workspaceWriteRunsCommandsWithAutomaticApproval'),
      icon: BadgeCheck,
    },
    {
      value: 'full-access',
      label: t('chat:focusSession.fullAccess'),
      description: t('chat:focusSession.fullAccessRunsShellWithoutPerCommandApproval'),
      icon: ShieldAlert,
    },
  ]
  const runOptions: Option<ComposerExecutionMode>[] = [
    {
      value: 'plan',
      label: t('chat:focusSession.planMode'),
      description: t('chat:focusSession.planModeDescription'),
      icon: ListTodo,
    },
    ...(goalsAvailable
      ? [
          {
            value: 'goal' as const,
            label: t('chat:focusSession.goalMode'),
            description: t('chat:focusSession.goalModeDescription'),
            icon: Target,
          },
        ]
      : []),
    ...(goalsAvailable && teamAvailable
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
  const approval = approvalOptions.find((item) => item.value === approvalMode) || approvalOptions[0]
  // 进行中的目标决定当前方式（与原执行模式控件一致）：Goal 运行中芯片不能写成 Plan。
  const activeGoal = goal?.status === 'active'
  const selectedRun: ComposerExecutionMode = activeGoal
    ? goal?.mode === 'team' && teamAvailable
      ? 'team'
      : 'goal'
    : runMode
  const run = runOptions.find((item) => item.value === selectedRun) || runOptions[0]
  const engaged = run.value !== 'plan'
  const ChipIcon = approval.value === 'full-access' ? ShieldAlert : Shield
  const runShort =
    run.value === 'team'
      ? t('board:composer.teamShort')
      : run.value === 'goal'
        ? t('board:composer.goalShort')
        : ''
  const chipLabel = runShort ? `${runShort} · ${approval.label}` : approval.label
  const hasExistingGoal = Boolean(goal?.id)
  const rawBudget = selectedRun === 'team' ? goal?.teamTokenBudget : goal?.tokenBudget
  const currentBudget = rawBudget == null ? null : Number(rawBudget) > 0 ? Number(rawBudget) : null
  const budgetDirty = tokenBudget !== currentBudget
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
  const label = `${t('board:composer.modeChip')} · ${chipLabel}`
  const budgetLabel =
    selectedRun === 'team'
      ? t('chat:focusSession.teamTokenBudget')
      : t('chat:focusSession.goalTokenBudget')

  // 保存已存在 Goal 的预算；空输入保存为 null，表示不限制 Token。
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
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="composer-mode-chip inline-flex h-7 max-w-[220px] min-w-0 shrink cursor-pointer items-center gap-1.5 rounded-sm border border-transparent px-2 text-[13px] text-content-secondary transition-colors hover:bg-surface-hover hover:text-content aria-expanded:bg-surface-hover aria-expanded:text-content max-[650px]:h-11 max-[650px]:max-w-[160px] max-[650px]:border-stroke max-[650px]:px-2.5 pointer-coarse:h-11"
          aria-label={label}
          title={label}
          data-composer-mode={`${run.value}:${approval.value}`}
        >
          <ChipIcon size={14} className="shrink-0" aria-hidden="true" />
          <span className="min-w-0 truncate">{chipLabel}</span>
          <ChevronDown size={14} className="shrink-0 opacity-70" aria-hidden="true" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        side="top"
        className="composer-mode-popover w-[300px] max-w-[calc(100vw_-_24px)] gap-1 p-1"
      >
        <Section label={t('chat:focusSession.approvalMode')}>
          {approvalOptions.map((option) => (
            <OptionRow
              key={option.value}
              option={option}
              selected={option.value === approval.value}
              disabled={approvalDisabled}
              // 选定即关闭弹层（与原审批菜单一致）；完全访问的确认对话框由调用方弹出。
              onSelect={(mode) => {
                setOpen(false)
                onApprovalModeChange(mode)
              }}
            />
          ))}
        </Section>
        {runOptions.length > 1 && (
          <>
            <div role="separator" className="mx-1 h-px bg-stroke-soft" />
            <Section
              label={t('chat:focusSession.executionMode')}
              hint={
                <span className="flex items-center gap-1 font-normal">
                  {t('board:composer.cycleRunMode')}
                  <Kbd>⇧Tab</Kbd>
                </span>
              }
            >
              {runOptions.map((option) => (
                <OptionRow
                  key={option.value}
                  option={option}
                  selected={option.value === selectedRun}
                  disabled={runModeDisabled}
                  onSelect={(mode) => {
                    // 选 Goal / Team 时留在弹层里，方便接着填预算；回到 Plan 直接关闭。
                    if (mode === 'plan') setOpen(false)
                    onRunModeChange(mode)
                  }}
                />
              ))}
            </Section>
            {(engaged || hasExistingGoal) && (
              <div className="grid gap-1 px-2 pt-1 pb-1.5 text-xs">
                {usage && <p className="text-content-muted tabular-nums">{usage}</p>}
                <div className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-1.5">
                  <label
                    htmlFor="goal-token-budget-input"
                    className="font-medium text-content-secondary"
                  >
                    {budgetLabel}
                  </label>
                  <input
                    id="goal-token-budget-input"
                    type="number"
                    min={MIN_GOAL_TOKEN_BUDGET}
                    step={1000}
                    value={tokenBudget ?? ''}
                    placeholder={t('chat:focusSession.goalTokenBudgetUnlimited')}
                    aria-label={budgetLabel}
                    disabled={savingBudget}
                    className="h-8 w-full min-w-0 rounded-sm border border-control bg-panel px-2 text-[13px] text-inherit tabular-nums focus:border-focus focus:ring-3 focus:ring-focus-ring focus:outline-none"
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
                  {hasExistingGoal && budgetDirty ? (
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
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
                  ) : (
                    <span />
                  )}
                </div>
                <small className="text-content-muted">
                  {selectedRun === 'team'
                    ? t('chat:focusSession.teamTokenBudgetHint')
                    : hasExistingGoal
                      ? t('chat:focusSession.goalTokenBudgetUpdateHint')
                      : t('chat:focusSession.goalTokenBudgetHint')}
                </small>
              </div>
            )}
          </>
        )}
      </PopoverContent>
    </Popover>
  )
}
