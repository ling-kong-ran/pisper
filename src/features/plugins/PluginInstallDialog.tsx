import { useEffect, useState } from 'react'
import { AlertTriangle, CheckCircle2, FolderOpen, LoaderCircle, PackagePlus } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { apiJson } from '@/lib/api'
import { hasSystemDirectoryPicker, pickSystemDirectory } from '@/lib/pick-system-directory'
import type { PluginInspection } from '@/features/plugins/plugin-types'

function formatBytes(value: number) {
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / 1024 / 1024).toFixed(1)} MB`
}

type PluginInstallDialogProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  onInstalled: () => Promise<void>
}

export function PluginInstallDialog({ open, onOpenChange, onInstalled }: PluginInstallDialogProps) {
  const { t } = useI18n()
  const [path, setPath] = useState('')
  const [inspection, setInspection] = useState<PluginInspection | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => {
    if (open) return
    setPath('')
    setInspection(null)
    setBusy(false)
    setError('')
  }, [open])

  const chooseDirectory = async () => {
    setError('')
    try {
      const selected = await pickSystemDirectory(path)
      if (selected) {
        setPath(selected)
        setInspection(null)
      }
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  const inspect = async () => {
    if (!path.trim() || busy) return
    setBusy(true)
    setError('')
    try {
      setInspection(
        await apiJson<PluginInspection>('/api/plugins/inspect', {
          method: 'POST',
          body: JSON.stringify({ path: path.trim() }),
        }),
      )
    } catch (caught) {
      setInspection(null)
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }

  const install = async () => {
    if (!inspection || busy) return
    setBusy(true)
    setError('')
    try {
      await apiJson('/api/plugins/install', {
        method: 'POST',
        body: JSON.stringify({ inspectionId: inspection.inspectionId }),
      })
      await onInstalled()
      onOpenChange(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <DialogContent className="z-[220] sm:max-w-xl" overlayClassName="z-[220]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <PackagePlus size={17} />
            {t('plugins:pluginsPage.installLocalPlugin')}
          </DialogTitle>
          <DialogDescription>
            {t('plugins:pluginsPage.installLocalPluginDescription')}
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-3">
          <label className="grid gap-1.5 text-xs font-semibold text-[var(--text-soft)]">
            {t('plugins:pluginsPage.pluginDirectory')}
            <span className="flex min-w-0 gap-2">
              <input
                className="h-10 min-w-0 flex-1 rounded-md border border-[var(--stroke)] bg-[var(--solid)] px-3 text-[13px] text-[var(--text)] outline-none focus:border-[var(--star-border)]"
                value={path}
                disabled={busy}
                placeholder={t('plugins:pluginsPage.pluginDirectoryPlaceholder')}
                onChange={(event) => {
                  setPath(event.target.value)
                  setInspection(null)
                }}
              />
              {hasSystemDirectoryPicker() && (
                <button
                  type="button"
                  className="button secondary icon-button size-10 shrink-0"
                  title={t('plugins:pluginsPage.chooseDirectory')}
                  onClick={chooseDirectory}
                >
                  <FolderOpen size={16} />
                </button>
              )}
            </span>
          </label>

          {inspection && (
            <div className="grid gap-3 rounded-md border border-[var(--stroke)] bg-[var(--surface-muted)] p-3">
              <div className="flex min-w-0 items-start gap-3">
                <span className="grid size-9 shrink-0 place-items-center rounded-md bg-[var(--success-soft)] text-[var(--success)]">
                  <CheckCircle2 size={17} />
                </span>
                <span className="min-w-0 flex-1">
                  <strong className="block truncate text-[13px] text-[var(--text)]">
                    {inspection.plugin.name}
                  </strong>
                  <small className="block truncate text-[12px] text-[var(--text-muted)]">
                    {inspection.plugin.id} · v{inspection.plugin.version} · {inspection.fileCount}{' '}
                    {t('plugins:pluginsPage.files')} · {formatBytes(inspection.byteCount)}
                  </small>
                </span>
              </div>
              <div className="grid gap-1.5">
                {inspection.plugin.capabilities.map((capability) => (
                  <div
                    key={capability.name}
                    className="flex items-start justify-between gap-3 border-t border-[var(--stroke-soft)] pt-2 text-xs"
                  >
                    <span>
                      <strong className="block text-[var(--text)]">{capability.label}</strong>
                      <small className="text-[var(--text-muted)]">{capability.description}</small>
                    </span>
                    <code className="shrink-0 text-[11px] text-[var(--text-soft)]">
                      {capability.name}
                    </code>
                  </div>
                ))}
              </div>
              <div className="flex items-start gap-2 rounded-md bg-[var(--warning-soft)] p-2.5 text-xs leading-5 text-[var(--warning-strong)]">
                <AlertTriangle className="mt-0.5 shrink-0" size={15} />
                <span>{t('plugins:pluginsPage.localPluginSecurityWarning')}</span>
              </div>
            </div>
          )}

          {error && (
            <div className="config-error m-0">
              <AlertTriangle size={14} />
              <span>{error}</span>
            </div>
          )}
        </div>

        <DialogFooter>
          <button
            type="button"
            className="button secondary"
            disabled={busy}
            onClick={() => onOpenChange(false)}
          >
            {t('common:ui.cancel')}
          </button>
          <button
            type="button"
            className="button primary"
            disabled={busy || (!inspection && !path.trim())}
            onClick={inspection ? install : inspect}
          >
            {busy ? (
              <LoaderCircle className="spin" size={15} />
            ) : inspection ? (
              <PackagePlus size={15} />
            ) : (
              <CheckCircle2 size={15} />
            )}
            {busy
              ? t('plugins:pluginsPage.processing')
              : inspection
                ? t('plugins:pluginsPage.installPlugin')
                : t('plugins:pluginsPage.inspectPlugin')}
          </button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
