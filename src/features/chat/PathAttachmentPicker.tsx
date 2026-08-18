// 路径附件选择：浏览文件系统并选择路径作为附件（桌面桥接目录/文件选择）。
import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'
import { AlertTriangle, ArrowUp, ChevronRight, File, Folder, LoaderCircle } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
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

export type WorkspaceEntryListing = {
  path: string
  parent?: string | null
  directories: Array<{ name: string; path: string }>
  files: Array<{ name: string; path: string }>
}

export function PathAttachmentPicker({
  open,
  initialPath,
  onOpenChange,
  onSelect,
}: {
  open: boolean
  initialPath?: string
  onOpenChange: (open: boolean) => void
  onSelect: (paths: string[]) => void
}) {
  const { t } = useI18n()
  const [path, setPath] = useState(initialPath || '')
  const [listing, setListing] = useState<WorkspaceEntryListing | null>(null)
  const [selected, setSelected] = useState<string[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const requestIdRef = useRef(0)

  const browse = useCallback(async (target: string) => {
    const requestId = ++requestIdRef.current
    setLoading(true)
    setError('')
    try {
      const data = await apiJson<WorkspaceEntryListing>(
        `/api/workspace-entries?path=${encodeURIComponent(target.trim())}`,
      )
      if (requestId !== requestIdRef.current) return
      setPath(data.path)
      setListing(data)
      setSelected([])
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
    setPath(initialPath || '')
    setListing(null)
    setSelected([])
    void browse(initialPath || '')
  }, [browse, initialPath, open])

  const submitPath = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    void browse(path)
  }

  const toggle = (filePath: string, checked: boolean) => {
    setSelected((current) =>
      checked
        ? [...current.filter((item) => item !== filePath), filePath].slice(-8)
        : current.filter((item) => item !== filePath),
    )
  }

  const confirm = () => {
    if (!selected.length) return
    onSelect(selected)
    onOpenChange(false)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[calc(100vw-24px)]! gap-0 overflow-hidden p-0 sm:max-w-[680px]!">
        <DialogHeader className="border-b px-4 py-3 pr-12">
          <DialogTitle>{t('chat:pathAttachmentPicker.title')}</DialogTitle>
          <DialogDescription>{t('chat:pathAttachmentPicker.description')}</DialogDescription>
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
            aria-label={t('chat:pathAttachmentPicker.directoryPath')}
            onChange={(event) => setPath(event.target.value)}
          />
          <Button type="submit" variant="secondary" disabled={loading || !path.trim()}>
            {loading ? <LoaderCircle className="animate-spin" /> : <ChevronRight />}
            {t('common:workspacePicker.go')}
          </Button>
        </form>

        <ScrollArea className="h-[min(400px,50dvh)] border-y bg-muted/20">
          <div className="p-2">
            {listing?.directories.map((directory) => (
              <button
                type="button"
                key={directory.path}
                className="grid h-9 w-full grid-cols-[24px_minmax(0,1fr)_20px] items-center rounded-md px-2 text-left text-[13px] hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                title={directory.path}
                disabled={loading}
                onClick={() => void browse(directory.path)}
              >
                <Folder className="size-4 text-content-muted" />
                <span className="truncate">{directory.name}</span>
                <ChevronRight className="size-4 text-content-muted" />
              </button>
            ))}
            {listing?.files.map((file) => {
              const checked = selected.includes(file.path)
              return (
                <label
                  key={file.path}
                  className="grid h-9 cursor-pointer grid-cols-[24px_minmax(0,1fr)_24px] items-center rounded-md px-2 text-[13px] hover:bg-muted"
                  title={file.path}
                >
                  <File className="size-4 text-content-muted" />
                  <span className="truncate">{file.name}</span>
                  <Checkbox
                    checked={checked}
                    aria-label={t('chat:pathAttachmentPicker.selectName', { name: file.name })}
                    onCheckedChange={(value) => toggle(file.path, value === true)}
                  />
                </label>
              )
            })}
            {loading && (
              <div className="flex h-32 items-center justify-center gap-2 text-content-muted">
                <LoaderCircle className="size-4 animate-spin" />
                <span>{t('common:workspacePicker.readingFolder')}</span>
              </div>
            )}
            {!loading && listing && !listing.directories.length && !listing.files.length && (
              <div className="flex h-32 items-center justify-center text-content-muted">
                {t('chat:pathAttachmentPicker.empty')}
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
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            {t('common:ui.cancel')}
          </Button>
          <Button type="button" variant="secondary" disabled={!selected.length} onClick={confirm}>
            {t('chat:pathAttachmentPicker.attachCount', { count: selected.length })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
