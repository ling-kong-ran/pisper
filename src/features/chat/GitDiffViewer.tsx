import { useResourceReading, type ResourceReadingPosition } from './use-resource-reading'
// Git 差异查看器：虚拟化渲染统一 diff，支持逐块展开与滚动。
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useVirtualizer } from '@tanstack/react-virtual'
import { FileDiff, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { parseUnifiedDiff, type GitDiffCell, type GitDiffFile, type GitDiffRow } from './git-diff'

import { Button } from '@/components/ui/button'

type DiffFileEntry = {
  file: GitDiffFile
  name: string
  directory: string
  added: number
  deleted: number
  status: 'added' | 'deleted' | 'modified' | 'renamed'
}

function DiffCell({ side, cell }: { side: 'old' | 'next'; cell: GitDiffCell }) {
  const marker = cell.tone === 'deleted' ? '−' : cell.tone === 'added' ? '+' : ' '
  return (
    <div
      className={`git-diff-cell grid min-w-0 min-h-[24px] grid-cols-[44px_20px_minmax(0,1fr)] border-b border-stroke-soft font-mono text-[12px] leading-[1.6] tabular-nums [&.next]:border-l [&.next]:border-l-stroke [&.deleted]:bg-[var(--danger-soft)] [&.added]:bg-[var(--success-soft)] [&.deleted]:shadow-[inset_2px_0_var(--danger)] [&.added]:shadow-[inset_2px_0_var(--success)] [&.empty]:bg-surface-subtle [&.context]:bg-code [&_code]:block [&_code]:min-w-0 [&_code]:p-[3px_8px_3px_2px] [&_code]:text-[var(--text)] [&_code]:font-[inherit] [&_code]:whitespace-pre-wrap [&_code]:[overflow-wrap:anywhere] ${side}    ${cell.tone}`}
    >
      <span className="block border-r border-stroke-soft p-[3px_8px_3px_4px] text-right text-code-gutter select-none">
        {cell.lineNumber ?? ''}
      </span>
      <span
        className="git-diff-line-marker [.git-diff-cell.deleted_&]:text-[var(--danger)] [.git-diff-cell.added_&]:text-[var(--success)] block [padding:3px_4px] text-[var(--text-muted)] text-center select-none"
        aria-hidden="true"
      >
        {cell.tone === 'empty' ? '' : marker}
      </span>
      <code>{cell.text || '\u00a0'}</code>
    </div>
  )
}

type VirtualDiffItem =
  | { kind: 'header'; hunkIndex: number; header: string }
  | { kind: 'row'; hunkIndex: number; rowIndex: number; row: GitDiffRow }

function VirtualDiffRows({
  file,
  scrollElement,
}: {
  file: GitDiffFile
  scrollElement: HTMLDivElement | null
}) {
  const listRef = useRef<HTMLDivElement>(null)
  const [scrollMargin, setScrollMargin] = useState(0)
  const items = useMemo<VirtualDiffItem[]>(
    () =>
      file.hunks.flatMap((hunk, hunkIndex) => [
        { kind: 'header' as const, hunkIndex, header: hunk.header },
        ...hunk.rows.map((row, rowIndex) => ({
          kind: 'row' as const,
          hunkIndex,
          rowIndex,
          row,
        })),
      ]),
    [file.hunks],
  )

  useLayoutEffect(() => {
    const listElement = listRef.current
    if (!scrollElement || !listElement) return undefined
    const measure = () => {
      const next = Math.max(
        0,
        listElement.getBoundingClientRect().top -
          scrollElement.getBoundingClientRect().top +
          scrollElement.scrollTop,
      )
      setScrollMargin((current) => (Math.abs(current - next) < 1 ? current : next))
    }
    measure()
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measure)
    observer?.observe(scrollElement)
    observer?.observe(listElement)
    return () => observer?.disconnect()
  }, [scrollElement])

  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    useFlushSync: false,
    count: items.length,
    getScrollElement: () => scrollElement,
    estimateSize: () => 25,
    getItemKey: (index) => {
      const item = items[index]
      return item.kind === 'header'
        ? `hunk-${item.hunkIndex}`
        : `row-${item.hunkIndex}-${item.rowIndex}`
    },
    measureElement: (element) => Math.ceil(element.getBoundingClientRect().height),
    overscan: 12,
    scrollMargin,
  })
  const virtualItems = virtualizer.getVirtualItems()

  return (
    <div
      className="relative min-w-[880px] w-full"
      data-pisper-diff-row-count={items.length}
      data-pisper-rendered-count={virtualItems.length}
      ref={listRef}
      style={{ height: `${virtualizer.getTotalSize()}px` }}
    >
      {virtualItems.map((virtualItem) => {
        const item = items[virtualItem.index]
        return (
          <div
            className="absolute left-0 w-full"
            data-index={virtualItem.index}
            key={virtualItem.key}
            ref={virtualizer.measureElement}
            style={{ top: `${virtualItem.start - scrollMargin}px` }}
          >
            {item.kind === 'header' ? (
              <div className="git-diff-hunk-header border-b border-stroke-soft bg-surface-subtle p-[4px_10px] font-mono text-[12px] text-content-muted [white-space:pre]">
                {item.header}
              </div>
            ) : item.row.kind === 'meta' ? (
              <div className="git-diff-row-meta border-b border-stroke-soft bg-surface-subtle p-[4px_10px] font-mono text-[12px] text-content-muted [white-space:pre]">
                {item.row.text}
              </div>
            ) : (
              <div className="grid min-w-[880px] grid-cols-[minmax(440px,1fr)_minmax(440px,1fr)]">
                <DiffCell side="old" cell={item.row.old} />
                <DiffCell side="next" cell={item.row.next} />
              </div>
            )}
          </div>
        )
      })}
    </div>
  )
}

