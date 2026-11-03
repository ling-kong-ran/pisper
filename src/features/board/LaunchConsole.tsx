// 启动台：“在 [项目 ▾] 中 描述任务…”。↵ 派发并留在原地，⌘↵ 派发后打开该会话，⇧↵ 换行；
// 输入为空或光标在开头时 Tab 循环最近用过的项目。芯片（模型 + 思考档位、审批模式、Plan 模式）
// 的上次选择记在 pisper-launch-defaults（“完全访问”只对当前草稿生效，派发后回到安全模式）。
// 草稿沿用 composer-drafts，派发失败时原样放回，并同步给所有挂着的启动台实例。
// 看板仪表区、空看板的放大形态、派发浮层与手机底部表单共用这一个组件，只换外形。
// composer：会话主页的“新任务”草稿输入框（会话页输入框的外形）。↵ 发出第一条消息时才建会话，
// 宿主随即以 replace 打开它；草稿期间只发一次，不像启动台那样连发。
import {
  forwardRef,
  lazy,
  Suspense,
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
  ChevronDown,
  CornerDownLeft,
  Folder,
  FolderOpen,
  FolderPlus,
  Shield,
  ShieldAlert,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Chip } from '@/components/ui/chip'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  ModelThinkingControl,
  clearComposerDraft,
  ComposerCommandMenu,
  getWorkspaceCommands,
  readComposerDraft,
  runtimeSettingCommands,
  updateComposerDraft,
} from '@/features/chat/public-launch'
import { SCROLL_FADE_CLASS, useScrollFade } from '@/hooks/useScrollFade'
import { ariaKeyShortcuts } from '@/lib/shortcut-aria'
import { formatShortcut, matchesShortcut } from '@/lib/shortcuts'
import { TOUCH_HIT_AREA } from '@/lib/touch-target'
import { cn } from '@/lib/utils'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { useShortcutStore } from '@/stores/shortcut-store'
import { BoardLaunchError, launchBoardTask } from './board-launch'
import { projectDisambiguation } from './card-presentation'
import { StartAfterChip } from './StartAfterChip'
import { useBoardItems } from './useBoardItems'
import {
  LAUNCH_EXECUTION_MODES,
  useLaunchDefaultsStore,
  type LaunchExecutionMode,
  type LaunchRunMode,
} from './launch-defaults-store'
import { useLaunchOptions, type LaunchModelOption } from './useLaunchOptions'
import { useNewProjectPicker } from './useNewProjectPicker'

// 系统目录对话框不可用时才用到的手动填写对话框与新建文件夹对话框：按需加载，不进启动台首屏。
const WorkspacePicker = lazy(() =>
  import('@/components/WorkspacePicker').then((module) => ({ default: module.WorkspacePicker })),
)
const NewFolderDialog = lazy(() =>
  import('./NewFolderDialog').then((module) => ({ default: module.NewFolderDialog })),
)

