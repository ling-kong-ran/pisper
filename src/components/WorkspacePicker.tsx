// 系统目录选择器由调用方打开；它出不来时（手机、配对设备、无图形会话的主机）用这个面板：
// Runtime 支持时在下面逐级点选它所在电脑上的文件夹，也可以直接填写路径。
import { useEffect, useRef, useState, type FormEvent } from 'react'
import {
  AlertTriangle,
  ArrowUp,
  Check,
  ChevronRight,
  Folder,
  FolderInput,
  FolderOpen,
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
import { InlineEmptyState } from '@/components/app/PageState'
import { OrbitSpinner } from '@/components/ui/spinner'
import { ApiError } from '@/lib/api-error'
import { importMobileWorkspaceDirectory, mobileWorkspaceMode } from '@/lib/mobile-workspace'
import { listWorkspaceFolders, type WorkspaceFolderListing } from '@/lib/workspace-folders'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

type WorkspacePickerProps = {
  open: boolean
  initialPath?: string
  description?: string
  onOpenChange: (open: boolean) => void
  onSelect: (path: string) => void | Promise<void>
  // 列文件夹的来源：默认是 Runtime 所在电脑；Bot 在另一台电脑上时由调用方换成那台电脑的列表。
  listFolders?: (path: string, signal: AbortSignal) => Promise<WorkspaceFolderListing>
  // 关闭时焦点默认回到打开前的位置；调用方选完目录要把焦点交给别处（例如启动台）时接管。
  onCloseAutoFocus?: (event: Event) => void
}

export function WorkspacePicker({
  open,
  initialPath = '',
  description,
  onOpenChange,
  onSelect,
  onCloseAutoFocus,
  listFolders,
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
  const runtimeCanList = useRuntimeCapabilitiesStore(
    (state) => state.capabilities.features.workspaceFolderList,
  )
  const canBrowse = Boolean(listFolders) || runtimeCanList
  const [listing, setListing] = useState<WorkspaceFolderListing | null>(null)
  const [listLoading, setListLoading] = useState(false)
  const [listError, setListError] = useState('')
  const listAbortRef = useRef<AbortController | null>(null)

  const listErrorText = (caught: unknown) => {
    const code = caught instanceof ApiError ? caught.data?.code : undefined
    if (code === 'folder_missing') return t('common:workspacePicker.folderMissing')
    if (code === 'folder_permission_denied') return t('common:workspacePicker.folderDenied')
    if (code === 'path_not_allowed') return t('common:workspacePicker.folderNotAllowed')
    if (code === 'executor_offline' || code === 'executor_timeout')
      return t('common:workspacePicker.computerOffline')
    if (code === 'executor_outdated') return t('common:workspacePicker.computerOutdated')
    return t('common:workspacePicker.folderListFailed')
  }

  // 打开一个文件夹：只认最后一次点击的结果，前一个还没回来的请求直接取消。
  // follow 为真时把路径框改成刚打开的文件夹，这样点进去就等于选中了它。
  const browse = async (target: string, follow: boolean, retryDefault = false) => {
    listAbortRef.current?.abort()
    const controller = new AbortController()
    listAbortRef.current = controller
    setListLoading(true)
    setListError('')
    try {
      const next = await (listFolders ?? listWorkspaceFolders)(target, controller.signal)
      if (controller.signal.aborted) return
      setListing(next)
      if (follow && next.path) setPath(next.path)
    } catch (caught) {
      if (controller.signal.aborted) return
      // 起始路径失效（目录被删、还没填过）时退回 Runtime 默认工作目录，不覆盖路径框里的原值。
      if (retryDefault && target) {
        void browse('', false)
        return
      }
      setListError(listErrorText(caught))
    } finally {
      if (listAbortRef.current === controller) {
        listAbortRef.current = null
        setListLoading(false)
      }
    }
  }

  useEffect(() => {
    const generation = ++generationRef.current
    busyRef.current = false
    if (open) {
      setPath(initialPath)
      setSaving(false)
      setImporting(false)
      setCanImport(false)
      setError('')
      setListing(null)
      setListError('')
      if (canBrowse) void browse(initialPath, !initialPath, true)
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
      listAbortRef.current?.abort()
      listAbortRef.current = null
    }
    // browse 每次渲染都会重建，只在打开、换起始路径或能力到位时重新加载。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canBrowse, initialPath, open])

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
      <DialogContent
        showCloseButton={!busy}
        className="sm:max-w-lg"
        onCloseAutoFocus={onCloseAutoFocus}
      >
        <DialogHeader>
          <DialogTitle>{t('common:workspacePicker.setWorkingDirectory')}</DialogTitle>
          <DialogDescription>
            {description ||
              (canBrowse
                ? t('common:workspacePicker.browseDescription')
                : t('common:workspacePicker.selectDirectoryDescription'))}
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
          {canBrowse && (
            <section
              aria-label={t('common:workspacePicker.folders')}
              aria-busy={listLoading}
              className="flex min-w-0 flex-col overflow-hidden rounded-md border border-stroke"
            >
              <div className="flex min-w-0 items-center gap-1 border-b border-stroke p-1">
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  aria-label={t('common:workspacePicker.goUp')}
                  title={t('common:workspacePicker.goUp')}
                  disabled={busy || listLoading || !listing?.parent}
                  onClick={() => listing && void browse(listing.parent, true)}
                >
                  <ArrowUp />
                </Button>
                {/* 长路径省略开头：手机上要看见的是当前所在的那一层。 */}
                <span
                  dir="rtl"
                  className="min-w-0 flex-1 truncate text-left font-mono text-xs text-content-muted"
                  title={listing?.path}
                >
                  <bdi dir="ltr">{listing?.path}</bdi>
                </span>
                {listLoading && <OrbitSpinner className="mr-1.5 size-3.5" />}
              </div>
              <div className="max-h-64 min-h-24 overflow-y-auto overscroll-contain p-1">
                {listError ? (
                  <p role="alert" className="m-0 px-2 py-3 text-xs text-destructive">
                    {listError}
                  </p>
                ) : listing && listing.folders.length === 0 ? (
                  <InlineEmptyState
                    compact
                    icon={FolderOpen}
                    title={t('common:workspacePicker.noSubfolders')}
                  />
                ) : (
                  <ul role="list" className="m-0 list-none p-0">
                    {listing?.folders.map((folder) => (
                      <li key={folder.path}>
                        <button
                          type="button"
                          className="flex h-8 w-full min-w-0 items-center gap-2 rounded-sm px-2 text-left text-sm text-content outline-none hover:bg-surface-hover focus-visible:ring-3 focus-visible:ring-focus-ring disabled:opacity-50 max-[650px]:h-11 pointer-coarse:h-11"
                          disabled={busy}
                          onClick={() => void browse(folder.path, true)}
                        >
                          <Folder
                            className="size-4 flex-none text-content-muted"
                            aria-hidden="true"
                          />
                          <span className="min-w-0 flex-1 truncate">{folder.name}</span>
                          <ChevronRight
                            className="size-3.5 flex-none text-content-muted"
                            aria-hidden="true"
                          />
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
                {listing?.truncated && !listError && (
                  <p className="m-0 px-2 py-2 text-xs text-content-muted">
                    {t('common:workspacePicker.listTruncated', { count: listing.folders.length })}
                  </p>
                )}
              </div>
            </section>
          )}
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
