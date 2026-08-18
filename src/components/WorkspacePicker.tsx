// 工作区选择对话框：支持手动输入路径与系统目录选择（桌面桥接），
// 提交前校验目录可用性，失败时展示错误提示不关闭。
import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'
import { AlertTriangle, ArrowUp, Check, ChevronRight, Folder, LoaderCircle } from 'lucide-react'
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
import { ScrollArea } from '@/components/ui/scroll-area'
import { apiJson } from '@/lib/api'

export type DirectoryListing = {
  path: string
  parent?: string | null
  directories: Array<{ name: string; path: string }>
}

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
  const [listing, setListing] = useState<DirectoryListing | null>(null)
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const requestIdRef = useRef(0)

  const browse = useCallback(async (target: string) => {
    const requestId = ++requestIdRef.current
    setLoading(true)
    setError('')
    try {
      const data = await apiJson<DirectoryListing>(
        `/api/directories?path=${encodeURIComponent(target.trim())}`,
      )
      if (requestId !== requestIdRef.current) return
      setPath(data.path)
      setListing(data)
    } catch (caught) {
      if (requestId !== requestIdRef.current) return
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      if (requestId === requestIdRef.current) setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (!open) {
      requestIdRef.current += 1
      return
    }
    setPath(initialPath)
    setListing(null)
    setSaving(false)
    void browse(initialPath)
  }, [browse, initialPath, open])

  const submitPath = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    void browse(path)
  }

  const choose = async () => {
    if (!path.trim()) return
    setSaving(true)
    setError('')
    try {
      await onSelect(path.trim())
      onOpenChange(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !saving && onOpenChange(nextOpen)}>
      <DialogContent
        showCloseButton={!saving}
        className="max-w-[calc(100vw-24px)]! gap-0 overflow-hidden p-0 sm:max-w-[620px]!"
      >
        <DialogHeader className="border-b px-4 py-3 pr-12">
          <DialogTitle>{t('common:workspacePicker.setWorkingDirectory')}</DialogTitle>
          <DialogDescription>
            {description || t('common:workspacePicker.selectDirectoryDescription')}
          </DialogDescription>
        </DialogHeader>

        <form className="flex min-w-0 items-center gap-2 px-4 py-3" onSubmit={submitPath}>
          <Button
            type="button"
            variant="outline"
            size="icon"
            title={t('common:workspacePicker.parentFolder')}
            aria-label={t('common:workspacePicker.parentFolder')}
            disabled={loading || !listing?.parent}
            onClick={() => listing?.parent && void browse(listing.parent)}
          >
            <ArrowUp />
          </Button>
          <Input
            className="min-w-0 flex-1 font-mono text-[12px]"
            value={path}
            disabled={saving}
            aria-label={t('common:workspacePicker.workingDirectoryPath')}
            placeholder={t('common:workspacePicker.enterAbsolutePath')}
            onChange={(event) => setPath(event.target.value)}
          />
          <Button type="submit" variant="secondary" disabled={loading || saving || !path.trim()}>
            {loading ? <LoaderCircle className="animate-spin" /> : <ChevronRight />}
            {t('common:workspacePicker.go')}
          </Button>
        </form>

        <ScrollArea className="h-[min(340px,45dvh)] border-y bg-muted/20">
          <div className="p-2">
            {listing?.directories.map((directory) => (
              <button
                type="button"
                key={directory.path}
                className="grid h-9 w-full grid-cols-[24px_minmax(0,1fr)_20px] items-center rounded-md px-2 text-left text-[13px] hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                title={directory.path}
                disabled={loading || saving}
                onClick={() => void browse(directory.path)}
              >
                <Folder className="size-4 text-content-muted" />
                <span className="truncate">{directory.name}</span>
                <ChevronRight className="size-4 text-content-muted" />
              </button>
            ))}
            {loading && (
              <div className="flex h-32 items-center justify-center gap-2 text-content-muted">
                <LoaderCircle className="size-4 animate-spin" />
                <span>{t('common:workspacePicker.readingFolder')}</span>
              </div>
            )}
            {!loading && listing && !listing.directories.length && (
              <div className="flex h-32 items-center justify-center text-content-muted">
                {t('common:workspacePicker.thisFolderHasNoSubfolders')}
              </div>
            )}
          </div>
        </ScrollArea>

        {error && (
          <Alert variant="destructive" className="mx-4 mt-3 w-auto">
            <AlertTriangle />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        <DialogFooter className="m-0 border-t-0 bg-transparent px-4 py-3">
          <Button
            type="button"
            variant="outline"
            disabled={saving}
            onClick={() => onOpenChange(false)}
          >
            {t('common:ui.cancel')}
          </Button>
          <Button
            type="button"
            variant="secondary"
            disabled={loading || saving || !path.trim()}
            onClick={() => void choose()}
          >
            {saving ? <LoaderCircle className="animate-spin" /> : <Check />}
            {saving
              ? t('common:workspacePicker.switching')
              : t('common:workspacePicker.useThisFolder')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
