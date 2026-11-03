// 新建文件夹：在“位置”下新建一个空文件夹，并把它登记为项目、选为派发目标。
// 位置可以直接填写，也可以用“浏览…”交给系统文件管理器选（选的是已有的上级目录）；系统对话框
// 打不开时（配对设备、无图形会话、手机）提示改为手动填写。名称与上级目录的校验在 Runtime 里做，
// 这里只把结构化错误码翻译成当前语言。
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { AlertTriangle, FolderOpen, FolderPlus } from 'lucide-react'
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
import { FieldDescription, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from '@/components/ui/input-group'
import { OrbitSpinner } from '@/components/ui/spinner'
import { createProjectFolder } from '@/features/chat/project-actions'
import { ApiError } from '@/lib/api-error'
import { pickSystemDirectory } from '@/lib/pick-system-directory'
import { joinFolderPath } from './folder-paths'

type Translate = ReturnType<typeof useI18n>['t']

// Runtime 的结构化错误码 → 当前语言的文案；不认识的错误码返回空串，交给通用失败文案。
function folderErrorMessage(code: unknown, t: Translate) {
  switch (code) {
    case 'folder_name_required':
      return t('board:folder.error.nameRequired')
    case 'folder_name_invalid':
      return t('board:folder.error.nameInvalid')
    case 'folder_parent_missing':
    case 'folder_parent_invalid':
      return t('board:folder.error.parentMissing')
    case 'folder_exists_as_file':
      return t('board:folder.error.existsAsFile')
    case 'folder_permission_denied':
      return t('board:folder.error.permission')
    default:
      return ''
  }
}

export type NewFolderDialogProps = {
  open: boolean
  // 打开时预填的位置（当前项目的上级目录）；空串表示用 Runtime 的默认工作目录。
  initialParent: string
  onOpenChange: (open: boolean) => void
  // 文件夹已建好（或同名文件夹已存在）：Runtime 返回的绝对路径。
  onCreated: (path: string) => void
  onCloseAutoFocus?: (event: Event) => void
}

export function NewFolderDialog({
  open,
  initialParent,
  onOpenChange,
  onCreated,
  onCloseAutoFocus,
}: NewFolderDialogProps) {
  const { t } = useI18n()
  const [parent, setParent] = useState(initialParent)
  const [name, setName] = useState('')
  const [saving, setSaving] = useState(false)
  const [browsing, setBrowsing] = useState(false)
  const [browseUnavailable, setBrowseUnavailable] = useState(false)
  const [error, setError] = useState('')
  const generationRef = useRef(0)
  const busyRef = useRef(false)
  const parentRef = useRef<HTMLInputElement>(null)
  const busy = saving || browsing

  // 每次打开都从预填位置、空名称开始；关闭后迟到的请求结果不再改动界面。
  useEffect(() => {
    generationRef.current += 1
    busyRef.current = false
    if (!open) return
    setParent(initialParent)
    setName('')
    setSaving(false)
    setBrowsing(false)
    setBrowseUnavailable(false)
    setError('')
  }, [initialParent, open])

  const browse = async () => {
    if (busyRef.current) return
    busyRef.current = true
    const generation = generationRef.current
    setBrowsing(true)
    setError('')
    try {
      const result = await pickSystemDirectory(parent.trim(), t('board:folder.browseTitle'))
      if (generation !== generationRef.current) return
      if (result.status === 'picked') setParent(result.path)
      else if (result.status === 'unavailable') {
        setBrowseUnavailable(true)
        parentRef.current?.focus()
      }
    } catch (caught) {
      if (generation === generationRef.current)
        setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      if (generation === generationRef.current) {
        busyRef.current = false
        setBrowsing(false)
      }
    }
  }

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (busyRef.current || !name.trim()) return
    busyRef.current = true
    const generation = generationRef.current
    setSaving(true)
    setError('')
    try {
      const result = await createProjectFolder(parent.trim(), name.trim())
      if (generation !== generationRef.current) return
      onCreated(result.path)
      onOpenChange(false)
    } catch (caught) {
      if (generation !== generationRef.current) return
      const known = folderErrorMessage(
        caught instanceof ApiError ? caught.data?.code : undefined,
        t,
      )
      setError(
        known ||
          (caught instanceof Error && caught.message
            ? t('board:folder.error.failedWithReason', { reason: caught.message })
            : t('board:folder.error.failed')),
      )
    } finally {
      if (generation === generationRef.current) {
        busyRef.current = false
        setSaving(false)
      }
    }
  }

  const preview = name.trim() ? joinFolderPath(parent, name.trim()) : ''

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !busyRef.current && onOpenChange(nextOpen)}>
      <DialogContent
        showCloseButton={!busy}
        aria-describedby="board-new-folder"
        onCloseAutoFocus={onCloseAutoFocus}
        data-new-folder-dialog
      >
        <DialogHeader>
          <DialogTitle>{t('board:folder.title')}</DialogTitle>
          <DialogDescription id="board-new-folder">
            {t('board:folder.description')}
          </DialogDescription>
        </DialogHeader>
        <form className="flex min-w-0 flex-col gap-4" onSubmit={(event) => void submit(event)}>
          <FieldLabel variant="control">
            {t('board:folder.parent')}
            <InputGroup className="max-[650px]:h-11">
              <InputGroupInput
                ref={parentRef}
                className="font-mono text-xs"
                value={parent}
                disabled={busy}
                placeholder={t('board:folder.parentPlaceholder')}
                data-new-folder-parent
                onChange={(event) => setParent(event.target.value)}
              />
              <InputGroupAddon align="inline-end">
                <InputGroupButton
                  size="xs"
                  disabled={busy}
                  data-new-folder-browse
                  onClick={() => void browse()}
                >
                  {browsing ? <OrbitSpinner className="text-current" /> : <FolderOpen />}
                  {t('board:folder.browse')}
                </InputGroupButton>
              </InputGroupAddon>
            </InputGroup>
          </FieldLabel>
          {browseUnavailable && (
            <FieldDescription className="-mt-2">
              {t('board:folder.browseUnavailable')}
            </FieldDescription>
          )}
          <FieldLabel variant="control">
            {t('board:folder.name')}
            <Input
              autoFocus
              className="max-[650px]:h-11"
              value={name}
              disabled={busy}
              placeholder={t('board:folder.namePlaceholder')}
              autoComplete="off"
              spellCheck={false}
              data-new-folder-name
              onChange={(event) => {
                setName(event.target.value)
                if (error) setError('')
              }}
            />
          </FieldLabel>
          {preview && (
            <p className="-mt-2 truncate font-mono text-xs text-content-muted" title={preview}>
              {t('board:folder.preview', { path: preview })}
            </p>
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
            <Button type="submit" size="dialogAction" disabled={busy || !name.trim()}>
              {saving ? <OrbitSpinner className="text-current" /> : <FolderPlus />}
              {saving ? t('board:folder.creating') : t('board:folder.create')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
