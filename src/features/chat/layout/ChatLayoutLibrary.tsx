import { useId, useRef, useState } from 'react'
import { Copy, Pencil, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
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
import { Label } from '@/components/ui/label'
import type { SavedChatLayout } from './chat-layout-store'
import { layoutTransferErrorLabel } from './chat-layout-transfer'

type LibraryAction = 'rename' | 'saveAs' | 'delete'

export function ChatLayoutLibrary({
  saved,
  selectedId,
  draftName,
  invalid,
  onDraftName,
  onSelect,
  onRename,
  onRemove,
  onSaveAs,
}: {
  saved: SavedChatLayout[]
  selectedId: string | null
  draftName: string
  invalid: boolean
  onDraftName: (name: string) => void
  onSelect: (id: string) => void
  onRename: (name: string) => void
  onRemove: () => void
  onSaveAs: (name: string) => void
}) {
  const { t } = useI18n()
  const [action, setAction] = useState<LibraryAction | null>(null)
  const [name, setName] = useState('')
  const [error, setError] = useState('')
  const triggerRef = useRef<HTMLButtonElement | null>(null)
  const nameRef = useRef<HTMLInputElement>(null)
  const id = useId()
  const selected = saved.find((entry) => entry.id === selectedId)
  const open = (next: LibraryAction, button: HTMLButtonElement) => {
    triggerRef.current = button
    setName(next === 'rename' ? (selected?.template.name ?? draftName) : draftName)
    setError('')
    setAction(next)
  }
  const submit = () => {
    try {
      if (action === 'rename') onRename(name)
      else if (action === 'saveAs') onSaveAs(name)
      else if (action === 'delete') onRemove()
      setAction(null)
    } catch (reason) {
      setError(layoutTransferErrorLabel(reason, t))
    }
  }
  return (
    <section className="min-w-0 space-y-3 rounded-xl border border-border p-4">
      <header className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-sm font-medium">{t('chat-layout:layout.savedTemplates')}</h3>
        <span className="text-xs text-muted-foreground">
          {t('chat-layout:library.count', { count: saved.length })}
        </span>
      </header>
      <div className="flex min-w-0 flex-wrap items-end gap-3">
        {saved.length > 0 ? (
          <div className="min-w-[180px] flex-1 space-y-2">
            <Label id={`${id}-select`} className="text-xs">
              {t('chat-layout:library.choose')}
            </Label>
            <AppSelect
              aria-labelledby={`${id}-select`}
              value={selected?.id ?? ''}
              onChange={(event) => onSelect(event.target.value)}
            >
              <option value="" disabled>
                {t('chat-layout:layout.selectTemplate')}
              </option>
              {saved.map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {entry.template.name}
                </option>
              ))}
            </AppSelect>
          </div>
        ) : (
          <p className="min-w-0 flex-1 text-sm text-muted-foreground">
            {t('chat-layout:library.empty')}
          </p>
        )}
        <div className="min-w-[180px] flex-1 space-y-2">
          <Label htmlFor={`${id}-name`} className="text-xs">
            {t('chat-layout:library.editingName')}
          </Label>
          <Input
            ref={nameRef}
            id={`${id}-name`}
            value={draftName}
            readOnly={Boolean(selected)}
            maxLength={80}
            onChange={(event) => onDraftName(event.currentTarget.value)}
          />
        </div>
        <div className="flex flex-wrap gap-1.5">
          <Button
            variant="outline"
            disabled={!selected}
            onClick={(event) => open('rename', event.currentTarget)}
          >
            <Pencil />
            {t('chat-layout:library.rename')}
          </Button>
          <Button
            variant="outline"
            disabled={invalid}
            onClick={(event) => open('saveAs', event.currentTarget)}
          >
            <Copy />
            {t('chat-layout:library.saveAs')}
          </Button>
          <Button
            variant="ghost"
            disabled={!selected}
            aria-label={t('chat-layout:layout.delete')}
            title={t('chat-layout:layout.delete')}
            onClick={(event) => open('delete', event.currentTarget)}
          >
            <Trash2 />
          </Button>
        </div>
      </div>
      <Dialog
        open={action !== null}
        onOpenChange={(next) => {
          if (!next) setAction(null)
        }}
      >
        <DialogContent
          className="min-w-0 grid-cols-[minmax(0,1fr)] overflow-x-hidden"
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            if (triggerRef.current?.isConnected && !triggerRef.current.disabled)
              triggerRef.current.focus()
            else nameRef.current?.focus()
          }}
        >
          <DialogHeader className="min-w-0">
            <DialogTitle>
              {action === 'rename'
                ? t('chat-layout:library.renameTitle')
                : action === 'saveAs'
                  ? t('chat-layout:library.saveAsTitle')
                  : t('chat-layout:layout.deleteTitle')}
            </DialogTitle>
            <DialogDescription className="[overflow-wrap:anywhere]">
              {action === 'rename'
                ? t('chat-layout:library.renameHint')
                : action === 'saveAs'
                  ? t('chat-layout:library.saveAsHint')
                  : t('chat-layout:layout.deleteHint', { name: selected?.template.name ?? '' })}
            </DialogDescription>
          </DialogHeader>
          {action !== 'delete' && (
            <div className="min-w-0 space-y-2">
              <Label htmlFor={`${id}-dialog-name`}>{t('chat-layout:layout.name')}</Label>
              <Input
                id={`${id}-dialog-name`}
                value={name}
                maxLength={80}
                onChange={(event) => {
                  setName(event.currentTarget.value)
                  setError('')
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && name.trim()) {
                    event.preventDefault()
                    submit()
                  }
                }}
              />
            </div>
          )}
          {error && (
            <p role="alert" className="text-sm leading-6">
              {error}
            </p>
          )}
          <DialogFooter className="min-w-0">
            <Button variant="outline" onClick={() => setAction(null)}>
              {t('chat-layout:layout.cancel')}
            </Button>
            <Button disabled={action !== 'delete' && !name.trim()} onClick={submit}>
              {action === 'rename'
                ? t('chat-layout:library.rename')
                : action === 'saveAs'
                  ? t('chat-layout:library.saveAs')
                  : t('chat-layout:layout.delete')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  )
}
