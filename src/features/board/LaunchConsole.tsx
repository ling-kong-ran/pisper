// 启动台：“在 [项目 ▾] 中 描述任务…”。↵ 派发并留在原地，⌘↵ 派发后打开该会话，⇧↵ 换行；
// 输入为空或光标在开头时 Tab 循环最近用过的项目。芯片（模型 + 思考档位、审批模式、Plan 模式）
// 的上次选择记在 pisper-launch-defaults（“完全访问”只对当前草稿生效，派发后回到安全模式）。
// 草稿沿用 composer-drafts，派发失败时原样放回，并同步给所有挂着的启动台实例。
// 看板仪表区、空看板的放大形态、派发浮层与手机底部表单共用这一个组件，只换外形。
// composer：会话主页的“新任务”草稿输入框（会话页输入框的外形）。↵ 发出第一条消息时才建会话，
// 宿主随即以 replace 打开它；草稿期间只发一次，不像启动台那样连发。
import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from 'react'
import {
  ArrowRight,
  ArrowUp,
  BadgeCheck,
  Bot,
  ChevronDown,
  CornerDownLeft,
  FileCheck2,
  Folder,
  ListTodo,
  Shield,
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
import {
  clearComposerDraft,
  readComposerDraft,
  updateComposerDraft,
} from '@/features/chat/public-launch'
import { SCROLL_FADE_CLASS, useScrollFade } from '@/hooks/useScrollFade'
import { ariaKeyShortcuts } from '@/lib/shortcut-aria'
import { formatShortcut, matchesShortcut } from '@/lib/shortcuts'
import { TOUCH_HIT_AREA } from '@/lib/touch-target'
import { cn } from '@/lib/utils'
import { useShortcutStore } from '@/stores/shortcut-store'
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
// 会话主页的新任务草稿单独一份：草稿上的 ⌘N 与标题栏“新任务”改为聚焦草稿，但派发浮层仍可能叠在草稿上面
//（命令面板的“新会话”，或主页还在加载时打开了浮层、随后落到草稿），共用一份会让两边的文本互相串改。
const HOME_DRAFT_KEY = 'home:new'

// composer-drafts 没有订阅机制：派发失败放回草稿时，由这里通知挂着同一份草稿的启动台实例重读，
// 否则浮层关闭后重新打开、或仪表区与浮层同时存在时，界面会显示不含被放回描述的旧文本。
const launchDraftListeners = new Set<{ key: string; apply: (text: string) => void }>()

function restoreDraftText(key: string, prompt: string) {
  const current = readComposerDraft(key).text
  const restored = current.trim() ? `${prompt}\n${current}` : prompt
  updateComposerDraft(key, { text: restored })
  for (const listener of launchDraftListeners) if (listener.key === key) listener.apply(restored)
}

// 看板时期的仪表区启动台（deck / hero / collapsed）随泳道退役（A3）：全部代理没有任务输入框，新任务走派发对话框。
export type LaunchConsoleVariant = 'overlay' | 'sheet' | 'composer'

// fill：空看板的“试试”建议只填入草稿并聚焦，不重建组件（派发中的状态与错误随之保留）。
// focusEnd：聚焦并把光标放到末尾（主页草稿认领 ⌘N 时用，接着已写的内容往下写）。
export type LaunchConsoleHandle = {
  focus: () => void
  focusEnd: () => void
  fill: (text: string) => void
}

export type LaunchConsoleProps = {
  variant: LaunchConsoleVariant
  autoFocus?: boolean
  // 窄宽度（主页草稿 1080 宽）：模型只显示短名，审批模式只显示图标（配提示文字）。
  compactChips?: boolean
  // composer：触屏（手机外壳）用不带键盘提示的占位与 44px 发送键。
  touch?: boolean
  // 派发被 Runtime 受理后回调；open 表示用户按了 ⌘↵，宿主负责打开会话。
  onDispatched?: (sessionId: string, options: { open: boolean }) => void
  // 派发失败时本实例已经卸载（例如浮层在发送途中被关闭）：错误行没人看得到，交给宿主用全局提示。
  // 草稿此时已经放回 composer-drafts，宿主只需提供“重新打开”的入口。
  onDetachedFailure?: (message: string) => void
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
    {
      variant,
      autoFocus,
      compactChips,
      touch,
      onDispatched,
      onDetachedFailure,
      onConfigureModels,
      className,
    },
    ref,
  ) {
    const { t } = useI18n()
    const labels = useLaunchLabels()
    const options = useLaunchOptions()
    const defaults = useLaunchDefaultsStore()
    // 会话主页的草稿输入框与会话页输入框同一套发送键（设置 → 快捷键的 sendMessage）：改成 ⌘↵ 后 ↵ 只换行，
    // 不会在草稿上误建会话。启动台其余外形仍是 ↵ 派发、⌘↵ 派发并打开。
    const sendBinding = useShortcutStore((state) => state.bindings.sendMessage)
    const inputRef = useRef<HTMLTextAreaElement>(null)
    const chipRowRef = useRef<HTMLDivElement>(null)
    const draftKey = variant === 'composer' ? HOME_DRAFT_KEY : LAUNCH_DRAFT_KEY
    const restoreLaunchDraft = (prompt: string) => restoreDraftText(draftKey, prompt)
    const [value, setValueState] = useState(() => readComposerDraft(draftKey).text)
    const [error, setError] = useState<LaunchError | null>(null)
    const [pending, setPending] = useState(0)
    // 本次草稿临时选了“完全访问”：不写进偏好，派发后回到记住的安全模式。
    const [fullAccess, setFullAccess] = useState(false)
    const mountedRef = useRef(true)
    const inputId = `launch-input-${variant}`
    const errorId = `launch-error-${variant}`

    useEffect(() => {
      mountedRef.current = true
      const listener = { key: draftKey, apply: (text: string) => setValueState(text) }
      launchDraftListeners.add(listener)
      return () => {
        mountedRef.current = false
        launchDraftListeners.delete(listener)
      }
    }, [draftKey])

    const setValue = useCallback(
      (next: string) => {
        setValueState(next)
        updateComposerDraft(draftKey, { text: next })
      },
      [draftKey],
    )

    useImperativeHandle(
      ref,
      () => ({
        focus: () => inputRef.current?.focus(),
        focusEnd: () => {
          const input = inputRef.current
          if (!input) return
          input.focus()
          input.setSelectionRange(input.value.length, input.value.length)
          // Chromium 系（含 WebView2、Android WebView）设置选区不会滚动：长草稿超过高度上限时光标落在可见区之下，要手动滚到底。
          input.scrollTop = input.scrollHeight
        },
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
    const executionMode: LaunchExecutionMode = fullAccess
      ? 'full-access'
      : defaults.executionMode || 'approval-required'
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
    // 没有记住的目录时沿用最近有会话的项目（登记的空项目虽排在列表前面，不作隐式默认）；
    // 都没有时交给 Runtime 默认目录。
    const cwd = defaults.cwd || options.defaultCwd || projects[0]?.cwd || ''
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

    const composer = variant === 'composer'
    // 芯片文字变化（换项目、换模型、完全访问）会改变溢出量，作为重新测量的键。
    const chipFade = useScrollFade(
      chipRowRef,
      composer ? `${cwd}|${model}|${executionMode}|${runMode}` : '',
    )
    const dispatch = async (open: boolean) => {
      const prompt = value.trim()
      if (!prompt) {
        inputRef.current?.focus()
        return
      }
      // 草稿只建一个会话：第一条还在发送时不再受理，避免连按建出两个会话。
      if (composer && pending > 0) return
      if (noModel) {
        setError({ message: t('board:launch.noModel'), noModel: true })
        return
      }
      setError(null)
      // 先清空输入并保持焦点，马上可以写下一个想法；失败时把这段描述放回草稿。
      setValue('')
      clearComposerDraft(draftKey)
      inputRef.current?.focus()
      setPending((count) => count + 1)
      // 完全访问只作用于这一次派发：偏好里保留原来的安全模式，下一条草稿回到安全模式。
      setFullAccess(false)
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
          executionMode: fullAccess ? 'full-access' : defaults.executionMode,
          runMode,
        })
        onDispatched?.(sessionId, { open })
      } catch (cause) {
        restoreLaunchDraft(prompt)
        const detail =
          cause instanceof BoardLaunchError
            ? cause.message
            : cause instanceof Error
              ? cause.message
              : ''
        const message = detail
          ? t('board:launch.failedWithReason', { reason: detail })
          : t('board:launch.failed')
        if (mountedRef.current) setError({ message, noModel: false })
        else onDetachedFailure?.(message)
      } finally {
        if (mountedRef.current) setPending((count) => Math.max(0, count - 1))
      }
    }

    const keyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
      if (event.nativeEvent.isComposing || event.keyCode === 229) return
      if (composer) {
        if (matchesShortcut(event.nativeEvent, sendBinding)) {
          event.preventDefault()
          // 草稿只建一个会话，发出后宿主总是打开它，不区分“留在原地”。
          void dispatch(true)
          return
        }
      } else if (event.key === 'Enter' && !event.shiftKey && !event.altKey) {
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

    const lines = value.split('\n').length
    const rows = Math.min(6, Math.max(3, lines))
    const chipSize = variant === 'sheet' || composer ? 'default' : 'sm'
    // 触屏上的 24px 小芯片：芯片行里与“派发”（触屏 44px）同一行，可见高度直接抬到 44；
    // 输入框里的项目芯片保持可见尺寸，命中区用伪元素补到 44（单行启动台的输入框只有 32px 高）。
    const rowChipTouch = chipSize === 'sm' ? 'pointer-coarse:min-h-11' : undefined
    const projectChipTouch = chipSize === 'sm' ? TOUCH_HIT_AREA : undefined
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
            // 手机草稿的工具行还要放模型与审批芯片：项目芯片收到 14ch，完整路径在菜单与悬停里。
            className={cn(composer && touch ? 'max-w-[14ch]' : 'max-w-[24ch]', projectChipTouch)}
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
            {/* 目录名是机器路径，用等宽；“默认目录”是中文说明，不用等宽字体（规格 6.4）。 */}
            <span className={cn('truncate', project?.label ? 'min-w-[8ch] font-mono' : 'min-w-0')}>
              {project?.label || t('board:launch.defaultProject')}
            </span>
            <ChevronDown aria-hidden="true" />
          </Chip>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="start"
          // 登记的空项目会让列表变长：最多用到视口可用高度，不在 320px 处把末项截半。
          className="max-h-[min(24rem,var(--radix-dropdown-menu-content-available-height))] w-64 overflow-y-auto"
        >
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

    const modelMenu = (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Chip
            size={chipSize}
            // 草稿输入框里的模型芯片与会话页一致：无描边、靠右（规格 v2 第 4.3 节）。
            className={cn(
              'min-w-0 max-w-[24ch]',
              composer && 'border-transparent bg-transparent',
              rowChipTouch,
            )}
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
              onValueChange={(next) => defaults.rememberLaunch({ model: next, thinkingLevel: '' })}
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
    )
    const approvalMenu = (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Chip
            size={chipSize}
            title={`${t('chat:focusSession.approvalMode')} · ${labels.execution[executionMode]}${fullAccess ? ` · ${t('board:launch.fullAccessOnce')}` : ''}`}
            aria-label={`${t('chat:focusSession.approvalMode')} · ${labels.execution[executionMode]}${fullAccess ? ` · ${t('board:launch.fullAccessOnce')}` : ''}`}
            // 完全访问会跳过所有审批：窄宽度下也写出文字并用警示色，派发前一眼能看到。
            className={cn(
              rowChipTouch,
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
            onValueChange={(next) => {
              const mode = next as LaunchExecutionMode
              setFullAccess(mode === 'full-access')
              // 安全模式照常记住；完全访问只留在本次草稿里。
              if (mode !== 'full-access') defaults.rememberLaunch({ executionMode: mode })
            }}
          >
            {LAUNCH_EXECUTION_MODES.map((mode) => (
              <DropdownMenuRadioItem key={mode} value={mode}>
                {mode === 'full-access' ? (
                  <span className="flex min-w-0 flex-col">
                    <span>{labels.execution[mode]}</span>
                    <span className="text-xs text-content-muted">
                      {t('board:launch.fullAccessOnce')}
                    </span>
                  </span>
                ) : (
                  labels.execution[mode]
                )}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    )
    const runModeMenu = (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Chip
            size={chipSize}
            className={rowChipTouch}
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
    )
    // 主页新任务草稿的工具行与会话页输入框一致（规格 v2 第 4.3 节）：审批模式与执行方式合在一枚盾牌芯片里，
    // 模型芯片靠右、紧挨石墨圆形发送键。完全访问只对本次草稿生效，芯片用盾牌警示图标与文字区分，不染琥珀。
    const ModeIcon = executionMode === 'full-access' ? ShieldAlert : Shield
    const modeLabel =
      runMode === 'plan'
        ? labels.execution[executionMode]
        : `${labels.runMode[runMode]} · ${labels.execution[executionMode]}`
    const modeMenu = (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Chip
            size={chipSize}
            className={cn('min-w-0 max-w-[26ch] border-transparent bg-transparent', rowChipTouch)}
            title={`${t('chat:focusSession.approvalMode')} · ${modeLabel}${fullAccess ? ` · ${t('board:launch.fullAccessOnce')}` : ''}`}
            aria-label={`${t('chat:focusSession.approvalMode')} · ${modeLabel}${fullAccess ? ` · ${t('board:launch.fullAccessOnce')}` : ''}`}
            data-launch-mode={`${runMode}:${executionMode}`}
          >
            <ModeIcon aria-hidden="true" />
            <span className="truncate">
              {compactChips && runMode === 'plan' && executionMode !== 'full-access'
                ? ''
                : modeLabel}
            </span>
            <ChevronDown aria-hidden="true" />
          </Chip>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-64">
          <DropdownMenuLabel>{t('chat:focusSession.approvalMode')}</DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={executionMode}
            onValueChange={(next) => {
              const mode = next as LaunchExecutionMode
              setFullAccess(mode === 'full-access')
              // 安全模式照常记住；完全访问只留在本次草稿里。
              if (mode !== 'full-access') defaults.rememberLaunch({ executionMode: mode })
            }}
          >
            {LAUNCH_EXECUTION_MODES.map((mode) => (
              <DropdownMenuRadioItem key={mode} value={mode}>
                {mode === 'full-access' ? (
                  <span className="flex min-w-0 flex-col">
                    <span>{labels.execution[mode]}</span>
                    <span className="text-xs text-content-muted">
                      {t('board:launch.fullAccessOnce')}
                    </span>
                  </span>
                ) : (
                  labels.execution[mode]
                )}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
          {(options.goalsAvailable || options.teamAvailable) && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuLabel>{t('chat:focusSession.executionMode')}</DropdownMenuLabel>
              <DropdownMenuRadioGroup
                value={runMode}
                onValueChange={(next) =>
                  defaults.rememberLaunch({ runMode: next as LaunchRunMode })
                }
              >
                <DropdownMenuRadioItem value="plan">{labels.runMode.plan}</DropdownMenuRadioItem>
                {options.goalsAvailable && (
                  <DropdownMenuRadioItem value="goal">{labels.runMode.goal}</DropdownMenuRadioItem>
                )}
                {options.teamAvailable && (
                  <DropdownMenuRadioItem value="team">{labels.runMode.team}</DropdownMenuRadioItem>
                )}
              </DropdownMenuRadioGroup>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    )

    const chips = (
      <div
        ref={chipRowRef}
        data-fade={composer ? chipFade : undefined}
        data-launch-chips
        className={cn(
          'flex min-w-0 flex-1 items-center gap-1.5',
          variant === 'sheet'
            ? 'flex-wrap overflow-visible'
            : // 输入框工具行：窄屏放不下时横向滑动，不截断芯片；被裁的一端渐隐，提示还能滑。
              composer
              ? cn('overflow-x-auto [scrollbar-width:none]', SCROLL_FADE_CLASS)
              : 'overflow-hidden',
        )}
      >
        {composer ? (
          <>
            {projectChip}
            {modeMenu}
          </>
        ) : (
          <>
            {modelMenu}
            {approvalMenu}
            {runModeMenu}
          </>
        )}
      </div>
    )

    const submit = composer ? (
      // 草稿的发送键与会话页输入框同一枚石墨圆键：桌面 32px，触屏 44px。
      <Button
        type="button"
        size="icon"
        className={cn(
          'flex-none rounded-full',
          touch ? 'size-11' : 'size-8 pointer-coarse:size-11',
        )}
        disabled={pending > 0}
        aria-label={t('board:home.send')}
        aria-keyshortcuts={ariaKeyShortcuts(sendBinding)}
        title={sendBinding ? `${t('board:home.send')} (${formatShortcut(sendBinding)})` : undefined}
        data-launch-send
        onClick={() => void dispatch(false)}
      >
        <ArrowUp aria-hidden="true" />
      </Button>
    ) : (
      <Button
        type="button"
        size={variant === 'sheet' ? 'lg' : 'sm'}
        className={cn(
          'flex-none',
          variant === 'sheet' && 'min-h-11 w-full px-4',
          composer && 'pointer-coarse:min-h-11',
        )}
        disabled={composer && pending > 0}
        onClick={() => void dispatch(false)}
        aria-keyshortcuts={composer ? ariaKeyShortcuts(sendBinding) : 'Enter'}
      >
        {composer ? t('board:home.send') : t('board:launch.dispatch')}
        <CornerDownLeft aria-hidden="true" className="opacity-70" />
      </Button>
    )

    // 手机底部表单（含手机空看板）：“在 [项目 ▾] 中”单独一行，输入框占满下一行。
    // 同一行并排时输入框只剩项目芯片让出的宽度，390 宽下只有约 112px。
    const stacked = variant === 'sheet'
    const input = (
      <div
        className={cn(
          'flex min-w-0 flex-1 items-start gap-1.5 rounded-sm border border-control bg-panel px-2 transition-[border-color,box-shadow] duration-(--d-fast) has-[textarea:focus-visible]:border-focus has-[textarea:focus-visible]:ring-3 has-[textarea:focus-visible]:ring-focus-ring',
          'py-1',
          stacked && 'min-h-11 flex-wrap gap-y-1 py-1.5',
        )}
      >
        <span
          className={cn(
            'flex flex-none items-center gap-1 text-sm text-content-muted',
            'pt-0.5',
            stacked && 'max-w-full min-w-0',
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
            // 触屏：文本框至少 44px 高。
            !stacked && 'pointer-coarse:min-h-11',
            stacked && 'min-h-11 w-full basis-full text-[calc(var(--text-base)+1px)]',
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
      <span role="status" className="flex-none text-xs text-content-muted">
        {composer ? t('board:home.sending') : t('board:launch.sending', { count: pending })}
      </span>
    )

    if (composer)
      return (
        <section
          aria-label={t('board:home.composerLabel')}
          data-launch-console={variant}
          // 与会话页输入框同一外形：22 圆角卡片（窄屏 16）、聚焦描边，工具行在下方。
          className={cn(
            'flex min-w-0 flex-col rounded-[var(--composer-radius,22px)] border border-control bg-panel shadow-composer transition-[border-color,box-shadow] duration-(--d-fast) has-[textarea:focus-visible]:border-focus has-[textarea:focus-visible]:ring-3 has-[textarea:focus-visible]:ring-focus-ring max-[650px]:rounded-2xl',
            className,
          )}
        >
          <label htmlFor={inputId} className="sr-only">
            {t('board:launch.inputLabel')}
          </label>
          <textarea
            ref={inputRef}
            id={inputId}
            data-launch-input
            autoFocus={autoFocus}
            rows={Math.min(6, Math.max(2, lines))}
            value={value}
            placeholder={
              touch
                ? t('board:home.placeholderTouch')
                : sendBinding
                  ? t('board:home.placeholder', { send: formatShortcut(sendBinding) })
                  : t('board:home.placeholderUnbound')
            }
            aria-invalid={error ? true : undefined}
            aria-describedby={error ? errorId : undefined}
            aria-keyshortcuts={ariaKeyShortcuts(sendBinding)}
            onChange={(event) => {
              setValue(event.target.value)
              if (error && !error.noModel) setError(null)
            }}
            onKeyDown={keyDown}
            className={cn(
              'max-h-[220px] min-h-11 w-full min-w-0 resize-none bg-transparent px-4 pt-3 pb-1 text-[length:calc(var(--app-message-font-size)*.9375)] leading-5 text-content outline-none placeholder:text-content-muted max-[650px]:px-3.5',
              touch && 'text-[length:var(--app-message-font-size)]',
            )}
          />
          {errorLine && <div className="px-4 max-[650px]:px-3.5">{errorLine}</div>}
          <div className="flex min-w-0 items-center gap-1.5 px-2 pt-1 pb-2" data-launch-toolbar>
            {chips}
            {status}
            <span className="flex min-w-0 shrink items-center">{modelMenu}</span>
            {submit}
          </div>
        </section>
      )

    return (
      <section
        aria-label={t('board:launch.title')}
        data-launch-console={variant}
        className={cn('flex min-w-0 flex-col gap-2', className)}
      >
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
