// 回收站（规格 5.9，A8）：删除的组件先移进回收站，保留 7 天，可以 [恢复]（目录、存储、状态与放置一起回来）
// 或 [永久删除]（先确认）。启动迁移移进来的旧 iframe 组件也列在这里：它们不能恢复（旧机制已删除），
// 也不会被 7 天清理自动删掉（回滚旧版本前可能要手动移回），只能由你永久删除。
import { useState } from 'react'
import { Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { relativeTime } from '@/lib/format'
import type { ComponentRow } from '@/features/custom-ui/custom-ui-api'
import type { TrashEntry } from '@/features/custom-ui/lifecycle-api'
import { useComponentTrash, type LifecycleActions } from './use-lifecycle'

const DAY = 24 * 60 * 60 * 1000

export function TrashButton({
  enabled,
  lifecycle,
  onRestored,
  className,
}: {
  enabled: boolean
  lifecycle: LifecycleActions
  onRestored?: (row: ComponentRow) => void
  className?: string
}) {
  const { t, language } = useI18n()
  const trash = useComponentTrash(enabled)
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState('')
  const [confirming, setConfirming] = useState<TrashEntry | null>(null)
  if (!enabled) return null
  const entries = trash.data?.entries ?? []
  const restore = async (entry: TrashEntry) => {
    setBusy(entry.entry)
    const result = await lifecycle.restoreTrash(entry)
    setBusy('')
    if (result) {
      setOpen(false)
      onRestored?.(result.component)
    }
  }
  const purge = async () => {
    if (!confirming) return
    setBusy(confirming.entry)
    const result = await lifecycle.purgeTrash(confirming)
    setBusy('')
    if (result) setConfirming(null)
  }
  const expiry = (entry: TrashEntry) => {
    if (!entry.expiresAt) return t('custom-ui:trash.keptLegacy')
    const days = Math.max(0, Math.ceil((Date.parse(entry.expiresAt) - Date.now()) / DAY))
    return t('custom-ui:trash.expiresIn', { count: days })
  }
  return (
    <>
      <Button
        size="xs"
        variant="ghost"
        data-component-trash
        className={className}
        onClick={() => setOpen(true)}
      >
        <Trash2 />
        {t('custom-ui:trash.button')}
        {entries.length > 0 && <span className="tabular-nums">{entries.length}</span>}
      </Button>
      <Dialog open={open} onOpenChange={(next) => !busy && setOpen(next)}>
        <DialogContent size="wide" scrollMode="body" data-trash-dialog>
          <DialogHeader>
            <DialogTitle>{t('custom-ui:trash.title')}</DialogTitle>
            <DialogDescription>
              {t('custom-ui:trash.hint', { days: trash.data?.retentionDays ?? 7 })}
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="-mx-4 border-t border-stroke">
            {trash.isPending ? (
              <p className="p-4 text-sm text-content-muted">{t('custom-ui:trash.loading')}</p>
            ) : trash.error ? (
              <p className="p-4 text-sm text-content-muted">{t('custom-ui:trash.failed')}</p>
            ) : entries.length ? (
              <ul className="flex flex-col">
                {entries.map((entry) => (
                  <li
                    key={entry.entry}
                    data-trash-entry={entry.entry}
                    className="flex min-w-0 items-center gap-3 border-b border-stroke px-4 py-2.5 last:border-b-0"
                  >
                    <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                      <span className="flex min-w-0 items-baseline gap-1.5 text-sm">
                        <strong className="min-w-0 truncate font-medium">{entry.name}</strong>
                        {entry.version && (
                          <span className="flex-none font-mono text-xs text-content-muted">
                            v{entry.version}
                          </span>
                        )}
                      </span>
                      <span className="truncate text-xs text-content-muted">
                        {[
                          entry.reason === 'legacy-retired'
                            ? t('custom-ui:trash.legacyRetired')
                            : t('custom-ui:trash.deletedAt', {
                                time: relativeTime(entry.deletedAt, language),
                              }),
                          entry.instances
                            ? t('custom-ui:trash.instances', { count: entry.instances })
                            : '',
                          expiry(entry),
                        ]
                          .filter(Boolean)
                          .join(' · ')}
                      </span>
                    </span>
                    {entry.kind === 'native' && (
                      <Button
                        size="xs"
                        variant="secondary"
                        disabled={Boolean(busy)}
                        data-trash-restore={entry.entry}
                        onClick={() => void restore(entry)}
                      >
                        {t('custom-ui:trash.restore')}
                      </Button>
                    )}
                    <Button
                      size="xs"
                      variant="ghost"
                      className="text-danger"
                      disabled={Boolean(busy)}
                      data-trash-purge={entry.entry}
                      onClick={() => setConfirming(entry)}
                    >
                      {t('custom-ui:trash.purge')}
                    </Button>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="p-4 text-sm text-content-muted">{t('custom-ui:trash.empty')}</p>
            )}
          </DialogBody>
        </DialogContent>
      </Dialog>
      <Dialog
        open={Boolean(confirming)}
        onOpenChange={(next) => !next && !busy && setConfirming(null)}
      >
        <DialogContent size="compact" surface="confirmation">
          <DialogHeader>
            <DialogTitle>
              {t('custom-ui:trash.purgeTitle', { name: confirming?.name ?? '' })}
            </DialogTitle>
            <DialogDescription>{t('custom-ui:trash.purgeHint')}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" disabled={Boolean(busy)} onClick={() => setConfirming(null)}>
              {t('custom-ui:studio.cancel')}
            </Button>
            <Button
              variant="destructive"
              disabled={Boolean(busy)}
              data-trash-purge-confirm
              onClick={() => void purge()}
            >
              {t('custom-ui:trash.purgeConfirm')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
