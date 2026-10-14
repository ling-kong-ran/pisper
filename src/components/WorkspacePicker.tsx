// 工作目录优先通过浏览选择；Android 本机可从系统文件夹选择器导入可读写副本。
import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'
import {
  AlertTriangle,
  ArrowUp,
  Check,
  ChevronRight,
  Folder,
  FolderInput,
  Home,
  LoaderCircle,
  RefreshCw,
  SquarePen,
} from 'lucide-react'
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
import { importMobileWorkspaceDirectory, mobileWorkspaceMode } from '@/lib/mobile-workspace'

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
  const [editingPath, setEditingPath] = useState(false)
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [importing, setImporting] = useState(false)
  const [mode, setMode] = useState<'local' | 'remote' | null>(null)
  const [error, setError] = useState('')
  const requestIdRef = useRef(0)
  const openGenerationRef = useRef(0)
  const busy = saving || importing
  const canImport = mode === 'local' && window.__PISPER_MOBILE_PLATFORM__ === 'android'

  const browse = useCallback(
    async (target: string) => {
      const requestId = ++requestIdRef.current
      setLoading(true)
      setError('')
      try {
        const data = await apiJson<DirectoryListing>(
          `/api/directories?path=${encodeURIComponent(target.trim())}`,
        )
        if (requestId !== requestIdRef.current) return
        if (
          !data ||
          typeof data.path !== 'string' ||
          !data.path.trim() ||
          (data.parent != null && (typeof data.parent !== 'string' || !data.parent.trim())) ||
          !Array.isArray(data.directories) ||
          data.directories.some(
            (entry) =>
              !entry ||
              typeof entry.name !== 'string' ||
              !entry.name.trim() ||
              typeof entry.path !== 'string' ||
              !entry.path.trim(),
          )
        ) {
          throw new Error(t('common:workspacePicker.invalidListing'))
        }
        setPath(data.path)
        setListing(data)
        setEditingPath(false)
      } catch (caught) {
        if (requestId !== requestIdRef.current) return
        setError(caught instanceof Error ? caught.message : String(caught))
      } finally {
        if (requestId === requestIdRef.current) setLoading(false)
      }
    },
    [t],
  )

  useEffect(() => {
    const generation = ++openGenerationRef.current
    if (open) {
      setPath(initialPath)
      setListing(null)
      setSaving(false)
      setImporting(false)
      setEditingPath(false)
      setMode(null)
      void browse(initialPath)
      void mobileWorkspaceMode()
        .then((nextMode) => {
          if (generation === openGenerationRef.current) setMode(nextMode)
        })
        .catch((caught: unknown) => {
          if (generation === openGenerationRef.current) {
            setError(caught instanceof Error ? caught.message : String(caught))
          }
        })
    }
    return () => {
      requestIdRef.current += 1
      openGenerationRef.current += 1
    }
  }, [browse, initialPath, open])

  const submitPath = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!busy && !loading) void browse(path)
  }

  const importDirectory = async () => {
    const generation = openGenerationRef.current
    setImporting(true)
    setError('')
    try {
      const importedPath = await importMobileWorkspaceDirectory()
      if (generation === openGenerationRef.current && importedPath) await browse(importedPath)
    } catch (caught) {
      if (generation === openGenerationRef.current) {
        setError(caught instanceof Error ? caught.message : String(caught))
      }
    } finally {
      if (generation === openGenerationRef.current) setImporting(false)
    }
  }

  const choose = async () => {
    if (!listing || busy || loading || error || path !== listing.path) return
    setSaving(true)
    setError('')
    try {
      await onSelect(listing.path)
      onOpenChange(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !busy && onOpenChange(nextOpen)}>
      <DialogContent
        showCloseButton={!busy}
        className="max-w-[calc(100vw-24px)]! gap-0 overflow-hidden p-0 sm:max-w-[620px]!"
      >
        <DialogHeader className="border-b px-4 py-3 pr-12">
          <DialogTitle>{t('common:workspacePicker.setWorkingDirectory')}</DialogTitle>
          <DialogDescription>
            {description || t('common:workspacePicker.selectDirectoryDescription')}
          </DialogDescription>
        </DialogHeader>

        {canImport && (
          <div className="border-b px-4 py-3">
            <Button
              type="button"
              variant="outline"
              className="h-auto min-h-10 w-full whitespace-normal"
              disabled={busy || loading}
              onClick={() => void importDirectory()}
            >
              {importing ? <LoaderCircle className="animate-spin" /> : <FolderInput />}
              {importing
                ? t('common:workspacePicker.importingFolder')
                : t('common:workspacePicker.importFolderCopy')}
            </Button>
          </div>
        )}

        <div className="flex min-w-0 items-center gap-2 px-4 py-3">
          <Button
            type="button"
            variant="outline"
            size="icon"
            title={t('common:workspacePicker.parentFolder')}
            aria-label={t('common:workspacePicker.parentFolder')}
            disabled={loading || busy || !listing?.parent}
            onClick={() => listing?.parent && void browse(listing.parent)}
          >
            <ArrowUp />
          </Button>
          <Button
            type="button"
            variant="outline"
            size="icon"
            title={t('common:workspacePicker.defaultFolder')}
            aria-label={t('common:workspacePicker.defaultFolder')}
            disabled={loading || busy}
            onClick={() => void browse('')}
          >
            <Home />
          </Button>
          <div className="min-w-0 flex-1">
            <div className="truncate text-[11px] text-content-muted">
              {mode === 'remote'
                ? t('common:workspacePicker.serverFolder')
                : t('common:workspacePicker.workingDirectoryPath')}
            </div>
            <div className="truncate font-mono text-xs" title={listing?.path || ''}>
              {listing?.path || t('common:workspacePicker.defaultFolder')}
            </div>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            title={t('common:workspacePicker.enterPath')}
            aria-label={t('common:workspacePicker.enterPath')}
            aria-expanded={editingPath}
            disabled={busy || loading}
            onClick={() => {
              setPath(listing?.path || initialPath)
              setEditingPath((current) => !current)
            }}
          >
            <SquarePen />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            title={t('common:workspacePicker.refreshFolder')}
            aria-label={t('common:workspacePicker.refreshFolder')}
            disabled={busy || loading}
            onClick={() => void browse(listing?.path || initialPath)}
          >
            {loading ? <LoaderCircle className="animate-spin" /> : <RefreshCw />}
          </Button>
        </div>

        {editingPath && (
          <form className="flex min-w-0 items-center gap-2 px-4 pb-3" onSubmit={submitPath}>
            <Input
              className="min-w-0 flex-1 font-mono text-[12px]"
              value={path}
              disabled={busy || loading}
              aria-label={t('common:workspacePicker.workingDirectoryPath')}
              placeholder={t('common:workspacePicker.enterAbsolutePath')}
              onChange={(event) => setPath(event.target.value)}
            />
            <Button type="submit" variant="secondary" disabled={loading || busy || !path.trim()}>
              <ChevronRight />
              {t('common:workspacePicker.go')}
            </Button>
          </form>
        )}

        <ScrollArea className="h-[min(340px,40dvh)] border-y bg-muted/20">
          <div className="p-2">
            {!loading &&
              listing?.directories.map((directory) => (
                <button
                  type="button"
                  key={directory.path}
                  className="grid min-h-11 w-full grid-cols-[24px_minmax(0,1fr)_20px] items-center rounded-md px-2 py-2 text-left text-[13px] hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  title={directory.path}
                  disabled={busy}
                  onClick={() => void browse(directory.path)}
                >
                  <Folder className="size-4 text-content-muted" />
                  <span className="truncate">{directory.name}</span>
                  <ChevronRight className="size-4 text-content-muted" />
                </button>
              ))}
            {loading && (
              <div
                role="status"
                className="flex h-32 items-center justify-center gap-2 text-content-muted"
              >
                <LoaderCircle className="size-4 animate-spin" />
                <span>{t('common:workspacePicker.readingFolder')}</span>
              </div>
            )}
            {!loading && !error && listing && !listing.directories.length && (
              <div className="flex h-32 items-center justify-center text-sm text-content-muted">
                {t('common:workspacePicker.thisFolderHasNoSubfolders')}
              </div>
            )}
          </div>
        </ScrollArea>

        {error && (
          <Alert variant="destructive" className="mx-4 mt-3 w-auto">
            <AlertTriangle />
            <AlertDescription className="min-w-0 break-words">{error}</AlertDescription>
          </Alert>
        )}

        <DialogFooter className="m-0 border-t-0 bg-transparent px-4 py-3">
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() => onOpenChange(false)}
          >
            {t('common:ui.cancel')}
          </Button>
          <Button
            type="button"
            variant="secondary"
            disabled={loading || busy || !listing || !!error || path !== listing.path}
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