function fileEntry(file: GitDiffFile): DiffFileEntry {
  const normalizedPath = file.path.replace(/\\/g, '/')
  const parts = normalizedPath.split('/')
  const name = parts.pop() || normalizedPath
  let added = 0
  let deleted = 0

  for (const hunk of file.hunks) {
    for (const row of hunk.rows) {
      if (row.kind !== 'pair') continue
      if (row.next.tone === 'added') added += 1
      if (row.old.tone === 'deleted') deleted += 1
    }
  }

  const metadata = file.metadata.join('\n')
  const status =
    /(?:^|\n)new file mode /.test(metadata) || (!file.oldPath && Boolean(file.newPath))
      ? 'added'
      : /(?:^|\n)deleted file mode /.test(metadata) || (!file.newPath && Boolean(file.oldPath))
        ? 'deleted'
        : /(?:^|\n)rename (?:from|to) /.test(metadata) ||
            Boolean(file.oldPath && file.newPath && file.oldPath !== file.newPath)
          ? 'renamed'
          : 'modified'

  return {
    file,
    name,
    directory: parts.join('/'),
    added,
    deleted,
    status,
  }
}

function statusLetter(status: DiffFileEntry['status']) {
  if (status === 'added') return 'A'
  if (status === 'deleted') return 'D'
  if (status === 'renamed') return 'R'
  return 'M'
}