// 启动台草稿与会话草稿共用同一套存储，键名不会与会话 id 冲突。
export const LAUNCH_DRAFT_KEY = 'board:launch'
// 会话主页的新任务草稿单独一份：草稿上的 ⌘N 与标题栏“新任务”改为聚焦草稿，但派发浮层仍可能叠在草稿上面
//（命令面板的“新会话”，或主页还在加载时打开了浮层、随后落到草稿），共用一份会让两边的文本互相串改。
const HOME_DRAFT_KEY = 'home:new'
// 项目菜单里“不选项目”的取值：NUL 不会出现在合法路径里，不会与某个目录撞上。
const NO_PROJECT_VALUE = '\0no-project'

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
      // 手机草稿工具行的短名，与会话页盾牌芯片一致（规格 7.2）。
      executionShort: {
        'approval-required': t('chat:focusSession.approvalRequiredShort'),
        'workspace-write': t('chat:focusSession.workspaceWriteShort'),
        'full-access': t('chat:focusSession.fullAccessShort'),
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
    // “排在…之后”（A9，sessionQueue）：只对这一条草稿生效，派发后回到“立即开始”。
    const [startAfter, setStartAfter] = useState('')
    const { items: boardItems } = useBoardItems('today')
    // 芯片在窄工具行里只放得下“等「重构…”：选了会话时在工具行下面写一句完整的说明。
    const startAfterTitle = startAfter
      ? (boardItems.find((item) => item.id === startAfter)?.title ??
        t('board:launch.startAfterGone'))
      : ''
    const mountedRef = useRef(true)
    // 项目菜单的“新建项目…”：等菜单关闭、焦点归还阶段再打开目录对话框，否则焦点会被菜单拉回芯片。
    const afterMenuClose = useRef<(() => void) | null>(null)
    // 新建的项目直接成为这条草稿的派发目标；选完把光标交回输入框接着写。
    // 触屏不自动聚焦：选完目录立刻弹出软键盘会挡住刚换好的项目芯片。
    const projectPicker = useNewProjectPicker({
      onProjectChosen: () => {
        if (touch || variant === 'sheet') return
        const input = inputRef.current
        if (!input) return
        input.focus()
        input.setSelectionRange(input.value.length, input.value.length)
      },
      onError: (message) => {
        if (mountedRef.current) setError({ message, noModel: false })
      },
    })
    const { mounted: pickerMounted, ...pickerProps } = projectPicker.picker
    const { mounted: folderDialogMounted, ...folderDialogProps } = projectPicker.folderDialog
    // 旧 Runtime 没有新建文件夹接口：菜单里只留“选择文件夹…”。
    const canCreateFolder = useRuntimeCapabilitiesStore(
      (state) => state.capabilities.features.workspaceFolders,
    )
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
    // 不选项目时不带目录，Runtime 为新会话建空文件夹；记住的项目留着，切回时不用重选。
    const projectless = defaults.projectless
    const cwd = projectless ? '' : defaults.cwd || options.defaultCwd || projects[0]?.cwd || ''
    const project = projectless ? undefined : projects.find((item) => item.cwd === cwd)
    const projectChipLabel =
      project?.label ||
      (projectless ? t('board:launch.noProject') : t('board:launch.defaultProject'))
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

    // 斜杠命令（与会话页同一菜单）：/model /effort /approval /run 改的是这条草稿的派发设置，
    // 与工具行芯片走同一套写法；Prompt / Skill 按所选项目目录列出。
    const chooseExecutionMode = (mode: LaunchExecutionMode) => {
      setFullAccess(mode === 'full-access')
      // 安全模式照常记住；完全访问只留在本次草稿里。
      if (mode !== 'full-access') defaults.rememberLaunch({ executionMode: mode })
    }
    const loadCommands = useCallback(() => getWorkspaceCommands(cwd), [cwd])
    const builtinCommands = runtimeSettingCommands({
      t,
      models: options.models,
      model,
      onModel: (key) => defaults.rememberLaunch({ model: key, thinkingLevel: '' }),
      thinkingLevels,
      thinkingLevel: shownThinking,
      thinkingLabel: labels.thinking,
      onThinking: (level) => defaults.rememberLaunch({ thinkingLevel: level }),
      executionModes: LAUNCH_EXECUTION_MODES,
      executionMode,
      executionLabel: (mode) => labels.execution[mode as LaunchExecutionMode] || mode,
      onExecutionMode: (mode) => chooseExecutionMode(mode as LaunchExecutionMode),
      runModes: [
        'plan',
        ...(options.goalsAvailable ? ['goal'] : []),
        ...(options.teamAvailable ? ['team'] : []),
      ],
      runMode,
      runModeLabel: (mode) => labels.runMode[mode as LaunchRunMode] || mode,
      onRunMode: (mode) => defaults.rememberLaunch({ runMode: mode as LaunchRunMode }),
    })
    const commandMenu = (
      <ComposerCommandMenu
        placement={variant === 'composer' ? 'top' : 'bottom'}
        resourceKey={cwd}
        loadCommands={loadCommands}
        builtinCommands={builtinCommands}
        value={value}
        onChange={setValue}
        inputRef={inputRef}
      />
    )

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
        ...(projectless ? { projectless } : { cwd }),
        model: modelKnown ? model : '',
        thinkingLevel,
        executionMode,
        runMode,
      })
      // “排在…之后”同样只作用于这一次派发。
      const after = startAfter
      setStartAfter('')
      try {
        const sessionId = await launchBoardTask({
          prompt,
          cwd,
          projectless,
          model: modelKnown ? model : '',
          thinkingLevel,
          executionMode: fullAccess ? 'full-access' : defaults.executionMode,
          runMode,
          ...(after ? { startAfter: after } : {}),
        })
        onDispatched?.(sessionId, { open })
      } catch (cause) {
        restoreLaunchDraft(prompt)
        if (mountedRef.current && after) setStartAfter(after)
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
    const sheet = variant === 'sheet'
    // 手机底部表单与桌面派发同一条芯片工具行，宽度只有 358：模型只显示短名，Plan + 安全模式的盾牌只留图标。
    const compact = compactChips || sheet
    const chipSize = sheet || composer ? 'default' : 'sm'
    // 手机草稿（composer + touch）：工具行与会话页输入框同一种样子——无边框浅灰胶囊、短名（规格 7.2）；
    // 深色下 surface-muted 与输入框底色几乎相同，改用 surface-active 才分得出胶囊。
    const phoneComposer = composer && touch
    const phonePill =
      phoneComposer &&
      'rounded-full border-transparent bg-surface-muted shadow-none dark:bg-surface-active'
    // 手机底部表单：芯片与派发键都是 44 高（规格 7，触控目标 ≥44）。
    // 桌面对话框的 24px 小芯片：触屏时可见高度直接抬到 44；项目芯片的命中区用伪元素补到 44。
    const rowChipTouch = sheet
      ? 'min-h-11'
      : chipSize === 'sm'
        ? 'pointer-coarse:min-h-11'
        : undefined
    const projectChipTouch = sheet ? 'min-h-11' : chipSize === 'sm' ? TOUCH_HIT_AREA : undefined
    const projectChip = (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          {/* 派发目标仓库最不能看错：芯片正文至少 8 个字符，悬停给完整路径。 */}
          <Chip
            size={chipSize}
            // 手机草稿的工具行还要放模型与审批芯片：项目芯片收到 14ch，完整路径在菜单与悬停里。
            // 手机底部表单的工具行只有 358 宽：项目芯片可以继续收窄（完整路径在菜单与悬停里），给盾牌与模型留位置。
            className={cn(
              sheet
                ? 'max-w-[13ch] min-w-0 shrink'
                : phoneComposer
                  ? 'max-w-[14ch]'
                  : // 派发对话框多了“排在…之后”芯片（A9）：放不下时项目芯片收窄（正文仍至少 8 个字符），不被挤出左边。
                    'max-w-[24ch] min-w-0 shrink',
              projectChipTouch,
              phonePill,
            )}
            title={project?.cwd || undefined}
            aria-label={t('board:launch.projectLabel', {
              project: project
                ? projectHints.get(project.key)
                  ? `${project.label} (${projectHints.get(project.key)})`
                  : project.label
                : projectChipLabel,
            })}
          >
            <Folder aria-hidden="true" />
            {/* 目录名是机器路径，用等宽；“默认目录”是中文说明，不用等宽字体（规格 6.4）。 */}
            {/* 正文至少 8 个字符只为长名字截断后仍可辨；短名字（如“ws”）按自身宽度，不在芯片里留一段空白。 */}
            <span
              className={cn(
                'truncate',
                project?.label && !sheet && projectChipLabel.length > 8 ? 'min-w-[8ch]' : 'min-w-0',
                project?.label && 'font-mono',
              )}
            >
              {projectChipLabel}
            </span>
            <ChevronDown aria-hidden="true" className={phoneComposer ? 'hidden' : undefined} />
          </Chip>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="start"
          // 登记的空项目会让列表变长：最多用到视口可用高度，不在 320px 处把末项截半。
          className="max-h-[min(24rem,var(--radix-dropdown-menu-content-available-height))] w-64 overflow-y-auto"
          onCloseAutoFocus={(event) => {
            const next = afterMenuClose.current
            if (!next) return
            afterMenuClose.current = null
            event.preventDefault()
            next()
          }}
        >
          <DropdownMenuRadioGroup
            value={projectless ? NO_PROJECT_VALUE : cwd}
            onValueChange={(next) =>
              defaults.rememberLaunch(
                next === NO_PROJECT_VALUE ? { projectless: true } : { cwd: next },
              )
            }
          >
            <DropdownMenuRadioItem value={NO_PROJECT_VALUE}>
              <span className="min-w-0 truncate">{t('board:launch.noProject')}</span>
            </DropdownMenuRadioItem>
            <DropdownMenuSeparator />
            <DropdownMenuLabel>{t('board:launch.projects')}</DropdownMenuLabel>
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
          <DropdownMenuSeparator />
          {/* 列表里没有想要的目录时就地新建项目：用系统文件管理器选一个已有文件夹，或者新建一个空文件夹；
              都会登记为项目并选为这条草稿的派发目标，不用先去侧栏。 */}
          <DropdownMenuItem
            disabled={projectPicker.picking}
            data-launch-new-project
            onSelect={() => (afterMenuClose.current = () => void projectPicker.newProject())}
          >
            <FolderOpen aria-hidden="true" />
            {t('board:launch.chooseFolder')}
          </DropdownMenuItem>
          {canCreateFolder && (
            <DropdownMenuItem
              data-launch-new-folder
              onSelect={() =>
                (afterMenuClose.current = () => projectPicker.newFolder(cwd || options.defaultCwd))
              }
            >
              <FolderPlus aria-hidden="true" />
              {t('board:launch.newFolder')}
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    )

    const workspacePicker = (pickerMounted || folderDialogMounted) && (
      <Suspense fallback={null}>
        {pickerMounted && <WorkspacePicker {...pickerProps} />}
        {folderDialogMounted && <NewFolderDialog {...folderDialogProps} />}
      </Suspense>
    )

    // 模型芯片与会话页是同一个组件（规格 v2 第 4.3 节）：弹层里选模型、滑条调思考等级。
    // 新任务没有会话，档位只记在派发偏好里；没选过时按 Runtime 默认档位显示，可“恢复默认”。
    const modelMenu = (
      <ModelThinkingControl
        sheet={sheet}
        side={composer ? 'top' : 'bottom'}
        model={model}
        models={options.models}
        onModelChange={(key) => defaults.rememberLaunch({ model: key, thinkingLevel: '' })}
        thinkingLevel={shownThinking}
        levels={thinkingLevels}
        status={options.loading ? '' : 'supported'}
        onThinkingChange={(level) => defaults.rememberLaunch({ thinkingLevel: level })}
        thinkingIsDefault={!thinkingLevel}
        onThinkingReset={() => defaults.rememberLaunch({ thinkingLevel: '' })}
        modelDisabled={!options.models.length}
        triggerClassName={cn(sheet ? 'max-w-[13ch]' : 'max-w-[24ch]', rowChipTouch)}
      />
    )
    // 主页新任务草稿的工具行与会话页输入框一致（规格 v2 第 4.3 节）：审批模式与执行方式合在一枚盾牌芯片里，
    // 模型芯片靠右、紧挨石墨圆形发送键。完全访问只对本次草稿生效，芯片用盾牌警示图标与文字区分，不染琥珀。
    const ModeIcon = executionMode === 'full-access' ? ShieldAlert : Shield
    const modeLabel =
      runMode === 'plan'
        ? labels.execution[executionMode]
        : `${labels.runMode[runMode]} · ${labels.execution[executionMode]}`
    // 手机草稿的盾牌芯片和会话页一样写短名；底部派发表单的工具行更窄，仍只留图标。
    const modeChipText = phoneComposer
      ? runMode === 'plan'
        ? labels.executionShort[executionMode]
        : `${runMode === 'team' ? t('board:composer.teamShort') : t('board:composer.goalShort')} · ${labels.executionShort[executionMode]}`
      : compact && runMode === 'plan' && executionMode !== 'full-access'
        ? ''
        : modeLabel
    const modeMenu = (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Chip
            size={chipSize}
            className={cn(
              'min-w-0 max-w-[26ch]',
              phoneComposer ? phonePill : 'border-transparent bg-transparent',
              // 手机底部表单只剩盾牌图标：不让它被项目芯片挤没。
              sheet && 'flex-none',
              rowChipTouch,
            )}
            title={`${t('chat:focusSession.approvalMode')} · ${modeLabel}${fullAccess ? ` · ${t('board:launch.fullAccessOnce')}` : ''}`}
            aria-label={`${t('chat:focusSession.approvalMode')} · ${modeLabel}${fullAccess ? ` · ${t('board:launch.fullAccessOnce')}` : ''}`}
            data-launch-mode={`${runMode}:${executionMode}`}
          >
            <ModeIcon aria-hidden="true" />
            <span className="truncate">{modeChipText}</span>
            <ChevronDown aria-hidden="true" className={phoneComposer ? 'hidden' : undefined} />
          </Chip>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-64">
          <DropdownMenuLabel>{t('chat:focusSession.approvalMode')}</DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={executionMode}
            onValueChange={(next) => chooseExecutionMode(next as LaunchExecutionMode)}
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

    const startAfterChip = (
      <StartAfterChip
        value={startAfter}
        onChange={setStartAfter}
        size={chipSize}
        compact={compact}
        className={cn(rowChipTouch, sheet && 'flex-none')}
      />
    )

    const chips = (
      <div
        ref={chipRowRef}
        data-fade={chipFade}
        data-launch-chips
        // 输入框工具行：窄屏放不下时横向滑动，不截断芯片；被裁的一端渐隐，提示还能滑。
        className={cn(
          'flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto [scrollbar-width:none]',
          SCROLL_FADE_CLASS,
        )}
      >
        {projectChip}
        {modeMenu}
        {startAfterChip}
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
        size="sm"
        className={cn('flex-none', sheet && 'min-h-11 px-4')}
        onClick={() => void dispatch(false)}
        aria-keyshortcuts="Enter"
      >
        {t('board:launch.dispatch')}
        {/* 手机没有回车键位提示：只写“派发”。 */}
        {!sheet && <CornerDownLeft aria-hidden="true" className="opacity-70" />}
      </Button>
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
            'flex min-w-0 flex-col rounded-[var(--composer-radius,22px)] border border-control bg-panel shadow-composer transition-[border-color,box-shadow] duration-(--d-fast) has-[textarea:focus-visible]:border-focus has-[textarea:focus-visible]:ring-3 has-[textarea:focus-visible]:ring-focus-ring max-[650px]:rounded-[28px]',
            className,
          )}
        >
          <label htmlFor={inputId} className="sr-only">
            {t('board:launch.inputLabel')}
          </label>
          {commandMenu}
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
          {workspacePicker}
        </section>
      )

    // 桌面派发对话框（规格 2.5，效果图 extra-dispatch-light.png）：无框的大输入区在上，下面一条工具行
    // [项目 ▾]（描边）[盾牌 审批 · 执行方式 ▾] … [模型 · 思考 ▾] [派发 ↵]，与主页新任务草稿同一套芯片；
    // 键位不变（↵ 派发、⌘↵ 派发并打开、⇧↵ 换行、Tab 在开头切项目）。
    // 手机底部表单（规格 7.2）用同一条工具行，芯片与派发键都是 44 高；输入框带细线外框、16px 字（iOS 不放大），
    // 占满表单宽度。原先“在 [项目] 中”前缀 + 模型 / 审批 / Plan 三枚芯片换行的旧布局随之删除。
    return (
      <section
        aria-label={t('board:launch.title')}
        data-launch-console={variant}
        className={cn('flex min-w-0 flex-col', className)}
      >
        <label htmlFor={inputId} className="sr-only">
          {t('board:launch.inputLabel')}
        </label>
        {commandMenu}
        <textarea
          ref={inputRef}
          id={inputId}
          data-launch-input
          autoFocus={autoFocus}
          rows={Math.min(8, Math.max(3, lines))}
          value={value}
          // 触屏没有 Tab：手机底部表单用不带键盘提示的占位文字。
          placeholder={sheet ? t('board:launch.placeholderTouch') : t('board:launch.placeholder')}
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
            'w-full min-w-0 resize-none bg-transparent text-content caret-st-attn-fill outline-none placeholder:text-content-muted',
            sheet
              ? 'max-h-[40dvh] min-h-[88px] rounded-[var(--r-md)] border border-control bg-panel px-3 py-2.5 text-[16px] leading-6 transition-[border-color,box-shadow] duration-(--d-fast) focus-visible:border-focus focus-visible:ring-3 focus-visible:ring-focus-ring'
              : 'max-h-[240px] min-h-[72px] px-5 pt-1 pb-2 text-[15px] leading-6',
          )}
        />
        {errorLine && <div className={sheet ? 'pt-1.5' : 'px-5 pb-1'}>{errorLine}</div>}
        {startAfterTitle && (
          <p
            data-launch-start-after-note
            className={cn('text-xs text-content-muted', sheet ? 'pt-1.5' : 'px-5 pb-1')}
          >
            {t('board:launch.startAfterNote', { title: startAfterTitle })}
          </p>
        )}
        <div
          className={cn('flex min-w-0 items-center gap-1.5', sheet ? 'pt-2' : 'px-3 pb-3')}
          data-launch-toolbar
        >
          <div
            ref={chipRowRef}
            data-launch-chips
            // 按内容宽度起算（不是 flex-1 的 0）：放不下时与右边的模型芯片按比例一起收窄，“排在…之后”不被挤成一个字。
            className="flex min-w-0 flex-[1_1_auto] items-center gap-1.5 overflow-hidden"
          >
            {projectChip}
            {modeMenu}
            {startAfterChip}
          </div>
          {status}
          <span className="flex min-w-0 shrink items-center">{modelMenu}</span>
          {submit}
        </div>
        {workspacePicker}
      </section>
    )
  },
)
