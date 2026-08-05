import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useVirtualizer } from '@tanstack/react-virtual'
import { FileDiff, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { parseUnifiedDiff, type GitDiffCell, type GitDiffFile, type GitDiffRow } from './git-diff'

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
    <div className={`git-diff-cell ${side} ${cell.tone}`}>
      <span className="git-diff-line-number">{cell.lineNumber ?? ''}</span>
      <span className="git-diff-line-marker" aria-hidden="true">
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
      className="git-diff-virtual-list"
      data-pisper-diff-row-count={items.length}
      data-pisper-rendered-count={virtualItems.length}
      ref={listRef}
      style={{ height: `${virtualizer.getTotalSize()}px` }}
    >
      {virtualItems.map((virtualItem) => {
        const item = items[virtualItem.index]
        return (
          <div
            className="git-diff-virtual-item"
            data-index={virtualItem.index}
            key={virtualItem.key}
            ref={virtualizer.measureElement}
            style={{ top: `${virtualItem.start - scrollMargin}px` }}
          >
            {item.kind === 'header' ? (
              <div className="git-diff-hunk-header">{item.header}</div>
            ) : item.row.kind === 'meta' ? (
              <div className="git-diff-row-meta">{item.row.text}</div>
            ) : (
              <div className="git-diff-row">
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
      className="git-diff-dialog-backdrop"
      role="presentation"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section
        className="git-diff-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t('chat:focusSession.gitViewDiff')}
      >
        <header className="git-diff-dialog-head">
          <span className="git-diff-dialog-icon">
            <FileDiff size={17} />
          </span>
          <span>
            <strong>{t('chat:focusSession.gitViewDiff')}</strong>
            <small>{t('chat:focusSession.gitDiffFileCount', { count: files.length })}</small>
          </span>
          <button
            type="button"
            className="icon-button"
            title={t('chat:focusSession.gitCloseDiff')}
            aria-label={t('chat:focusSession.gitCloseDiff')}
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </header>

        {!entries.length ? (
          <div className="git-diff-dialog-body git-diff-dialog-empty-body">
            <div className="git-diff-dialog-empty">
              {diff || t('chat:focusSession.gitDiffUnavailable')}
            </div>
          </div>
        ) : (
          <div className="git-diff-workbench">
            <nav
              className="git-diff-file-nav"
              aria-label={t('chat:focusSession.gitDiffChangedFiles')}
            >
              <strong className="git-diff-file-nav-title">
                {t('chat:focusSession.gitDiffChangedFiles')}
              </strong>
              <div className="git-diff-file-nav-list">
                {entries.map((entry, index) => {
                  const selected = index === selectedFileIndex
                  return (
                    <button
                      type="button"
                      className={`git-diff-file-nav-item ${entry.status} ${selected ? 'selected' : ''}`}
                      key={`${entry.file.path}-${index}`}
                      title={entry.file.path}
                      aria-current={selected ? 'true' : undefined}
                      onClick={() => selectFile(index)}
                    >
                      <span className="git-diff-file-status" aria-hidden="true">
                        {statusLetter(entry.status)}
                      </span>
                      <span className="git-diff-file-name">
                        <strong>{entry.name}</strong>
                        {entry.directory && <small>{entry.directory}</small>}
                      </span>
                      <span className="git-diff-file-stats" aria-hidden="true">
                        {entry.added > 0 && <em className="added">+{entry.added}</em>}
                        {entry.deleted > 0 && <em className="deleted">−{entry.deleted}</em>}
                      </span>
                    </button>
                  )
                })}
              </div>
            </nav>

            <div className="git-diff-content" ref={setContentElement}>
              {truncated && (
                <div className="git-diff-truncated">{t('chat:focusSession.gitDiffTruncated')}</div>
              )}
              {selectedEntry && (
                <section className="git-diff-file" key={selectedEntry.file.path}>
                  <header className="git-diff-file-head">
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
                    <div className="git-diff-file-meta">
                      {selectedEntry.file.metadata.join(' · ')}
                    </div>
                  )}
                  {selectedEntry.file.hunks.length > 0 ? (
                    <>
                      <div className="git-diff-column-head">
                        <span>{t('chat:focusSession.gitDiffOriginal')}</span>
                        <span>{t('chat:focusSession.gitDiffModified')}</span>
                      </div>
                      <VirtualDiffRows file={selectedEntry.file} scrollElement={contentElement} />
                    </>
                  ) : (
                    <div className="git-diff-no-text">
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