export function GitDiffDialog({
  diff,
  truncated,
  onClose,
}: {
  diff: string
  truncated?: boolean
  onClose: () => void
}) {
  const { t } = useI18n()
  const files = useMemo(() => parseUnifiedDiff(diff), [diff])
  const entries = useMemo(() => files.map(fileEntry), [files])
  const [requestedFileIndex, setRequestedFileIndex] = useState(0)
  const [contentElement, setContentElement] = useState<HTMLDivElement | null>(null)
  const selectedFileIndex = Math.min(requestedFileIndex, Math.max(entries.length - 1, 0))
  const selectedEntry = entries[selectedFileIndex]

  useEffect(() => {
    const previousOverflow = document.body.style.overflow
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !event.defaultPrevented) {
        event.preventDefault()
        event.stopPropagation()
        onClose()
      }
    }
    document.body.style.overflow = 'hidden'
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.body.style.overflow = previousOverflow
      document.removeEventListener('keydown', onKeyDown)
    }
  }, [onClose])

  const selectFile = (index: number) => {
    setRequestedFileIndex(index)
    contentElement?.scrollTo({ top: 0, left: 0 })
  }

  return createPortal(
    <div
      className="git-diff-dialog-backdrop fixed inset-0 z-[120] grid place-items-center bg-[var(--overlay-scrim)] p-[24px] max-[760px]:p-[10px]"
      role="presentation"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section
        className="git-diff-dialog max-[760px]:w-[calc(100vw_-_20px)] max-[760px]:h-[calc(100vh_-_20px)] flex w-[min(1180px,calc(100vw_-_48px))] h-[min(820px,calc(100vh_-_48px))] min-w-0 min-h-0 flex-col overflow-hidden rounded-xl border border-stroke bg-solid shadow-float"
        role="dialog"
        aria-modal="true"
        aria-label={t('chat:focusSession.gitViewDiff')}
      >
        <header className="git-diff-dialog-head [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[2px] [&_strong]:text-[13px] [&_strong]:font-medium [&_small]:text-content-muted [&_small]:text-[12px] [&_small]:tabular-nums grid min-h-[52px] flex-none grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[10px] border-b border-stroke p-[8px_12px]">
          <span className="grid size-[30px] place-items-center rounded-sm border border-key-border bg-key text-content-secondary shadow-key">
            <FileDiff size={16} />
          </span>
          <span>
            <strong>{t('chat:focusSession.gitViewDiff')}</strong>
            <small>{t('chat:focusSession.gitDiffFileCount', { count: files.length })}</small>
          </span>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            title={t('chat:focusSession.gitCloseDiff')}
            aria-label={t('chat:focusSession.gitCloseDiff')}
            onClick={onClose}
          >
            <X size={17} />
          </Button>
        </header>

        {!entries.length ? (
          <div className="min-h-0 min-w-0 flex-1 overflow-auto bg-surface-subtle p-[12px]">
            <div className="rounded-md border border-stroke bg-panel p-[20px] whitespace-pre-wrap text-content-muted">
              {diff || t('chat:focusSession.gitDiffUnavailable')}
            </div>
          </div>
        ) : (
          <div className="git-diff-layout max-[760px]:grid-rows-[minmax(112px,26vh)_minmax(0,1fr)] max-[760px]:grid-cols-[minmax(0,1fr)] grid min-w-0 min-h-0 flex-1 grid-cols-[252px_minmax(0,1fr)] overflow-hidden">
            <nav
              className="git-diff-file-nav flex min-h-0 min-w-0 flex-col overflow-hidden border-r border-stroke bg-chrome max-[760px]:border-r-0 max-[760px]:border-b"
              aria-label={t('chat:focusSession.gitDiffChangedFiles')}
            >
              <strong className="git-diff-file-nav-title flex-none p-[10px_12px_6px] text-[12px] font-medium tracking-[.02em] text-content-muted max-[760px]:[padding-block:7px_5px]">
                {t('chat:focusSession.gitDiffChangedFiles')}
              </strong>
              <div className="git-diff-file-nav-list max-[760px]:pb-[6px] min-h-0 flex-1 overflow-x-hidden overflow-y-auto [padding:0_6px_8px]">
                {entries.map((entry, index) => {
                  const selected = index === selectedFileIndex
                  return (
                    <button
                      type="button"
                      className={`git-diff-file-nav-item grid min-h-[42px] w-full cursor-pointer grid-cols-[20px_minmax(0,1fr)_auto] items-center gap-[7px] rounded-sm border-0 bg-transparent p-[5px_7px] text-left text-content-secondary transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-foreground [&.selected]:bg-key [&.selected]:text-foreground [&.selected]:shadow-[inset_3px_0_var(--star),var(--key-shadow)] ${entry.status}    ${selected ? 'selected' : ''}`}
                      key={`${entry.file.path}-${index}`}
                      title={entry.file.path}
                      aria-current={selected ? 'true' : undefined}
                      onClick={() => selectFile(index)}
                    >
                      <span
                        className="git-diff-file-status text-center font-mono text-[12px] font-medium [.git-diff-file-nav-item.added_&]:text-[var(--success)] [.git-diff-file-nav-item.deleted_&]:text-[var(--danger)] [.git-diff-file-nav-item.renamed_&]:text-[var(--warning-strong)] [.git-diff-file-nav-item.modified_&]:text-content-secondary"
                        aria-hidden="true"
                      >
                        {statusLetter(entry.status)}
                      </span>
                      <span className="git-diff-file-name [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[12px] [&_strong]:font-medium [&_small]:text-content-muted [&_small]:text-[12px] flex min-w-0 flex-col gap-[1px]">
                        <strong>{entry.name}</strong>
                        {entry.directory && <small>{entry.directory}</small>}
                      </span>
                      <span
                        className="git-diff-file-stats inline-flex items-center gap-[4px] font-mono text-[12px] tabular-nums [&_em]:[font-style:normal]"
                        aria-hidden="true"
                      >
                        {entry.added > 0 && (
                          <em className="added [.git-diff-file-stats_&]:text-[var(--success)]">
                            +{entry.added}
                          </em>
                        )}
                        {entry.deleted > 0 && (
                          <em className="deleted [.git-diff-file-stats_&]:text-[var(--danger)]">
                            −{entry.deleted}
                          </em>
                        )}
                      </span>
                    </button>
                  )
                })}
              </div>
            </nav>

            <div
              className="min-w-0 min-h-0 overflow-auto bg-surface-subtle p-[12px]"
              ref={setContentElement}
            >
              {truncated && (
                <div className="mb-[12px] rounded-sm border border-[var(--warning-border)] bg-warning-soft p-[8px_10px] text-[12px] text-warning-strong">
                  {t('chat:focusSession.gitDiffTruncated')}
                </div>
              )}
              {selectedEntry && (
                <section
                  className="min-w-[880px] [overflow:clip] rounded-md border border-stroke bg-panel"
                  key={selectedEntry.file.path}
                >
                  <header className="git-diff-file-head [&_>_svg]:text-content-muted [&_strong]:overflow-hidden [&_strong]:font-mono [&_strong]:text-[12px] [&_strong]:font-medium [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:max-w-[300px] [&_small]:overflow-hidden [&_small]:text-content-muted [&_small]:text-[12px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap sticky top-0 z-[3] grid min-h-[38px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[8px] border-b border-stroke bg-panel p-[7px_10px]">
                    <FileDiff size={14} />
                    <strong title={selectedEntry.file.path}>{selectedEntry.file.path}</strong>
                    {selectedEntry.file.oldPath &&
                      selectedEntry.file.newPath &&
                      selectedEntry.file.oldPath !== selectedEntry.file.newPath && (
                        <small title={selectedEntry.file.oldPath}>
                          {selectedEntry.file.oldPath}
                        </small>
                      )}
                  </header>
                  {selectedEntry.file.metadata.length > 0 && (
                    <div className="git-diff-file-meta border-b border-stroke-soft bg-surface-subtle p-[5px_10px] font-mono text-[12px] text-content-muted">
                      {selectedEntry.file.metadata.join(' · ')}
                    </div>
                  )}
                  {selectedEntry.file.hunks.length > 0 ? (
                    <>
                      <div className="git-diff-column-head [&_span]:p-[5px_10px] [&_span_+_span]:border-l [&_span_+_span]:border-stroke sticky top-[38px] z-[2] grid grid-cols-[1fr_1fr] border-b border-stroke bg-surface-subtle text-[12px] font-medium text-content-muted">
                        <span>{t('chat:focusSession.gitDiffOriginal')}</span>
                        <span>{t('chat:focusSession.gitDiffModified')}</span>
                      </div>
                      <VirtualDiffRows file={selectedEntry.file} scrollElement={contentElement} />
                    </>
                  ) : (
                    <div className="[padding:28px_16px] text-[var(--text-muted)] text-[12px] text-center">
                      {t('chat:focusSession.gitDiffNoTextChanges')}
                    </div>
                  )}
                </section>
              )}
            </div>
          </div>
        )}
      </section>
    </div>,
    document.body,
  )
}

