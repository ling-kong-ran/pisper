import { useCallback, useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { AlertTriangle, Check, ChevronRight, FolderOpen, RefreshCw, X } from 'lucide-react'
import { useI18n } from '../app/use-i18n'
import { apiJson } from '../lib/api'
import type { SessionSummary } from '../types/chat'

type DirectoryEntry = { name: string; path: string }
type DirectoryListing = { path: string; parent?: string; directories: DirectoryEntry[] }
type WorkspacePickerProps = {
  session: SessionSummary
  onClose: () => void
  onSelect: (path: string) => void | Promise<void>
}

export function WorkspacePicker({ session, onClose, onSelect }: WorkspacePickerProps) {
  const { t } = useI18n()
  const [path, setPath] = useState(session.cwd || '')
  const [listing, setListing] = useState<DirectoryListing | null>(null)
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')

  const browse = useCallback(async (target: string) => {
    setLoading(true)
    setError('')
    try {
      const data = await apiJson<DirectoryListing>(
        `/api/directories?path=${encodeURIComponent(target || '')}`,
      )
      setPath(data.path)
      setListing(data)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    browse(session.cwd || '')
  }, [browse, session.cwd])

  const choose = async () => {
    setSaving(true)
    setError('')
    try {
      await onSelect(path)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  return createPortal(
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section
        className="modal workspace-modal"
        role="dialog"
        aria-modal="true"
        aria-label={t('common:workspacePicker.setWorkingDirectory')}
      >
        <div className="card-head">
          <div>
            <h2>{t('common:workspacePicker.setWorkingDirectory')}</h2>
            <p>
              {t('common:workspacePicker.toolsAndTheAgentForNameWillRunInThisDirectory', {
                name: session.name,
              })}
            </p>
          </div>
          <button
            className="icon-button"
            aria-label={t('common:workspacePicker.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </div>
        <form
          className="workspace-path-form"
          onSubmit={(event) => {
            event.preventDefault()
            browse(path)
          }}
        >
          <FolderOpen size={15} />
          <input
            value={path}
            onChange={(event) => setPath(event.target.value)}
            placeholder={t('common:workspacePicker.enterTheProjectSAbsolutePath')}
            autoFocus
          />
          <button className="button secondary" disabled={loading}>
            {loading ? <RefreshCw className="spin" size={13} /> : t('common:workspacePicker.go')}
          </button>
        </form>
        <div className="directory-browser">
          {listing?.parent && (
            <button type="button" onClick={() => browse(listing.parent!)}>
              <FolderOpen size={14} />
              <span>..</span>
              <small>{t('common:workspacePicker.parentFolder')}</small>
            </button>
          )}
          {listing?.directories.map((directory) => (
            <button type="button" key={directory.path} onClick={() => browse(directory.path)}>
              <FolderOpen size={14} />
              <span>{directory.name}</span>
              <ChevronRight size={13} />
            </button>
          ))}
          {!loading && listing && !listing.directories.length && (
            <div className="directory-empty">
              {t('common:workspacePicker.thisFolderHasNoSubfolders')}
            </div>
          )}
          {loading && (
            <div className="directory-empty">
              <RefreshCw className="spin" size={16} />
              {t('common:workspacePicker.readingFolder')}
            </div>
          )}
        </div>
        {error && (
          <div className="config-error">
            <AlertTriangle size={13} />
            {error}
          </div>
        )}
        <div className="modal-actions">
          <button type="button" className="button secondary" onClick={onClose}>
            {t('common:workspacePicker.cancel')}
          </button>
          <button
            type="button"
            className="button primary"
            onClick={choose}
            disabled={saving || loading || !path.trim()}
          >
            {saving ? <RefreshCw className="spin" size={14} /> : <Check size={14} />}
            {saving
              ? t('common:workspacePicker.switching')
              : t('common:workspacePicker.useThisFolder')}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  )
}
