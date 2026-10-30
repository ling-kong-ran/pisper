// 会话页检查器的“改动”标签（规格 4.4）：顶部是范围切换（本会话 / 工作区）与改动汇总，
// 中间是改动文件列表（M / A / D 标记 + 增删统计），下面是所选文件的差异（统一 / 并排、
// 等宽 12/20、双列行号、词级高亮、hunk 标题，长行横向滚动不截断），然后是所选文件的批准状态与文件操作；
// 全部批准、全部还原、刷新这类批量操作收在最底部的“⋯”里（规格 v2 第 4.4 节），汇总行只写数字。
// 数据逻辑沿用现有实现：本会话 = /file-changes 快照（useSessionFileChanges），
// 工作区 = /vcs/changes（与旧上下文面板同一个查询键，缓存互通）；这里只重做结构与样式。
import {
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react'
import { useQuery } from '@tanstack/react-query'
import { Check, FolderSearch, GitBranch, MoreHorizontal, RefreshCw, Undo2 } from 'lucide-react'
import { LOCAL_REVEAL_NOTICE_EVENT } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { SegmentedControl } from '@/components/ui/segmented-control'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { relativeTime } from '@/lib/format'
import {
  LocalPathRevealError,
  requestLocalPathReveal,
  revealPathThroughRuntime,
} from '@/lib/local-path-reveal'
import { cn } from '@/lib/utils'
import { chatApi, type SessionFileChangeFile } from './chat-api'
import { summarizeProjectDiff } from './project-diff-summary'
import type { ContextPageView } from './session-context-pages'
import {
  useSessionInspectorStore,
  type SessionInspectorChangesView,
} from './session-inspector-store'
import { TaskFileDiff } from './TaskChangesPane'
import { useSessionFileChanges } from './use-session-file-changes'

const MAX_VISIBLE_FILES = 100

type ChangeStatus = 'modified' | 'created' | 'deleted' | 'renamed' | 'other'

type ChangedFile = {
  path: string
  status: ChangeStatus
  added: number | null
  removed: number | null
  // 行尾的状态说明（已批准 / 已撤销），同时写进可访问名称。
  note?: string
}

// 版本控制的状态码（git porcelain / svn）→ 文件标记；认不出的原样显示首字母。
function vcsStatus(code: string): ChangeStatus {
  const value = code.trim().toUpperCase()
  if (value === '??' || value.includes('A')) return 'created'
  if (value.includes('D')) return 'deleted'
  if (value.includes('R')) return 'renamed'
  if (value.includes('M')) return 'modified'
  return 'other'
}

const STATUS_LETTER: Record<ChangeStatus, string> = {
  modified: 'M',
  created: 'A',
  deleted: 'D',
  renamed: 'R',
  other: '·',
}

const STATUS_TONE: Record<ChangeStatus, string> = {
  modified: 'text-content-secondary',
  created: 'text-st-done',
  deleted: 'text-st-fail',
  renamed: 'text-content-secondary',
  other: 'text-content-muted',
}

function splitPath(path: string) {
  const normalized = path.replace(/\\/g, '/')
  const index = normalized.lastIndexOf('/')
  return index < 0
    ? { directory: '', name: normalized }
    : { directory: normalized.slice(0, index + 1), name: normalized.slice(index + 1) }
}

// 会话快照里的路径相对工作区；交给系统文件管理器前拼成绝对路径（Runtime 会再校验）。
function absolutePath(cwd: string, path: string) {
  if (/^(?:[a-zA-Z]:[\\/]|\/|\\\\)/.test(path)) return path
  return `${cwd.replace(/[\\/]+$/, '')}/${path}`
}

function emitRevealNotice(message: string, tone: 'info' | 'error') {
  window.dispatchEvent(new CustomEvent(LOCAL_REVEAL_NOTICE_EVENT, { detail: { message, tone } }))
}

function DiffStats({ added, removed }: { added: number | null; removed: number | null }) {
  if (added === null || removed === null) return null
  return (
    // 红绿只用于 diff 增删（规格 6.1）。v2 把 --st-done / --st-fail 改成了中性灰，这里原先借用它们，增删数因此失去颜色。
    <span className="flex-none font-mono text-xs tabular-nums">
      <span className="text-success">+{added}</span> <span className="text-danger">−{removed}</span>
    </span>
  )
}

// 改动页最底部的一行：左边是状态（待批准数、进行中的批量操作），右边“⋯”收着批量操作。
function ChangesFooter({ status, children }: { status?: ReactNode; children: ReactNode }) {
  const { t } = useI18n()
  return (
    <footer
      className="mt-auto flex h-10 shrink-0 items-center gap-2 border-t border-stroke-soft pr-1.5 pl-3 text-xs text-content-muted"
      data-changes-footer
    >
      <span className="flex min-w-0 flex-1 items-center gap-1.5 truncate">{status}</span>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="pointer-coarse:size-11"
            aria-label={t('board:changes.more')}
            title={t('board:changes.more')}
            data-changes-more
          >
            <MoreHorizontal aria-hidden="true" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" side="top" className="min-w-48">
          {children}
        </DropdownMenuContent>
      </DropdownMenu>
    </footer>
  )
}

function StatusLetter({ status }: { status: ChangeStatus }) {
  const { t } = useI18n()
  const label =
    status === 'created'
      ? t('board:changes.statusCreated')
      : status === 'deleted'
        ? t('board:changes.statusDeleted')
        : status === 'renamed'
          ? t('board:changes.statusRenamed')
          : t('board:changes.statusModified')
  return (
    <span
      className={cn(
        'w-3.5 flex-none text-center font-mono text-xs font-semibold',
        STATUS_TONE[status],
      )}
      title={label}
    >
      <span aria-hidden="true">{STATUS_LETTER[status]}</span>
      <span className="sr-only">{label}</span>
    </span>
  )
}

// 文件列表：每行一个按钮，点击即选中并在下方显示差异；↑ / ↓ 在行间移动并选中。
function ChangedFileList({
  files,
  selected,
  label,
  onSelect,
}: {
  files: ChangedFile[]
  selected: string
  label: string
  onSelect: (path: string) => void
}) {
  const listRef = useRef<HTMLUListElement>(null)
  const move = (event: KeyboardEvent<HTMLUListElement>) => {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return
    const buttons = [
      ...(listRef.current?.querySelectorAll<HTMLButtonElement>('[data-change-file]') ?? []),
    ]
    const current = buttons.findIndex((button) => button === document.activeElement)
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? buttons.length - 1
          : Math.min(
              buttons.length - 1,
              Math.max(0, current + (event.key === 'ArrowDown' ? 1 : -1)),
            )
    const target = buttons[next]
    if (!target) return
    event.preventDefault()
    target.focus()
    onSelect(target.dataset.changeFile || '')
  }
  return (
    <ul
      ref={listRef}
      aria-label={label}
      className="max-h-[34%] min-h-11 shrink-0 overflow-y-auto border-b border-stroke-soft py-1"
      onKeyDown={move}
      data-change-list
    >
      {files.slice(0, MAX_VISIBLE_FILES).map((file) => {
        const active = file.path === selected
        const { directory, name } = splitPath(file.path)
        return (
          <li key={file.path} className="px-1">
            <button
              type="button"
              title={file.path}
              // 可访问名称保持“路径 状态 统计”顺序（与旧面板一致，冒烟按名称定位）。
              aria-label={[
                file.path,
                STATUS_LETTER[file.status],
                file.added !== null && file.removed !== null
                  ? `+${file.added} −${file.removed}`
                  : '',
                file.note || '',
              ]
                .filter(Boolean)
                .join(' ')}
              aria-current={active ? 'true' : undefined}
              data-change-file={file.path}
              className={cn(
                'flex h-8 w-full min-w-0 items-center gap-2 rounded-sm px-2 text-left text-xs transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus max-[650px]:h-11 pointer-coarse:h-11',
                active
                  ? 'bg-solid shadow-[inset_0_0_0_1px_var(--stroke-strong)]'
                  : 'hover:bg-surface-hover',
              )}
              onClick={() => onSelect(file.path)}
            >
              <StatusLetter status={file.status} />
              <span className="flex min-w-0 flex-1 font-mono">
                <span className="min-w-0 truncate text-content-muted">{directory}</span>
                <span className="flex-none text-content">{name}</span>
              </span>
              {file.note && <span className="flex-none text-content-muted">{file.note}</span>}
              <DiffStats added={file.added} removed={file.removed} />
            </button>
          </li>
        )
      })}
      {files.length > MAX_VISIBLE_FILES && (
        <li className="px-3 py-1.5 text-xs text-content-muted tabular-nums">
          <MoreFiles count={files.length - MAX_VISIBLE_FILES} />
        </li>
      )}
    </ul>
  )
}

function MoreFiles({ count }: { count: number }) {
  const { t } = useI18n()
  return <>{t('chat:focusSession.gitMoreFiles', { count })}</>
}

// 所选文件的差异：文件头（路径、标记、统计、统一 / 并排切换）+ 填满剩余高度的差异区 + 页脚。
// 并排视图至少要两栏各约 300px 才读得下；更窄的检查器（默认 456、覆盖层 420）只给统一视图，
// 偏好保留，拖宽后自动恢复并排。
const SPLIT_DIFF_MIN_WIDTH = 600

function ChangeDetail({
  sessionId,
  file,
  scope,
  revision,
  view,
  onViewChange,
  footer,
}: {
  sessionId: string
  file: ChangedFile
  scope: 'task' | 'project'
  revision: number
  view?: ContextPageView
  onViewChange?: (patch: Partial<ContextPageView>) => void
  footer: ReactNode
}) {
  const { t } = useI18n()
  const diffMode = useSessionInspectorStore((state) => state.diffMode)
  const setDiffMode = useSessionInspectorStore((state) => state.setDiffMode)
  const sectionRef = useRef<HTMLElement>(null)
  const [width, setWidth] = useState(0)
  useLayoutEffect(() => {
    const element = sectionRef.current
    if (!element) return
    const measure = () => setWidth(element.clientWidth)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  // 宽度未知（首帧）时按可用处理，避免偏好为并排的人先看到一闪统一视图。
  const splitAvailable = width === 0 || width >= SPLIT_DIFF_MIN_WIDTH
  const shownMode = splitAvailable ? diffMode : 'unified'
  const splitHint = splitAvailable ? undefined : t('board:changes.splitTooNarrow')
  return (
    <section
      ref={sectionRef}
      className="flex min-h-0 flex-1 flex-col"
      aria-label={file.path}
      data-change-detail={file.path}
    >
      <header className="flex min-h-10 shrink-0 items-center gap-2 border-b border-stroke-soft py-1.5 pr-2 pl-3">
        <StatusLetter status={file.status} />
        <strong
          className="min-w-0 flex-1 truncate font-mono text-xs font-semibold text-content"
          title={file.path}
        >
          {file.path}
        </strong>
        <DiffStats added={file.added} removed={file.removed} />
        {/* 禁用的段不接收指针事件，悬停落在整组上：说明写在组的 title 与描述里。 */}
        <SegmentedControl
          size="sm"
          aria-label={t('board:changes.diffView')}
          title={splitHint}
          aria-description={splitHint}
          value={shownMode}
          onValueChange={setDiffMode}
          options={[
            { value: 'unified', label: t('board:changes.unified') },
            { value: 'split', label: t('board:changes.split'), disabled: !splitAvailable },
          ]}
        />
      </header>
      <TaskFileDiff
        key={`${scope}:${file.path}`}
        sessionId={sessionId}
        path={file.path}
        scope={scope}
        revision={revision}
        mode={shownMode}
        fill
        position={view?.reading?.[file.path]}
        onReading={(position) =>
          onViewChange?.({ reading: { ...view?.reading, [file.path]: position } })
        }
      />
      <footer className="flex min-h-10 shrink-0 flex-wrap items-center gap-1 border-t border-stroke-soft py-1 pr-1.5 pl-3 text-xs">
        {footer}
      </footer>
    </section>
  )
}

// 在系统文件管理器中显示：只在桌面壳提供（Runtime 的 reveal-path 要求本机进程）；
// 现有能力里没有“在编辑器打开”，不伪造这个入口。
function RevealButton({ cwd, path }: { cwd: string; path: string }) {
  const { t } = useI18n()
  if (!window.pisperDesktop || !cwd) return null
  const target = absolutePath(cwd, path)
  return (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      className="max-[650px]:h-11"
      onClick={async () => {
        try {
          await requestLocalPathReveal(target, revealPathThroughRuntime)
          emitRevealNotice(t('common:markdownMessage.revealLocalPathOk', { path }), 'info')
        } catch (error) {
          emitRevealNotice(
            error instanceof LocalPathRevealError && error.reason === 'unavailable'
              ? t('common:markdownMessage.revealLocalPathUnavailable')
              : error instanceof LocalPathRevealError && error.reason === 'timeout'
                ? t('common:markdownMessage.revealLocalPathTimeout')
                : t('common:markdownMessage.revealLocalPathFailed', { path }),
            'error',
          )
        }
      }}
    >
      <FolderSearch aria-hidden="true" />
      {t('chat:chatMessage.fileActionReveal')}
    </Button>
  )
}

function snapshotFile(file: SessionFileChangeFile): ChangedFile {
  return {
    path: file.path,
    status:
      file.status === 'created' ? 'created' : file.status === 'deleted' ? 'deleted' : 'modified',
    added: file.added,
    removed: file.removed,
  }
}

// 本会话：/file-changes 快照，带批准与还原（还原前沿用原有的确认框）。
function SessionScopeChanges({
  sessionId,
  cwd,
  streaming,
  view,
  onViewChange,
  requestConfirm,
}: {
  sessionId: string
  cwd: string
  streaming: boolean
  view?: ContextPageView
  onViewChange?: (patch: Partial<ContextPageView>) => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t, language } = useI18n()
  const {
    changes,
    loading,
    running,
    snapshotRevision,
    writesDisabled,
    pendingCount,
    reversibleCount,
    displayedError,
    notice,
    runAction,
    refresh,
  } = useSessionFileChanges({ sessionId, streaming, requestConfirm })
  const files = changes?.files ?? []
  const stored = view?.expandedFiles?.[0]
  const selected = files.find((file) => file.path === stored) ?? files[0]
  const select = (path: string) => onViewChange?.({ expandedFiles: [path] })
  const busy = (kind: 'approve' | 'revert', path?: string) =>
    running?.kind === kind && running.path === path
  const waitTitle = streaming ? t('chat:focusSession.gitWaitForRunToFinish') : undefined

  return (
    <section
      className="flex min-h-0 flex-1 flex-col"
      aria-label={t('chat:focusSession.fileChanges')}
      aria-busy={loading || Boolean(running)}
      data-changes-scope="session"
    >
      <header className="flex min-h-10 shrink-0 items-center gap-1.5 border-b border-stroke-soft py-1 pr-1.5 pl-3">
        <p className="flex min-w-0 flex-1 items-center gap-2 text-xs text-content-secondary">
          <span className="min-w-0 truncate">
            {t('board:changes.sessionSummary', { count: changes?.summary.files ?? 0 })}
          </span>
          {/* 没有改动时只写“0 个文件”：+0 / −0 与禁用的批量键只是噪音，红绿也不该花在零上。 */}
          {changes && changes.summary.files > 0 && (
            <DiffStats added={changes.summary.added} removed={changes.summary.removed} />
          )}
        </p>
      </header>
      {displayedError && (
        <p role="alert" className="shrink-0 px-3 pt-2 text-xs text-st-fail">
          {displayedError}
        </p>
      )}
      {notice && !displayedError && (
        <p role="status" className="shrink-0 px-3 pt-2 text-xs text-content-secondary">
          {notice}
        </p>
      )}
      {loading && !changes ? (
        <PageState kind="loading" title={t('chat:focusSession.gitLoading')} />
      ) : !files.length ? (
        !displayedError && (
          <InlineEmptyState compact title={t('chat:focusSession.fileChangesEmpty')} />
        )
      ) : (
        <>
          <ChangedFileList
            files={files.map((file) => ({
              ...snapshotFile(file),
              note: file.reverted
                ? t('chat:focusSession.fileChangesReverted')
                : file.approved
                  ? t('chat:focusSession.fileChangesApproved')
                  : undefined,
            }))}
            selected={selected?.path ?? ''}
            label={t('board:changes.files')}
            onSelect={select}
          />
          {selected && (
            <ChangeDetail
              sessionId={sessionId}
              file={snapshotFile(selected)}
              scope="task"
              revision={snapshotRevision}
              view={view}
              onViewChange={onViewChange}
              footer={
                <>
                  <span
                    className={cn(
                      'mr-auto flex min-w-0 items-center gap-1.5 truncate',
                      selected.approved ? 'text-st-done' : 'text-content-muted',
                    )}
                    data-change-state={
                      selected.reverted ? 'reverted' : selected.approved ? 'approved' : 'pending'
                    }
                  >
                    {selected.approved && !selected.reverted && (
                      <Check className="size-3.5 flex-none" aria-hidden="true" />
                    )}
                    {selected.reverted
                      ? t('board:changes.revertedAt', {
                          time: relativeTime(selected.changedAt, language),
                        })
                      : selected.approved
                        ? t('board:changes.approvedAt', {
                            time: relativeTime(selected.changedAt, language),
                          })
                        : t('board:changes.pendingAt', {
                            time: relativeTime(selected.changedAt, language),
                          })}
                  </span>
                  {selected.pending && !selected.approved && !selected.reverted && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      disabled={writesDisabled}
                      title={waitTitle}
                      onClick={() => void runAction({ kind: 'approve', path: selected.path })}
                    >
                      {busy('approve', selected.path) ? (
                        <RefreshCw className="animate-spin" />
                      ) : (
                        <Check />
                      )}
                      {t('chat:focusSession.fileChangesApproveFile')}
                    </Button>
                  )}
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    disabled={writesDisabled || selected.reverted || !selected.canRevert}
                    title={waitTitle}
                    onClick={() => void runAction({ kind: 'revert', path: selected.path })}
                  >
                    {busy('revert', selected.path) ? (
                      <RefreshCw className="animate-spin" />
                    ) : (
                      <Undo2 />
                    )}
                    {t('board:changes.revertFile')}
                  </Button>
                  <RevealButton cwd={cwd} path={selected.path} />
                </>
              }
            />
          )}
        </>
      )}
      <ChangesFooter
        status={
          running ? (
            <>
              <RefreshCw className="size-3.5 flex-none animate-spin" aria-hidden="true" />
              {running.kind === 'approve'
                ? t('chat:focusSession.fileChangesApproveAll')
                : t('board:changes.revertAll')}
            </>
          ) : pendingCount > 0 ? (
            t('board:changes.pending', { count: pendingCount })
          ) : null
        }
      >
        {pendingCount > 0 && (
          <DropdownMenuItem
            disabled={writesDisabled}
            title={waitTitle}
            onSelect={() => void runAction({ kind: 'approve' })}
          >
            <Check />
            {t('chat:focusSession.fileChangesApproveAll')}
          </DropdownMenuItem>
        )}
        {(changes?.summary.files ?? 0) > 0 && (
          <DropdownMenuItem
            disabled={writesDisabled || reversibleCount === 0}
            title={waitTitle ?? t('chat:focusSession.fileChangesRevertAllDescription')}
            onSelect={() => void runAction({ kind: 'revert' })}
          >
            <Undo2 />
            {t('board:changes.revertAll')}
          </DropdownMenuItem>
        )}
        {(pendingCount > 0 || (changes?.summary.files ?? 0) > 0) && <DropdownMenuSeparator />}
        <DropdownMenuItem disabled={loading || Boolean(running)} onSelect={refresh}>
          <RefreshCw />
          {t('chat:focusSession.gitRefresh')}
        </DropdownMenuItem>
      </ChangesFooter>
    </section>
  )
}

// 工作区：版本控制里的全部未提交改动（不止本会话），只读查看；分支名放在汇总行。
function WorkspaceScopeChanges({
  sessionId,
  cwd,
  streaming,
  view,
  onViewChange,
}: {
  sessionId: string
  cwd: string
  streaming: boolean
  view?: ContextPageView
  onViewChange?: (patch: Partial<ContextPageView>) => void
}) {
  const { t } = useI18n()
  const query = useQuery({
    queryKey: ['task-project-changes', sessionId, cwd, streaming],
    queryFn: ({ signal }) => chatApi.getVcsChanges(sessionId, { signal }),
  })
  const totals = useMemo(
    () =>
      summarizeProjectDiff(
        query.data?.diff || '',
        query.data?.files.map((file) => file.path) || [],
        query.data?.diffTruncated,
      ),
    [query.data],
  )
  const error = query.error?.message || query.data?.error
  const files: ChangedFile[] = (query.data?.files || []).map((file) => ({
    path: file.path,
    status: vcsStatus(file.status),
    added: Object.hasOwn(totals.files, file.path) ? totals.files[file.path].added : null,
    removed: Object.hasOwn(totals.files, file.path) ? totals.files[file.path].removed : null,
  }))
  const stored = view?.expandedFiles?.[0]
  const selected = files.find((file) => file.path === stored) ?? files[0]

  return (
    <section
      className="flex min-h-0 flex-1 flex-col"
      aria-label={t('chat:workbench.projectChanges')}
      aria-busy={query.isFetching}
      data-changes-scope="workspace"
    >
      <header className="flex min-h-10 shrink-0 items-center gap-1.5 border-b border-stroke-soft py-1 pr-1.5 pl-3">
        <p className="flex min-w-0 flex-1 items-center gap-2 text-xs text-content-secondary">
          <span className="min-w-0 truncate">
            {t('board:changes.workspaceSummary', { count: files.length })}
          </span>
          {query.data?.isRepo && !error && files.length > 0 && (
            <DiffStats added={totals.added} removed={totals.removed} />
          )}
        </p>
        {query.data?.branch && (
          <span
            className="inline-flex h-5.5 max-w-[40%] min-w-0 flex-none items-center gap-1 rounded-xs border border-stroke px-1.5 font-mono text-xs text-content-secondary"
            title={cwd}
          >
            <GitBranch size={12} className="flex-none" aria-hidden="true" />
            <span className="truncate">{query.data.branch}</span>
          </span>
        )}
      </header>
      {query.isPending ? (
        <PageState kind="loading" title={t('chat:focusSession.gitLoading')} />
      ) : error ? (
        <p role="alert" className="p-3 text-xs text-st-fail">
          {error}
        </p>
      ) : !query.data?.isRepo ? (
        <InlineEmptyState compact title={t('chat:workbench.notRepository')} />
      ) : !files.length ? (
        <InlineEmptyState compact title={t('chat:focusSession.fileChangesEmpty')} />
      ) : (
        <>
          <ChangedFileList
            files={files}
            selected={selected?.path ?? ''}
            label={t('board:changes.files')}
            onSelect={(path) => onViewChange?.({ expandedFiles: [path] })}
          />
          {selected && (
            <ChangeDetail
              sessionId={sessionId}
              file={selected}
              scope="project"
              revision={query.dataUpdatedAt}
              view={view}
              onViewChange={onViewChange}
              footer={
                <>
                  <span
                    className="mr-auto min-w-0 truncate font-mono text-content-muted"
                    title={cwd}
                  >
                    {cwd}
                  </span>
                  <RevealButton cwd={cwd} path={selected.path} />
                </>
              }
            />
          )}
        </>
      )}
      <ChangesFooter
        status={
          query.isFetching ? (
            <RefreshCw className="size-3.5 flex-none animate-spin" aria-hidden="true" />
          ) : null
        }
      >
        <DropdownMenuItem disabled={query.isFetching} onSelect={() => void query.refetch()}>
          <RefreshCw />
          {t('chat:focusSession.gitRefresh')}
        </DropdownMenuItem>
      </ChangesFooter>
    </section>
  )
}

export function InspectorChanges({
  sessionId,
  cwd,
  streaming,
  scope,
  onScopeChange,
  vcsAvailable,
  view,
  onViewChange,
  requestConfirm,
}: {
  sessionId: string
  cwd: string
  streaming: boolean
  scope: SessionInspectorChangesView
  onScopeChange: (scope: SessionInspectorChangesView) => void
  vcsAvailable: boolean
  view?: ContextPageView
  onViewChange?: (patch: Partial<ContextPageView>) => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t } = useI18n()
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-inspector-changes={scope}>
      <div className="flex shrink-0 items-center gap-2 border-b border-stroke-soft px-3 py-2">
        <SegmentedControl
          size="sm"
          aria-label={t('board:inspector.changesScope')}
          value={scope}
          onValueChange={onScopeChange}
          options={[
            { value: 'session', label: t('board:inspector.scopeSession') },
            {
              value: 'workspace',
              label: t('board:inspector.scopeWorkspace'),
              disabled: !vcsAvailable,
            },
          ]}
        />
      </div>
      {!view ? (
        <p role="status" className="p-4 text-sm text-content-muted">
          {t('chat:focusSession.gitLoading')}
        </p>
      ) : scope === 'workspace' ? (
        <WorkspaceScopeChanges
          sessionId={sessionId}
          cwd={cwd}
          streaming={streaming}
          view={view}
          onViewChange={onViewChange}
        />
      ) : (
        <SessionScopeChanges
          sessionId={sessionId}
          cwd={cwd}
          streaming={streaming}
          view={view}
          onViewChange={onViewChange}
          requestConfirm={requestConfirm}
        />
      )}
    </div>
  )
}
