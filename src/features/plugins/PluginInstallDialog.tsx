// 插件安装对话框：从本地目录安装插件（目录选择 + 校验 + 安装进度）。
import { useEffect, useState } from 'react'
import { AlertTriangle, CheckCircle2, FolderOpen, PackagePlus } from 'lucide-react'
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
import { pickSystemDirectory } from '@/lib/pick-system-directory'
import type { PluginInspection } from '@/features/plugins/plugin-types'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'

import { AppError } from '@/components/ui/app-primitives'

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

  // 选择插件目录（桌面系统选择器），选中后清空旧的检查结果。
  const chooseDirectory = async () => {
    setError('')
    try {
      const result = await pickSystemDirectory(path, t('plugins:pluginsPage.chooseDirectory'))
      if (result.status === 'picked') {
        setPath(result.path)
        setInspection(null)
      } else if (result.status === 'unavailable') {
        setError(t('common:workspacePicker.systemPickerUnavailable'))
      }
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  // 检查插件目录：调运行时解析清单与能力，校验通过才能安装。
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

  // 安装插件：用检查结果 id 提交安装，完成后刷新并关闭对话框。
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
            <PackagePlus className="size-4 text-content-secondary" />
            {t('plugins:pluginsPage.installLocalPlugin')}
          </DialogTitle>
          <DialogDescription>
            {t('plugins:pluginsPage.installLocalPluginDescription')}
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-3">
          <label className="grid gap-1.5 text-xs font-medium text-content-secondary">
            {t('plugins:pluginsPage.pluginDirectory')}
            <span className="flex min-w-0 gap-2">
              <Input
                className="min-w-0 flex-1 font-mono text-xs max-[650px]:h-11"
                value={path}
                disabled={busy}
                placeholder={t('plugins:pluginsPage.pluginDirectoryPlaceholder')}
                onChange={(event) => {
                  setPath(event.target.value)
                  setInspection(null)
                }}
              />
              <Button
                type="button"
                variant="secondary"
                size="icon"
                className="size-[34px] shrink-0 max-[650px]:size-11"
                title={t('plugins:pluginsPage.chooseDirectory')}
                aria-label={t('plugins:pluginsPage.chooseDirectory')}
                onClick={chooseDirectory}
              >
                <FolderOpen />
              </Button>
            </span>
          </label>

          {inspection && (
            <div className="grid gap-3 overflow-hidden rounded-md border border-stroke bg-surface-subtle p-3">
              <div className="flex min-w-0 items-start gap-3">
                <span className="grid size-8 shrink-0 place-items-center rounded-sm bg-success-soft text-success">
                  <CheckCircle2 className="size-4" />
                </span>
                <span className="min-w-0 flex-1">
                  <strong className="block truncate text-sm font-medium text-content">
                    {inspection.plugin.name}
                  </strong>
                  <small className="block truncate font-mono text-xs text-content-muted tabular-nums">
                    {inspection.plugin.id} · v{inspection.plugin.version} · {inspection.fileCount}{' '}
                    {t('plugins:pluginsPage.files')} · {formatBytes(inspection.byteCount)}
                  </small>
                </span>
              </div>
              <div className="grid gap-1.5">
                {inspection.plugin.capabilities.map((capability) => (
                  <div
                    key={capability.name}
                    className="flex items-start justify-between gap-3 border-t border-stroke-soft pt-2 text-xs"
                  >
                    <span className="min-w-0">
                      <strong className="block font-medium text-content">{capability.label}</strong>
                      <small className="text-xs text-content-muted">{capability.description}</small>
                    </span>
                    <code className="shrink-0 font-mono text-xs text-content-secondary">
                      {capability.name}
                    </code>
                  </div>
                ))}
              </div>
              <div className="flex items-start gap-2 rounded-sm bg-warning-soft p-2.5 text-xs leading-5 text-warning-strong">
                <AlertTriangle className="mt-0.5 shrink-0" size={15} />
                <span>{t('plugins:pluginsPage.localPluginSecurityWarning')}</span>
              </div>
            </div>
          )}

          {error && (
            <AppError className="m-0">
              <AlertTriangle size={14} />
              <span>{error}</span>
            </AppError>
          )}
        </div>

        <DialogFooter>
          <Button
            type="button"
            variant="secondary"
            size="dialogAction"
            disabled={busy}
            onClick={() => onOpenChange(false)}
          >
            {t('common:ui.cancel')}
          </Button>
          <Button
            type="button"
            size="dialogAction"
            disabled={busy || (!inspection && !path.trim())}
            onClick={inspection ? install : inspect}
          >
            {busy ? (
              <OrbitSpinner className="text-current" />
            ) : inspection ? (
              <PackagePlus />
            ) : (
              <CheckCircle2 />
            )}
            {busy
              ? t('plugins:pluginsPage.processing')
              : inspection
                ? t('plugins:pluginsPage.installPlugin')
                : t('plugins:pluginsPage.inspectPlugin')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
