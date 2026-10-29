// 系统目录选择器由调用方打开；这里仅收集 Runtime 路径，不枚举宿主文件系统。
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { AlertTriangle, Check, FolderInput } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import { importMobileWorkspaceDirectory, mobileWorkspaceMode } from '@/lib/mobile-workspace'

type WorkspacePickerProps = {
  open: boolean
  initialPath?: string
  description?: string
  onOpenChange: (open: boolean) => void
  onSelect: (path: string) => void | Promise<void>
}

export function WorkspacePicker({
  open,
  initialPath = '',
  description,
  onOpenChange,
  onSelect,
}: WorkspacePickerProps) {
  const { t } = useI18n()
  const [path, setPath] = useState(initialPath)
  const [saving, setSaving] = useState(false)
  const [importing, setImporting] = useState(false)
  const [canImport, setCanImport] = useState(false)
  const [error, setError] = useState('')
  const generationRef = useRef(0)
  const busyRef = useRef(false)
  const busy = saving || importing

  useEffect(() => {
    const generation = ++generationRef.current
    busyRef.current = false
    if (open) {
      setPath(initialPath)
      setSaving(false)
      setImporting(false)
      setCanImport(false)
      setError('')
      if (window.__PISPER_MOBILE_PLATFORM__ === 'android') {
        void mobileWorkspaceMode().then(
          (mode) => {
            if (generation === generationRef.current) setCanImport(mode === 'local')
          },
          (caught: unknown) => {
            if (generation === generationRef.current)
              setError(caught instanceof Error ? caught.message : String(caught))
          },
        )
      }
    }
    return () => {
      generationRef.current += 1
    }
  }, [initialPath, open])

  const importDirectory = async () => {
    if (busyRef.current) return
    busyRef.current = true
    const generation = generationRef.current
    setImporting(true)
    setError('')
    try {
      const importedPath = await importMobileWorkspaceDirectory()
      if (generation === generationRef.current && importedPath) setPath(importedPath)
    } catch (caught) {
      if (generation === generationRef.current)
        setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      if (generation === generationRef.current) {
        busyRef.current = false
        setImporting(false)
      }
    }
  }

  const choose = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (busyRef.current || !path.trim()) return
    busyRef.current = true
    const generation = generationRef.current
    setSaving(true)
    setError('')
    try {
      // 目录存在性仍由各用例的 Runtime 入口统一校验，不能用目录枚举代替校验。
      await onSelect(path.trim())
      if (generation === generationRef.current) onOpenChange(false)
    } catch (caught) {
      if (generation === generationRef.current)
        setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      if (generation === generationRef.current) {
        busyRef.current = false
        setSaving(false)
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !busyRef.current && onOpenChange(nextOpen)}>
      <DialogContent showCloseButton={!busy} className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t('common:workspacePicker.setWorkingDirectory')}</DialogTitle>
          <DialogDescription>
            {description || t('common:workspacePicker.selectDirectoryDescription')}
          </DialogDescription>
        </DialogHeader>
        <form className="flex min-w-0 flex-col gap-4" onSubmit={(event) => void choose(event)}>
          <label className="flex min-w-0 flex-col gap-1.5 text-sm font-medium text-content">
            {t('common:workspacePicker.workingDirectoryPath')}
            <Input
              autoFocus
              className="font-mono text-xs font-normal max-[650px]:h-11"
              value={path}
              disabled={busy}
              placeholder={t('common:workspacePicker.enterAbsolutePath')}
              onChange={(event) => setPath(event.target.value)}
            />
          </label>
          {canImport && (
            <Button
              type="button"
              variant="secondary"
              className="max-[650px]:h-11"
              disabled={busy}
              onClick={() => void importDirectory()}
            >
              {importing ? <OrbitSpinner className="text-current" /> : <FolderInput />}
              {importing
                ? t('common:workspacePicker.importingFolder')
                : t('common:workspacePicker.importFolderCopy')}
            </Button>
          )}
          {error && (
            <Alert variant="destructive">
              <AlertTriangle />
              <AlertDescription className="min-w-0 break-words">{error}</AlertDescription>
            </Alert>
          )}
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
            {/* 「使用此文件夹」是对话框唯一的主操作：强调色实心键。 */}
            <Button type="submit" size="dialogAction" disabled={busy || !path.trim()}>
              {saving ? <OrbitSpinner className="text-current" /> : <Check />}
              {saving
                ? t('common:workspacePicker.switching')
                : t('common:workspacePicker.useThisFolder')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
