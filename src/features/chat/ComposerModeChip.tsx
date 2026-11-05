// 输入框左侧的盾牌芯片（规格 v2 第 2.5 / 4.3 节）：一个弹层里选审批模式（单选行，各带一行说明）
// 与执行方式（Plan / Goal / Team 分段，下面写当前方式的说明）。
// 原来工具条上的审批芯片与“Plan”按钮合并到这里；执行方式不是默认的 Plan 时，芯片前面写出模式名，
// 例如“Goal · 审批后写入”。⇧Tab（输入框里）在可用的执行方式之间切换，见 nextComposerRunMode。
// 芯片只用中性色：琥珀只表示“需要你”，完全访问靠图标与文字区分，不再染警示色（规格 6.1）。
import { useState } from 'react'
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
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Kbd } from '@/components/ui/kbd'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { PopoverSheet } from '@/components/app/PopoverSheet'
import { formatTokenCount } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { EntityRecord } from '@/types/chat'
import { type ComposerExecutionMode } from './composer-run-mode'
import { COMPOSER_CHIP_CLASSES } from './focus-session-composer-bits'

const MIN_GOAL_TOKEN_BUDGET = 1_000
type Option<T extends string> = {
  value: T
  label: string
  description: string
  icon: LucideIcon
}

function ApprovalRow({
  option,
  selected,
  disabled,
  onSelect,
}: {
  option: Option<string>
  selected: boolean
  disabled?: boolean
  onSelect: (value: string) => void
}) {
  const Icon = option.icon
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      disabled={disabled}
      className="grid min-h-12 w-full cursor-pointer grid-cols-[16px_minmax(0,1fr)_16px] items-center gap-2.5 rounded-sm px-2 py-1.5 text-left text-content hover:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus disabled:cursor-default disabled:opacity-50 disabled:hover:bg-transparent max-[650px]:min-h-14"
      data-mode-option={option.value}
      onClick={() => onSelect(option.value)}
    >
      <Icon size={16} strokeWidth={1.75} className="text-content-secondary" aria-hidden="true" />
      <span className="flex min-w-0 flex-col gap-px">
        <span className={cn('text-[13px] max-[650px]:text-[15px]', selected && 'font-medium')}>
          {option.label}
        </span>
        <small className="text-xs leading-normal text-content-muted max-[650px]:text-[13px]">
          {option.description}
        </small>
      </span>
      {selected ? <Check size={15} strokeWidth={2.25} aria-hidden="true" /> : <span />}
    </button>
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
  sheet = false,
  collapse = false,
  open: controlledOpen,
  onOpenChange,
}: {
  // 手机会话页：弹层换成底部表单（规格 7.2）。
  sheet?: boolean
  // 手机工具行只留 [+] [模型] … [麦克风] [发送]：默认的“Plan · 审批后写入 / 自动”不占位置，
  // 由 + 菜单打开同一个表单；完全访问或 Goal / Team 时芯片照常露出，免得看不见当前的风险与方式。
  collapse?: boolean
  open?: boolean
  onOpenChange?: (open: boolean) => void
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
  const [ownOpen, setOwnOpen] = useState(false)
  const open = controlledOpen ?? ownOpen
  const setOpen = (next: boolean) => {
    if (controlledOpen === undefined) setOwnOpen(next)
    onOpenChange?.(next)
  }
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
      label: t('board:composer.planShort'),
      description: t('chat:focusSession.planModeDescription'),
      icon: ListTodo,
    },
    ...(goalsAvailable
      ? [
          {
            value: 'goal' as const,
            label: t('board:composer.goalShort'),
            description: t('chat:focusSession.goalModeDescription'),
            icon: Target,
          },
        ]
      : []),
    ...(goalsAvailable && teamAvailable
      ? [
          {
            value: 'team' as const,
            label: t('board:composer.teamShort'),
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
  // 默认（Plan · 审批后写入）时芯片只留盾牌：大多数时候不用看它；换了模式才写出文字。
  // 悬停与读屏照样是完整名字。
  const quiet = run.value === 'plan' && approval.value === 'approval-required'
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
    <PopoverSheet
      sheet={sheet}
      title={t('board:composer.modeChip')}
      open={open}
      onOpenChange={setOpen}
      align="start"
      side="top"
      className="composer-mode-popover w-[300px] max-w-[calc(100vw_-_24px)] gap-1 p-1"
      trigger={
        <button
          type="button"
          className={cn(
            COMPOSER_CHIP_CLASSES,
            // 手机上它只在完全访问或 Goal / Team 时露出，而且只画图标（完全访问是带感叹号的盾牌，
            // Goal / Team 前面再加方式图标）：文字在无障碍标签、悬停提示和点开的底部表单里。
            'composer-mode-chip max-w-[220px] max-[650px]:min-w-9 max-[650px]:justify-center max-[650px]:gap-1',
            engaged ? 'max-[650px]:px-2.5' : 'max-[650px]:px-0',
          )}
          aria-label={label}
          title={label}
          hidden={collapse && !engaged && approval.value !== 'full-access'}
          data-composer-mode={`${run.value}:${approval.value}`}
        >
          {engaged && (
            <run.icon size={16} className="hidden shrink-0 max-[650px]:block" aria-hidden="true" />
          )}
          <ChipIcon size={14} className="shrink-0 max-[650px]:size-4" aria-hidden="true" />
          {!quiet && <span className="min-w-0 truncate max-[650px]:hidden">{chipLabel}</span>}
          {/* 手机上胶囊本身就是按钮（点开底部表单），不再画下拉箭头。 */}
          <ChevronDown
            size={14}
            className="shrink-0 opacity-70 max-[650px]:hidden"
            aria-hidden="true"
          />
        </button>
      }
    >
      <div
        role="radiogroup"
        aria-label={t('chat:focusSession.approvalMode')}
        className="flex flex-col gap-0.5"
      >
        <div className="px-2 pt-1.5 pb-1 text-xs font-medium text-content-muted">
          {t('chat:focusSession.approvalMode')}
        </div>
        {approvalOptions.map((option) => (
          <ApprovalRow
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
      </div>
      {runOptions.length > 1 && (
        <>
          <div role="separator" className="mx-1 h-px bg-stroke-soft" />
          <div className="grid gap-1.5 px-2 pt-1.5 pb-1.5">
            <div className="flex min-h-7 items-center justify-between gap-2">
              <span className="flex items-center gap-1.5 text-xs font-medium text-content-muted">
                {t('chat:focusSession.executionMode')}
                <Kbd className="font-normal max-[650px]:hidden pointer-coarse:hidden">⇧Tab</Kbd>
              </span>
              {/* 换方式不关弹层：留下来看说明、填预算，和 ⇧Tab 循环时的反馈一致。 */}
              <SegmentedControl
                size="sm"
                aria-label={t('chat:focusSession.executionMode')}
                value={selectedRun}
                disabled={runModeDisabled}
                options={runOptions.map(({ value, label: name }) => ({ value, label: name }))}
                onValueChange={onRunModeChange}
              />
            </div>
            <p className="text-xs leading-normal text-content-muted max-[650px]:text-[13px]">
              {run.description}
            </p>
          </div>
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
                <Input
                  id="goal-token-budget-input"
                  type="number"
                  min={MIN_GOAL_TOKEN_BUDGET}
                  step={1000}
                  value={tokenBudget ?? ''}
                  placeholder={t('chat:focusSession.goalTokenBudgetUnlimited')}
                  aria-label={budgetLabel}
                  disabled={savingBudget}
                  className="text-[13px] tabular-nums"
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
    </PopoverSheet>
  )
}
