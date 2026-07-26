import { useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { FileDiff, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { parseUnifiedDiff, type GitDiffCell, type GitDiffFile } from './git-diff'

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
  const contentRef = useRef<HTMLDivElement>(null)
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
    contentRef.current?.scrollTo({ top: 0, left: 0 })
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

            <div className="git-diff-content" ref={contentRef}>
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
                      {selectedEntry.file.hunks.map((hunk, hunkIndex) => (
                        <div className="git-diff-hunk" key={`${hunk.header}-${hunkIndex}`}>
                          <div className="git-diff-hunk-header">{hunk.header}</div>
                          {hunk.rows.map((row, rowIndex) =>
                            row.kind === 'meta' ? (
                              <div className="git-diff-row-meta" key={`meta-${rowIndex}`}>
                                {row.text}
                              </div>
                            ) : (
                              <div className="git-diff-row" key={`row-${rowIndex}`}>
                                <DiffCell side="old" cell={row.old} />
                                <DiffCell side="next" cell={row.next} />
                              </div>
                            ),
                          )}
                        </div>
                      ))}
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
