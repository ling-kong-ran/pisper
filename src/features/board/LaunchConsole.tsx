// 启动台：“在 [项目 ▾] 中 描述任务…”。↵ 派发并留在原地，⌘↵ 派发后打开该会话，⇧↵ 换行；
// 输入为空或光标在开头时 Tab 循环最近用过的项目。芯片（模型 + 思考档位、审批模式、Plan 模式）
// 的上次选择记在 pisper-launch-defaults。草稿沿用 composer-drafts，派发失败时原样放回。
// 看板仪表区、空看板的放大形态、派发浮层与手机底部表单共用这一个组件，只换外形。
import {
  forwardRef,
  useCallback,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from 'react'
import {
  ArrowRight,
  BadgeCheck,
  Bot,
  ChevronDown,
  CornerDownLeft,
  FileCheck2,
  Folder,
  ListTodo,
  Plus,
  ShieldAlert,
  Target,
  Users,
  type LucideIcon,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Chip } from '@/components/ui/chip'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Kbd } from '@/components/ui/kbd'
import {
  clearComposerDraft,
  readComposerDraft,
  updateComposerDraft,
} from '@/features/chat/public-launch'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { BoardLaunchError, launchBoardTask } from './board-launch'
import { projectDisambiguation } from './card-presentation'
import {
  LAUNCH_EXECUTION_MODES,
  useLaunchDefaultsStore,
  type LaunchExecutionMode,
  type LaunchRunMode,
} from './launch-defaults-store'
import { useLaunchOptions, type LaunchModelOption } from './useLaunchOptions'

// 启动台草稿与会话草稿共用同一套存储，键名不会与会话 id 冲突。
export const LAUNCH_DRAFT_KEY = 'board:launch'

export type LaunchConsoleVariant = 'deck' | 'collapsed' | 'hero' | 'overlay' | 'sheet'

// fill：空看板的“试试”建议只填入草稿并聚焦，不重建组件（派发中的状态与错误随之保留）。
export type LaunchConsoleHandle = { focus: () => void; fill: (text: string) => void }

export type LaunchConsoleProps = {
  variant: LaunchConsoleVariant
  autoFocus?: boolean
  // 1080 等窄宽度：模型只显示短名，审批模式只显示图标（配提示文字）。
  compactChips?: boolean
  // 派发被 Runtime 受理后回调；open 表示用户按了 ⌘↵，宿主负责打开会话。
  onDispatched?: (sessionId: string, options: { open: boolean }) => void
  onConfigureModels: () => void
  className?: string
}

type LaunchError = { message: string; noModel: boolean }

const EXECUTION_ICONS: Record<LaunchExecutionMode, LucideIcon> = {
  'approval-required': FileCheck2,
  'workspace-write': BadgeCheck,
  'full-access': ShieldAlert,
}

const RUN_MODE_ICONS: Record<LaunchRunMode, LucideIcon> = {
  plan: ListTodo,
  goal: Target,
  team: Users,
}

function useLaunchLabels() {
  const { t } = useI18n()
  return useMemo(
    () => ({
      thinking: (level: string) =>
        ({
          off: t('chat:focusSession.thinkingLabel.off'),
          minimal: t('chat:focusSession.thinkingLabel.minimal'),
          low: t('chat:focusSession.thinkingLabel.low'),
          medium: t('chat:focusSession.thinkingLabel.medium'),
          high: t('chat:focusSession.thinkingLabel.high'),
          xhigh: t('chat:focusSession.thinkingLabel.xhigh'),
          max: t('chat:focusSession.thinkingLabel.max'),
        })[level] || level,
      execution: {
        'approval-required': t('chat:focusSession.approvalRequired'),
        'workspace-write': t('chat:focusSession.workspaceWrite'),
        'full-access': t('chat:focusSession.fullAccess'),
      } satisfies Record<LaunchExecutionMode, string>,
      runMode: {
        plan: t('chat:focusSession.planMode'),
        goal: t('chat:focusSession.goalMode'),
        team: t('chat:focusSession.teamMode'),
      } satisfies Record<LaunchRunMode, string>,
    }),
    [t],
  )
}

// 光标在开头（且没有选区）或输入为空时，Tab 用来切换项目而不是移走焦点。
function caretAtStart(element: HTMLTextAreaElement) {
  return !element.value || (element.selectionStart === 0 && element.selectionEnd === 0)
}

