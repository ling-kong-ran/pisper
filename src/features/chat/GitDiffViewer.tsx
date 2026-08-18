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
      className={`git-diff-cell grid min-w-0 min-h-[24px] grid-cols-[44px_20px_minmax(0,1fr)] [border-bottom:1px_solid_var(--stroke-soft)] font-[ui-monospace,SFMono-Regular,Consolas,'Liberation_Mono',monospace] text-[11px] leading-[1.55] [&.next]:[border-left:1px_solid_var(--stroke)] [&.deleted]:bg-[var(--danger-soft)] [&.added]:bg-[var(--success-soft)] [&.empty]:bg-[var(--surface-muted)] [&.context]:bg-[var(--solid)] [&_code]:block [&_code]:min-w-0 [&_code]:p-[3px_8px_3px_2px] [&_code]:text-[var(--text)] [&_code]:font-[inherit] [&_code]:whitespace-pre-wrap [&_code]:[overflow-wrap:anywhere] ${side}    ${cell.tone}`}
    >
      <span className="block [padding:3px_7px_3px_4px] [border-right:1px_solid_var(--stroke-soft)] text-[var(--text-muted)] text-right select-none">
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
              <div className="git-diff-hunk-header [border-bottom:1px_solid_var(--stroke-soft)] bg-[var(--blue-soft)] p-[4px_10px] text-[var(--brand-blue-strong)] font-[ui-monospace,SFMono-Regular,Consolas,'Liberation_Mono',monospace] text-[10px] [white-space:pre]">
                {item.header}
              </div>
            ) : item.row.kind === 'meta' ? (
              <div className="git-diff-row-meta [border-bottom:1px_solid_var(--stroke-soft)] bg-[var(--blue-soft)] p-[4px_10px] text-[var(--brand-blue-strong)] font-[ui-monospace,SFMono-Regular,Consolas,'Liberation_Mono',monospace] text-[10px] [white-space:pre] bg-[var(--surface-muted)] text-[var(--text-muted)]">
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
      if (event.key === 'Escape') onClose()
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
      className="git-diff-dialog-backdrop max-[760px]:p-[10px] fixed z-[120] inset-0 grid place-items-center bg-[rgb(0_0_0/.48)] [padding:24px] [backdrop-filter:blur(3px)]"
      role="presentation"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section
        className="git-diff-dialog max-[760px]:w-[calc(100vw_-_20px)] max-[760px]:h-[calc(100vh_-_20px)] flex w-[min(1180px,calc(100vw_-_48px))] h-[min(820px,calc(100vh_-_48px))] min-w-0 min-h-0 flex-col overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-lg)] bg-[var(--solid)] shadow-[0_28px_90px_-24px_rgb(0_0_0/.65)]"
        role="dialog"
        aria-modal="true"
        aria-label={t('chat:focusSession.gitViewDiff')}
      >
        <header className="git-diff-dialog-head [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[2px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] grid min-h-[54px] flex-none grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[10px] [border-bottom:1px_solid_var(--stroke)] [padding:8px_12px]">
          <span className="grid w-[34px] h-[34px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)]">
            <FileDiff size={17} />
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
          <div className="min-w-0 min-h-0 flex-1 bg-[var(--surface-subtle)] overflow-auto [padding:12px]">
            <div className="[border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--solid)] [padding:20px] text-[var(--text-muted)] whitespace-pre-wrap">
              {diff || t('chat:focusSession.gitDiffUnavailable')}
            </div>
          </div>
        ) : (
          <div className="git-diff-workbench max-[760px]:grid-rows-[minmax(112px,26vh)_minmax(0,1fr)] max-[760px]:grid-cols-[minmax(0,1fr)] grid min-w-0 min-h-0 flex-1 grid-cols-[252px_minmax(0,1fr)] overflow-hidden">
            <nav
              className="git-diff-file-nav max-[760px]:[border-right:0] max-[760px]:[border-bottom:1px_solid_var(--stroke)] flex min-w-0 min-h-0 flex-col overflow-hidden [border-right:1px_solid_var(--stroke)] bg-[var(--surface-muted)]"
              aria-label={t('chat:focusSession.gitDiffChangedFiles')}
            >
              <strong className="git-diff-file-nav-title max-[760px]:[padding-block:7px_5px] flex-none [padding:10px_12px_8px] text-[var(--text-muted)] text-[10px] font-[700] tracking-[.055em] [text-transform:uppercase]">
                {t('chat:focusSession.gitDiffChangedFiles')}
              </strong>
              <div className="git-diff-file-nav-list max-[760px]:pb-[6px] min-h-0 flex-1 overflow-x-hidden overflow-y-auto [padding:0_6px_8px]">
                {entries.map((entry, index) => {
                  const selected = index === selectedFileIndex
                  return (
                    <button
                      type="button"
                      className={`git-diff-file-nav-item hover:bg-[var(--surface-hover)] hover:text-[var(--text)] [&.selected]:bg-[var(--accent-soft)] [&.selected]:text-[var(--text)] [&.selected]:shadow-[inset_2px_0_var(--star-strong)] grid w-full min-h-[42px] grid-cols-[20px_minmax(0,1fr)_auto] items-center gap-[7px] border-0 rounded-[var(--r-sm)] bg-transparent [padding:5px_7px] text-[var(--text-secondary)] text-left cursor-pointer ${entry.status}    ${selected ? 'selected' : ''}`}
                      key={`${entry.file.path}-${index}`}
                      title={entry.file.path}
                      aria-current={selected ? 'true' : undefined}
                      onClick={() => selectFile(index)}
                    >
                      <span
                        className="git-diff-file-status font-[ui-monospace,SFMono-Regular,Consolas,'Liberation_Mono',monospace] text-[11px] font-[750] text-center [.git-diff-file-nav-item.added_&]:text-[var(--success)] [.git-diff-file-nav-item.deleted_&]:text-[var(--danger)] [.git-diff-file-nav-item.renamed_&]:text-[var(--warning-strong)] [.git-diff-file-nav-item.modified_&]:text-[var(--star-strong)]"
                        aria-hidden="true"
                      >
                        {statusLetter(entry.status)}
                      </span>
                      <span className="git-diff-file-name [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[11px] [&_strong]:font-[650] [&_small]:text-[var(--text-muted)] [&_small]:text-[9px] flex min-w-0 flex-col gap-[1px]">
                        <strong>{entry.name}</strong>
                        {entry.directory && <small>{entry.directory}</small>}
                      </span>
                      <span
                        className="git-diff-file-stats inline-flex items-center gap-[4px] font-[ui-monospace,SFMono-Regular,Consolas,'Liberation_Mono',monospace] text-[9px] [&_em]:[font-style:normal]"
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
              className="min-w-0 min-h-0 overflow-auto bg-[var(--surface-subtle)] [padding:12px]"
              ref={setContentElement}
            >
              {truncated && (
                <div className="[margin:0_0_12px] [border:1px_solid_var(--warning-border)] rounded-[var(--r-sm)] bg-[var(--warning-soft)] [padding:8px_10px] text-[var(--warning-strong)] text-[11px]">
                  {t('chat:focusSession.gitDiffTruncated')}
                </div>
              )}
              {selectedEntry && (
                <section
                  className="min-w-[880px] [overflow:clip] [border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--solid)]"
                  key={selectedEntry.file.path}
                >
                  <header className="git-diff-file-head [&_>_svg]:text-[var(--star-strong)] [&_strong]:overflow-hidden [&_strong]:font-[ui-monospace,SFMono-Regular,Consolas,'Liberation_Mono',monospace] [&_strong]:text-[12px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:max-w-[300px] [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[10px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap sticky z-[3] top-0 grid min-h-[38px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[8px] [border-bottom:1px_solid_var(--stroke)] bg-[var(--solid)] [padding:7px_10px]">
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
                    <div className="git-diff-file-meta [border-bottom:1px_solid_var(--stroke-soft)] bg-[var(--surface-muted)] p-[5px_10px] text-[var(--text-muted)] font-[ui-monospace,SFMono-Regular,Consolas,'Liberation_Mono',monospace] text-[10px]">
                      {selectedEntry.file.metadata.join(' · ')}
                    </div>
                  )}
                  {selectedEntry.file.hunks.length > 0 ? (
                    <>
                      <div className="git-diff-column-head [&_span]:p-[5px_10px] [&_span_+_span]:[border-left:1px_solid_var(--stroke)] sticky z-[2] [top:38px] grid grid-cols-[1fr_1fr] [border-bottom:1px_solid_var(--stroke)] bg-[var(--surface-muted)] text-[var(--text-muted)] text-[10px] font-[650] [text-transform:uppercase]">
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