// 内嵌差异复用相同解析与虚拟行；每个文件有独立的有界滚动容器。
export function InlineGitDiff({
  diff,
  truncated,
  position,
  onReading = () => {},
}: {
  diff: string
  truncated?: boolean
  position?: ResourceReadingPosition
  onReading?: (position: ResourceReadingPosition) => void
}) {
  const { t } = useI18n()
  const files = useMemo(() => parseUnifiedDiff(diff), [diff])
  const { element: scrollElement, ref: setScrollElement } = useResourceReading<HTMLDivElement>(
    position,
    onReading,
  )
  return (
    <div>
      {truncated && (
        <p className="px-3 py-2 text-xs text-content-muted">
          {t('chat:chatMessage.filePreviewTruncated')}
        </p>
      )}
      <div
        ref={setScrollElement}
        tabIndex={0}
        className="max-h-80 min-w-0 overflow-auto border-y border-border bg-background"
      >
        {!files.length || files.every((file) => !file.hunks.length) ? (
          <p className="p-3 text-xs text-content-muted">
            {t('chat:focusSession.gitDiffNoTextChanges')}
          </p>
        ) : (
          files.map((file, index) => (
            <VirtualDiffRows
              key={`${file.path}-${index}`}
              file={file}
              scrollElement={scrollElement}
            />
          ))
        )}
      </div>
    </div>
  )
}