export const LaunchConsole = forwardRef<LaunchConsoleHandle, LaunchConsoleProps>(
  function LaunchConsole(
    { variant, autoFocus, compactChips, onDispatched, onConfigureModels, className },
    ref,
  ) {
    const { t } = useI18n()
    const labels = useLaunchLabels()
    const options = useLaunchOptions()
    const defaults = useLaunchDefaultsStore()
    const newTaskKey = useShortcutLabel('primaryAction')
    const inputRef = useRef<HTMLTextAreaElement>(null)
    const [value, setValueState] = useState(() => readComposerDraft(LAUNCH_DRAFT_KEY).text)
    const [error, setError] = useState<LaunchError | null>(null)
    const [pending, setPending] = useState(0)
    const inputId = `launch-input-${variant}`
    const errorId = `launch-error-${variant}`

    const setValue = useCallback((next: string) => {
      setValueState(next)
      updateComposerDraft(LAUNCH_DRAFT_KEY, { text: next })
    }, [])

    useImperativeHandle(
      ref,
      () => ({
        focus: () => inputRef.current?.focus(),
        fill: (text: string) => {
          setValue(text)
          inputRef.current?.focus()
        },
      }),
      [setValue],
    )

    // 记住的选择可能已失效（模型被移除、项目目录不再出现）：失效项退回 Runtime 默认，不清掉偏好。
    const modelKnown = options.models.some((model) => model.key === defaults.model)
    const model = modelKnown ? defaults.model : options.defaultModel
    const currentModel: LaunchModelOption | undefined = options.models.find(
      (item) => item.key === model,
    )
    const thinkingLevels = currentModel?.thinkingLevels ?? []
    const thinkingLevel = thinkingLevels.includes(defaults.thinkingLevel)
      ? defaults.thinkingLevel
      : ''
    // 没选档位时按 Runtime 默认档位显示（只显示，不写入会话），让用户知道这次派发用哪一档。
    const shownThinking =
      thinkingLevel ||
      (thinkingLevels.includes(options.defaultThinkingLevel) ? options.defaultThinkingLevel : '')
    const executionMode: LaunchExecutionMode = defaults.executionMode || 'approval-required'
    const runMode: LaunchRunMode =
      defaults.runMode === 'team' && !options.teamAvailable
        ? 'plan'
        : defaults.runMode === 'goal' && !options.goalsAvailable
          ? 'plan'
          : defaults.runMode
    const projects = useMemo(() => {
      if (!defaults.cwd || options.projects.some((project) => project.cwd === defaults.cwd))
        return options.projects
      const label =
        defaults.cwd
          .replace(/[\\/]+$/, '')
          .split(/[\\/]/)
          .pop() || defaults.cwd
      return [{ key: defaults.cwd, cwd: defaults.cwd, label }, ...options.projects]
    }, [defaults.cwd, options.projects])
    const projectHints = useMemo(() => projectDisambiguation(projects), [projects])
    // 没有记住的目录时沿用最近项目；都没有时交给 Runtime 默认目录。
    const cwd = defaults.cwd || projects[0]?.cwd || ''
    const project = projects.find((item) => item.cwd === cwd)
    const noModel = !options.loading && !options.error && options.models.length === 0

    const cycleProject = (direction: 1 | -1) => {
      if (projects.length < 2) return false
      const index = Math.max(
        0,
        projects.findIndex((item) => item.cwd === cwd),
      )
      const next = projects[(index + direction + projects.length) % projects.length]
      defaults.rememberLaunch({ cwd: next.cwd })
      return true
    }

    const dispatch = async (open: boolean) => {
      const prompt = value.trim()
      if (!prompt) {
        inputRef.current?.focus()
        return
      }
      if (noModel) {
        setError({ message: t('board:launch.noModel'), noModel: true })
        return
      }
      setError(null)
      // 先清空输入并保持焦点，马上可以写下一个想法；失败时把这段描述放回草稿。
      setValue('')
      clearComposerDraft(LAUNCH_DRAFT_KEY)
      inputRef.current?.focus()
      setPending((count) => count + 1)
      defaults.rememberLaunch({
        cwd,
        model: modelKnown ? model : '',
        thinkingLevel,
        executionMode,
        runMode,
      })
      try {
        const sessionId = await launchBoardTask({
          prompt,
          cwd,
          model: modelKnown ? model : '',
          thinkingLevel,
          executionMode: defaults.executionMode ? executionMode : '',
          runMode,
        })
        onDispatched?.(sessionId, { open })
      } catch (cause) {
        const current = readComposerDraft(LAUNCH_DRAFT_KEY).text
        const restored = current.trim() ? `${prompt}\n${current}` : prompt
        setValue(restored)
        const detail =
          cause instanceof BoardLaunchError
            ? cause.message
            : cause instanceof Error
              ? cause.message
              : ''
        setError({
          message: detail
            ? t('board:launch.failedWithReason', { reason: detail })
            : t('board:launch.failed'),
          noModel: false,
        })
      } finally {
        setPending((count) => Math.max(0, count - 1))
      }
    }

    const keyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
      if (event.nativeEvent.isComposing || event.keyCode === 229) return
      if (event.key === 'Enter' && !event.shiftKey && !event.altKey) {
        event.preventDefault()
        void dispatch(event.metaKey || event.ctrlKey)
        return
      }
      if (
        event.key === 'Tab' &&
        !event.shiftKey &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.altKey &&
        caretAtStart(event.currentTarget) &&
        cycleProject(1)
      )
        event.preventDefault()
    }

    const roomy = variant === 'hero' || variant === 'overlay' || variant === 'sheet'
    const collapsed = variant === 'collapsed'
    const lines = value.split('\n').length
    const rows = collapsed ? 1 : Math.min(roomy ? 6 : 3, Math.max(roomy ? 3 : 1, lines))
    const chipSize = variant === 'sheet' ? 'default' : 'sm'
    const ExecutionIcon = EXECUTION_ICONS[executionMode]
    const RunModeIcon = RUN_MODE_ICONS[runMode]
    const modelLabel = currentModel
      ? compactChips
        ? currentModel.label
        : `${currentModel.label}${shownThinking ? ` · ${labels.thinking(shownThinking)}` : ''}`
      : model
        ? model.split('/').at(-1) || model
        : t('board:launch.defaultModel')

    const projectChip = (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          {/* 派发目标仓库最不能看错：芯片正文至少 8 个字符，悬停给完整路径。 */}
          <Chip
            size={chipSize}
            className="max-w-[24ch] font-mono"
            title={project?.cwd || undefined}
            aria-label={t('board:launch.projectLabel', {
              project: project
                ? projectHints.get(project.key)
                  ? `${project.label} (${projectHints.get(project.key)})`
                  : project.label
                : t('board:launch.defaultProject'),
            })}
          >
            <Folder aria-hidden="true" />
            <span className="min-w-[8ch] truncate">
              {project?.label || t('board:launch.defaultProject')}
            </span>
            <ChevronDown aria-hidden="true" />
          </Chip>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="max-h-80 w-64 overflow-y-auto">
          <DropdownMenuLabel>{t('board:launch.projects')}</DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={cwd}
            onValueChange={(next) => defaults.rememberLaunch({ cwd: next })}
          >
            {/* 空目录表示“沿用最近项目”，只有一个项目都没有时才真正落到 Runtime 默认目录；
                有项目时选它会立刻解析回最近项目，选择不生效，因此只在没有项目时列出。 */}
            {projects.length === 0 && (
              <DropdownMenuRadioItem value="">
                {t('board:launch.defaultProject')}
              </DropdownMenuRadioItem>
            )}
            {projects.map((item) => (
              <DropdownMenuRadioItem key={item.key} value={item.cwd} title={item.cwd}>
                <span className="min-w-0 truncate font-mono text-xs">{item.label}</span>
                {projectHints.get(item.key) && (
                  <span className="ml-auto min-w-0 truncate pl-2 font-mono text-xs text-content-muted">
                    {projectHints.get(item.key)}
                  </span>
                )}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    )

    const chips = (
      <div
        className={cn(
          'flex min-w-0 flex-1 items-center gap-1.5 overflow-hidden',
          variant === 'sheet' && 'flex-wrap overflow-visible',
        )}
      >
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Chip
              size={chipSize}
              className="min-w-0 max-w-[24ch]"
              title={t('board:launch.modelTitle', { model: modelLabel })}
              aria-label={t('board:launch.modelTitle', { model: modelLabel })}
            >
              <Bot aria-hidden="true" />
              <span className="min-w-0 truncate">{modelLabel}</span>
              <ChevronDown aria-hidden="true" />
            </Chip>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="max-h-96 w-72 overflow-y-auto">
            <DropdownMenuLabel>{t('board:launch.model')}</DropdownMenuLabel>
            {options.models.length ? (
              <DropdownMenuRadioGroup
                value={model}
                onValueChange={(next) =>
                  defaults.rememberLaunch({ model: next, thinkingLevel: '' })
                }
              >
                {options.models.map((item) => (
                  <DropdownMenuRadioItem key={item.key} value={item.key}>
                    <span className="min-w-0 truncate">
                      {item.providerName} · {item.label}
                    </span>
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            ) : (
              <p className="px-2 py-1.5 text-xs text-content-muted">
                {options.loading ? t('board:launch.loadingModels') : t('board:launch.noModel')}
              </p>
            )}
            {thinkingLevels.length > 1 && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuLabel>{t('board:launch.thinking')}</DropdownMenuLabel>
                <DropdownMenuRadioGroup
                  value={thinkingLevel}
                  onValueChange={(next) => defaults.rememberLaunch({ thinkingLevel: next })}
                >
                  <DropdownMenuRadioItem value="">
                    {t('board:launch.thinkingDefault')}
                  </DropdownMenuRadioItem>
                  {thinkingLevels.map((level) => (
                    <DropdownMenuRadioItem key={level} value={level}>
                      {labels.thinking(level)}
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Chip
              size={chipSize}
              title={`${t('chat:focusSession.approvalMode')} · ${labels.execution[executionMode]}`}
              aria-label={`${t('chat:focusSession.approvalMode')} · ${labels.execution[executionMode]}`}
              // 完全访问会跳过所有审批且会被记住：窄宽度下也写出文字并用警示色，避免悄悄沿用。
              className={cn(
                executionMode === 'full-access' &&
                  'border-st-attn-border bg-st-attn-soft text-st-attn',
              )}
            >
              <ExecutionIcon aria-hidden="true" />
              {(!compactChips || executionMode === 'full-access') && (
                <span className="truncate">{labels.execution[executionMode]}</span>
              )}
              <ChevronDown aria-hidden="true" />
            </Chip>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="w-56">
            <DropdownMenuLabel>{t('chat:focusSession.approvalMode')}</DropdownMenuLabel>
            <DropdownMenuRadioGroup
              value={executionMode}
              onValueChange={(next) =>
                defaults.rememberLaunch({ executionMode: next as LaunchExecutionMode })
              }
            >
              {LAUNCH_EXECUTION_MODES.map((mode) => (
                <DropdownMenuRadioItem key={mode} value={mode}>
                  {labels.execution[mode]}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Chip
              size={chipSize}
              title={`${t('chat:focusSession.executionMode')} · ${labels.runMode[runMode]}`}
              aria-label={`${t('chat:focusSession.executionMode')} · ${labels.runMode[runMode]}`}
            >
              <RunModeIcon aria-hidden="true" />
              {!compactChips && <span className="truncate">{labels.runMode[runMode]}</span>}
              <ChevronDown aria-hidden="true" />
            </Chip>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="w-48">
            <DropdownMenuLabel>{t('chat:focusSession.executionMode')}</DropdownMenuLabel>
            <DropdownMenuRadioGroup
              value={runMode}
              onValueChange={(next) => defaults.rememberLaunch({ runMode: next as LaunchRunMode })}
            >
              <DropdownMenuRadioItem value="plan">{labels.runMode.plan}</DropdownMenuRadioItem>
              {options.goalsAvailable && (
                <DropdownMenuRadioItem value="goal">{labels.runMode.goal}</DropdownMenuRadioItem>
              )}
              {options.teamAvailable && (
                <DropdownMenuRadioItem value="team">{labels.runMode.team}</DropdownMenuRadioItem>
              )}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    )

    const submit = (
      <Button
        type="button"
        size={variant === 'sheet' ? 'lg' : 'sm'}
        className={cn('flex-none', variant === 'sheet' && 'min-h-11 w-full px-4')}
        onClick={() => void dispatch(false)}
        aria-keyshortcuts="Enter"
      >
        {t('board:launch.dispatch')}
        <CornerDownLeft aria-hidden="true" className="opacity-70" />
      </Button>
    )

    const input = (
      <div
        className={cn(
          'flex min-w-0 flex-1 items-start gap-1.5 rounded-sm border border-control bg-panel px-2 transition-[border-color,box-shadow] duration-(--d-fast) has-[textarea:focus-visible]:border-focus has-[textarea:focus-visible]:ring-3 has-[textarea:focus-visible]:ring-focus-ring',
          collapsed ? 'h-8 items-center' : 'py-1',
          variant === 'sheet' && 'min-h-11 py-1.5',
        )}
      >
        <span
          className={cn(
            'flex flex-none items-center gap-1 text-sm text-content-muted',
            !collapsed && 'pt-0.5',
          )}
        >
          <span aria-hidden="true">{t('board:launch.in')}</span>
          {projectChip}
          <span aria-hidden="true">{t('board:launch.inSuffix')}</span>
        </span>
        <label htmlFor={inputId} className="sr-only">
          {t('board:launch.inputLabel')}
        </label>
        <textarea
          ref={inputRef}
          id={inputId}
          data-launch-input
          autoFocus={autoFocus}
          rows={rows}
          value={value}
          // 触屏没有 Tab：手机底部表单用不带键盘提示的占位文字。
          placeholder={
            variant === 'sheet' ? t('board:launch.placeholderTouch') : t('board:launch.placeholder')
          }
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? errorId : undefined}
          aria-keyshortcuts="Enter Meta+Enter Control+Enter Shift+Enter"
          onChange={(event) => {
            setValue(event.target.value)
            if (error && !error.noModel) setError(null)
          }}
          onKeyDown={keyDown}
          // 启动台是“轮到你了”的输入位：琥珀光标是规格允许的唯一例外。
          className={cn(
            'min-w-0 flex-1 resize-none bg-transparent text-sm leading-6 text-content caret-st-attn-fill outline-none placeholder:text-content-muted',
            collapsed && 'h-8 overflow-hidden py-1 leading-6 whitespace-nowrap',
            variant === 'sheet' && 'text-[calc(var(--text-base)+1px)]',
          )}
        />
      </div>
    )

    const errorLine = error && (
      <p id={errorId} role="alert" className="flex min-w-0 items-center gap-2 text-xs text-st-fail">
        <span className="min-w-0 truncate">{error.message}</span>
        {error.noModel && (
          <Button variant="link" size="xs" className="h-auto px-0" onClick={onConfigureModels}>
            {t('board:launch.configureModels')}
            <ArrowRight aria-hidden="true" />
          </Button>
        )}
      </p>
    )

    const status = pending > 0 && (
      <span role="status" className="text-xs text-content-muted">
        {t('board:launch.sending', { count: pending })}
      </span>
    )

    if (collapsed)
      return (
        <section
          aria-label={t('board:launch.title')}
          data-launch-console={variant}
          className={cn(
            'flex h-11 min-w-0 items-center gap-2 rounded-card border border-stroke bg-solid px-2 shadow-card',
            className,
          )}
        >
          <span className="flex flex-none items-center gap-1 pl-1 text-sm font-semibold text-content">
            <Plus className="size-4 text-content-muted" aria-hidden="true" />
            {t('board:launch.title')}
          </span>
          {input}
          {status}
          {submit}
          {error && <span className="sr-only">{error.message}</span>}
        </section>
      )

    return (
      <section
        aria-label={t('board:launch.title')}
        data-launch-console={variant}
        className={cn(
          'flex min-w-0 flex-col gap-2',
          variant === 'deck' && 'rounded-card border border-stroke bg-solid p-3 shadow-card',
          variant === 'hero' && 'rounded-card border border-stroke bg-solid p-4 shadow-card',
          className,
        )}
      >
        {(variant === 'deck' || variant === 'hero') && (
          <header className="flex min-w-0 items-center gap-2">
            <Plus className="size-4 flex-none text-content-muted" aria-hidden="true" />
            <h2 className="flex-none text-sm font-semibold text-content">
              {t('board:launch.title')}
            </h2>
            {newTaskKey && <Kbd>{newTaskKey}</Kbd>}
            <span className="min-w-0 truncate text-xs text-content-muted">
              {t('board:launch.hint')}
            </span>
          </header>
        )}
        {input}
        {errorLine}
        {variant === 'sheet' ? (
          // 手机底部表单：芯片自由换行，派发键独占一行的 44px 大按钮。
          <>
            {chips}
            {status}
            {submit}
          </>
        ) : (
          <div className="flex min-w-0 items-center gap-2">
            {chips}
            {status}
            {submit}
          </div>
        )}
      </section>
    )
  },
)
