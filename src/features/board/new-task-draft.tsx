// 新任务草稿（'/board/new'）的看板一侧：项目、派发偏好（模型、思考档位、审批与运行模式）与“排在…之后”。
// 会话页把草稿画成会话页的样子（同一个输入框），这里只提供契约 NewTaskDraftChrome：项目芯片、+ 菜单里的
// 排队选项、第一条消息前的建会话（prepareLaunchSession）。派发浮层与主页草稿退役后，新任务只有这一个入口。
// 偏好照旧记在 pisper-launch-defaults：侧栏“在此项目新建任务”先写项目再打开草稿，草稿就落在那个项目。
import { useMemo, useRef, useState } from 'react'
import { ChevronDown, Folder, FolderOpen, FolderPlus, ListOrdered, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
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
  COMPOSER_CHIP_CLASSES,
  type NewTaskDraftChrome,
  type NewTaskProjectVariant,
} from '@/features/chat/public-launch'
import { cn } from '@/lib/utils'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { prepareLaunchSession } from './board-launch'
import { projectDisambiguation } from './card-presentation'
import { ProjectPickerDialogs } from './ProjectPickerDialogs'
import {
  LAUNCH_SAFE_EXECUTION_MODES,
  useLaunchDefaultsStore,
  type LaunchExecutionMode,
  type LaunchRunMode,
} from './launch-defaults-store'
import { useBoardItems } from './useBoardItems'
import { useLaunchOptions } from './useLaunchOptions'
import { useNewProjectPicker } from './useNewProjectPicker'

// 草稿里“不选项目”的取值：Runtime 为新会话建空文件夹。
const NO_PROJECT_VALUE = '\u0000no-project'

function isExecutionMode(mode: string): mode is LaunchExecutionMode {
  return mode === 'full-access' || (LAUNCH_SAFE_EXECUTION_MODES as readonly string[]).includes(mode)
}

function isRunMode(mode: string): mode is LaunchRunMode {
  return mode === 'plan' || mode === 'goal' || mode === 'team'
}

export function useNewTaskDraftChrome({
  onError,
}: {
  // 选目录失败等提示：交给宿主的全局提示。
  onError: (message: string) => void
}): NewTaskDraftChrome {
  const { t } = useI18n()
  const options = useLaunchOptions()
  const defaults = useLaunchDefaultsStore()
  // 本次草稿临时选了“完全访问”：不写进偏好，第一条消息发出后回到记住的安全模式。
  const [fullAccess, setFullAccess] = useState(false)
  // “排在…之后”（A9，sessionQueue）：只对这一条草稿生效，发出后回到“立即开始”。
  const [startAfter, setStartAfter] = useState('')
  const { items: boardItems, capabilities } = useBoardItems('today')
  // 项目菜单的“选择文件夹 / 新建文件夹”：等菜单关闭、焦点归还阶段再打开对话框，否则焦点会被菜单拉回芯片。
  const afterMenuClose = useRef<(() => void) | null>(null)
  const projectPicker = useNewProjectPicker({ onProjectChosen: () => undefined, onError })
  // 旧 Runtime 没有新建文件夹接口：菜单里只留“选择文件夹…”。
  const canCreateFolder = useRuntimeCapabilitiesStore(
    (state) => state.capabilities.features.workspaceFolders,
  )

  // 记住的选择可能已失效（模型被移除、项目目录不再出现）：失效项退回 Runtime 默认，不清掉偏好。
  const modelKnown = options.models.some((model) => model.key === defaults.model)
  const model = modelKnown ? defaults.model : options.defaultModel
  const thinkingLevels = options.models.find((item) => item.key === model)?.thinkingLevels ?? []
  const thinkingLevel = thinkingLevels.includes(defaults.thinkingLevel)
    ? defaults.thinkingLevel
    : ''
  // 没选档位时按 Runtime 默认档位显示（只显示，不写入会话），让人知道这次用哪一档。
  const shownThinking =
    thinkingLevel ||
    (thinkingLevels.includes(options.defaultThinkingLevel) ? options.defaultThinkingLevel : '')
  const executionMode: LaunchExecutionMode = fullAccess
    ? 'full-access'
    : defaults.executionMode || 'approval-required'
  const runMode: LaunchRunMode =
    (defaults.runMode === 'team' && !options.teamAvailable) ||
    (defaults.runMode === 'goal' && !options.goalsAvailable)
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
  // 没有记住的目录时沿用最近有会话的项目；不选项目时不带目录，记住的项目留着，切回时不用重选。
  const projectless = defaults.projectless
  const cwd = projectless ? '' : defaults.cwd || options.defaultCwd || projects[0]?.cwd || ''
  const project = projectless ? undefined : projects.find((item) => item.cwd === cwd)
  const projectLabel =
    project?.label || (projectless ? t('board:launch.noProject') : t('board:launch.defaultProject'))
  const projectAria = t('board:launch.projectLabel', {
    project: project
      ? projectHints.get(project.key)
        ? `${project.label} (${projectHints.get(project.key)})`
        : project.label
      : projectLabel,
  })

  // 桌面在输入框工具行里画成芯片；手机照参考稿不占工具行，放在顶栏“新任务”下面一行，
  // 和会话页顶栏写项目名的位置一致。
  const renderProjectControl = (variant: NewTaskProjectVariant) => (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          {/* 派发目标仓库最不能看错：悬停给完整路径；目录名是机器路径，用等宽。 */}
          <button
            type="button"
            className={
              variant === 'header'
                ? // 只有 24 高的一行字：伪元素把触控区撑到 44（规格 v2 第 7 节）。
                  'relative inline-flex h-6 max-w-full min-w-0 cursor-pointer items-center gap-1 rounded-full px-2 text-xs leading-4 text-content-muted outline-none after:absolute after:-inset-x-1 after:-inset-y-2.5 after:content-[""] focus-visible:outline-2 focus-visible:outline-focus active:bg-surface-hover aria-expanded:bg-surface-hover'
                : cn(COMPOSER_CHIP_CLASSES, 'max-w-[24ch] max-[650px]:max-w-[14ch]')
            }
            title={project?.cwd || undefined}
            aria-label={projectAria}
            data-new-task-project={projectless ? 'none' : cwd || 'default'}
          >
            <Folder size={variant === 'header' ? 12 : 14} className="shrink-0" aria-hidden="true" />
            <span className={cn('min-w-0 truncate', project?.label && 'font-mono')}>
              {projectLabel}
            </span>
            <ChevronDown
              size={variant === 'header' ? 12 : 14}
              className={cn('shrink-0 opacity-70', variant === 'chip' && 'max-[650px]:hidden')}
              aria-hidden="true"
            />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align={variant === 'header' ? 'center' : 'start'}
          side={variant === 'header' ? 'bottom' : 'top'}
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
            {/* 空目录表示“沿用最近项目”，只有一个项目都没有时才真正落到 Runtime 默认目录。 */}
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
          {/* 列表里没有想要的目录时就地新建项目：选一个已有文件夹，或者新建一个空文件夹。 */}
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
      <ProjectPickerDialogs
        picker={projectPicker.picker}
        folderDialog={projectPicker.folderDialog}
      />
    </>
  )

  // “排在…之后”只列还会“结束一次”的会话：运行中的，和本身在排队的（排队链）。
  // 没有 sessionQueue 能力、或眼下没有可排的会话（也没选过）时不出现（规格 8.6 降级矩阵）。
  const candidates = boardItems.filter((item) => item.lane === 'running' || item.lane === 'queued')
  const startAfterTitle = startAfter
    ? (boardItems.find((item) => item.id === startAfter)?.title ?? t('board:launch.startAfterGone'))
    : ''
  // 列在 + 菜单里（不用子菜单：手机上子菜单会从屏幕右缘伸出去被截掉）。
  const plusMenuItems =
    capabilities.sessionQueue && (candidates.length > 0 || startAfter) ? (
      <>
        <DropdownMenuSeparator />
        <DropdownMenuLabel
          className="flex items-center gap-1.5"
          data-composer-plus-item="start-after"
        >
          <ListOrdered size={14} aria-hidden="true" />
          {t('board:launch.startAfterLabel')}
        </DropdownMenuLabel>
        <DropdownMenuRadioGroup value={startAfter} onValueChange={setStartAfter}>
          <DropdownMenuRadioItem value="">{t('board:launch.startNowChip')}</DropdownMenuRadioItem>
          {candidates.map((item) => (
            <DropdownMenuRadioItem key={item.id} value={item.id} data-start-after-option={item.id}>
              <span className="flex min-w-0 flex-col">
                <span className="truncate">
                  {t('board:launch.startAfterChip', { title: item.title })}
                </span>
                <span className="text-xs text-content-muted">
                  {item.lane === 'running'
                    ? t('board:launch.startAfterRunning')
                    : t('board:launch.startAfterQueued')}
                </span>
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </>
    ) : undefined
  const notice = startAfter ? (
    <p
      className="flex min-w-0 items-center gap-1.5 text-xs text-content-muted"
      data-launch-start-after-note
    >
      <ListOrdered size={14} className="shrink-0" aria-hidden="true" />
      <span className="min-w-0 line-clamp-2">
        {t('board:launch.startAfterNote', { title: startAfterTitle })}
      </span>
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        className="shrink-0 pointer-coarse:size-11"
        aria-label={t('board:launch.startNowChip')}
        title={t('board:launch.startNowChip')}
        onClick={() => setStartAfter('')}
      >
        <X aria-hidden="true" />
      </Button>
    </p>
  ) : undefined

  return {
    cwd,
    model,
    thinkingLevel: shownThinking,
    availableThinkingLevels: thinkingLevels,
    executionMode,
    runMode,
    onModelChange: (key) => defaults.rememberLaunch({ model: key, thinkingLevel: '' }),
    onThinkingLevelChange: (level) => defaults.rememberLaunch({ thinkingLevel: level }),
    onExecutionModeChange: (mode) => {
      if (!isExecutionMode(mode)) return false
      setFullAccess(mode === 'full-access')
      // 安全模式照常记住；完全访问只留在本次草稿里。
      if (mode !== 'full-access') defaults.rememberLaunch({ executionMode: mode })
      return true
    },
    onRunModeChange: (mode) => {
      if (!isRunMode(mode)) return false
      defaults.rememberLaunch({ runMode: mode })
      return true
    },
    renderProjectControl,
    plusMenuItems,
    notice,
    prepare: (prompt) =>
      prepareLaunchSession({
        prompt,
        cwd,
        projectless,
        model: modelKnown ? model : '',
        thinkingLevel,
        executionMode,
        runMode,
        ...(startAfter ? { startAfter } : {}),
      }),
    onSent: () => {
      setFullAccess(false)
      setStartAfter('')
    },
  }
}
