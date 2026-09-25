import { useRef, useState } from 'react'
import {
  ArrowRight,
  Check,
  Focus,
  LayoutTemplate,
  Monitor,
  PanelsTopLeft,
  PanelTop,
  RotateCcw,
  Save,
  Smartphone,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { cn } from '@/lib/utils'
import { ChatCanvasEditor } from './ChatCanvasEditor'
import { ChatLayoutLibrary } from './ChatLayoutLibrary'
import { ChatLayoutExportDialog, ChatLayoutImportDialog } from './ChatLayoutTransfer'
import {
  CHAT_LAYOUT_PRESETS,
  ChatLayoutValidationError,
  DEFAULT_CHAT_LAYOUT,
  equalChatLayoutContent,
  type ChatLayoutTemplate,
} from './chat-layout'
import { chatLayoutPresetLabels } from './chat-layout-preset-labels'
import { useChatLayoutStore } from './chat-layout-store'
import { layoutTransferErrorLabel } from './chat-layout-transfer'

type PendingLayout = { template: ChatLayoutTemplate; id: string | null; notice?: string }
const presetIcons = {
  default: LayoutTemplate,
  focus: Focus,
  workbench: PanelsTopLeft,
  studio: PanelTop,
}

export function ChatLayoutEditor() {
  const { t } = useI18n()
  const active = useChatLayoutStore((state) => state.active)
  const saved = useChatLayoutStore((state) => state.saved)
  const storageError = useChatLayoutStore((state) => state.storageError)
  const [draft, setDraft] = useState(active)
  const [baseline, setBaseline] = useState(active)
  const [device, setDevice] = useState<'desktop' | 'mobile'>('desktop')
  const [selectedSaved, setSelectedSaved] = useState<string | null>(
    () =>
      saved.find(
        (entry) =>
          entry.template.name === active.name && equalChatLayoutContent(entry.template, active),
      )?.id ?? null,
  )
  const [status, setStatus] = useState('')
  const [error, setError] = useState('')
  const [canvasValid, setCanvasValid] = useState(true)
  const [canvasEpoch, setCanvasEpoch] = useState(0)
  const [pendingLoad, setPendingLoad] = useState<PendingLayout | null>(null)
  const editorRef = useRef<HTMLDivElement>(null)
  const returnFocusRef = useRef<HTMLElement | null>(null)
  const dirty = JSON.stringify(draft) !== JSON.stringify(active)
  const hasUnsaved = JSON.stringify(draft) !== JSON.stringify(baseline) || !canvasValid
  const selected = saved.find((entry) => entry.id === selectedSaved)
  const labels = chatLayoutPresetLabels(t)

  const edit = (next: ChatLayoutTemplate) => {
    setDraft(next)
    setStatus('')
    setError('')
  }
  const load = ({ template, id, notice }: PendingLayout) => {
    edit(template)
    setBaseline(template)
    setSelectedSaved(id)
    setCanvasEpoch((value) => value + 1)
    if (notice) setStatus(notice)
  }
  const requestLoad = (next: PendingLayout) => {
    if (hasUnsaved) {
      returnFocusRef.current =
        document.activeElement instanceof HTMLElement ? document.activeElement : null
      setPendingLoad(next)
    } else load(next)
  }
  const savedEntry = (id: string) => {
    const entry = useChatLayoutStore.getState().saved.find((item) => item.id === id)
    if (!entry) throw new ChatLayoutValidationError('not_found')
    return entry
  }
  const apply = () => {
    try {
      useChatLayoutStore.getState().apply(draft)
      setDraft(useChatLayoutStore.getState().active)
      setStatus(t('chat-layout:layout.applied'))
      setError('')
    } catch (reason) {
      setError(layoutTransferErrorLabel(reason, t))
    }
  }
  const persistDraft = (name: string, createNew: boolean) => {
    const value = { ...draft, name }
    const store = useChatLayoutStore.getState()
    const id = createNew ? store.importTemplate(value) : store.save(value)
    const entry = savedEntry(id)
    // 保存和重命名只更新模板元信息；不重挂载正在编辑的画布，也不清除未完成的样式输入。
    setDraft((current) => ({ ...current, name: entry.template.name }))
    setBaseline(entry.template)
    setSelectedSaved(id)
    setError('')
    setStatus(t('chat-layout:library.saved', { name: entry.template.name }))
  }
  const save = () => {
    try {
      persistDraft(selected?.template.name ?? draft.name, !selected)
    } catch (reason) {
      setError(layoutTransferErrorLabel(reason, t))
    }
  }
  const rename = (name: string) => {
    if (!selected) throw new ChatLayoutValidationError('not_found')
    useChatLayoutStore.getState().rename(selected.id, name)
    const renamed = savedEntry(selected.id).template.name
    setDraft((current) => ({ ...current, name: renamed }))
    setBaseline((current) => ({ ...current, name: renamed }))
    setError('')
    setStatus(t('chat-layout:library.renamed'))
  }
  const remove = () => {
    if (selected) useChatLayoutStore.getState().remove(selected.id)
    setSelectedSaved(null)
    setStatus(t('chat-layout:layout.deleted'))
  }
  const reset = () =>
    requestLoad({
      template: DEFAULT_CHAT_LAYOUT,
      id: null,
      notice: t('chat-layout:layout.restored'),
    })
  return (
    <div ref={editorRef} className="mx-auto w-full max-w-[1440px] space-y-6 px-1 pb-6">
      <header className="flex flex-wrap items-start justify-between gap-4">
        <div className="space-y-1.5">
          <h2 className="text-lg font-medium tracking-tight">{t('chat-layout:layout.title')}</h2>
          <p className="max-w-2xl text-sm leading-6 text-muted-foreground">
            {t('chat-layout:layout.description')}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <ChatLayoutImportDialog
            onImport={(template) => {
              const id = useChatLayoutStore.getState().importTemplate(template)
              const imported = savedEntry(id)
              requestLoad({
                template: imported.template,
                id,
                notice: t('chat-layout:library.imported', { name: imported.template.name }),
              })
            }}
          />
          <ChatLayoutExportDialog template={draft} disabled={!canvasValid} />
        </div>
      </header>
      <details className="rounded-xl border border-border px-4 py-3">
        <summary className="cursor-pointer text-sm font-medium">
          {t('chat-layout:canvas.startingPoints')}
        </summary>
        <div
          className="mt-3 grid gap-2 sm:grid-cols-2 xl:grid-cols-4"
          aria-label={t('chat-layout:layout.presets')}
        >
          {CHAT_LAYOUT_PRESETS.map(({ id, template }) => {
            const { title, description } = labels[id]
            const Icon = presetIcons[id]
            const matches = equalChatLayoutContent(draft, template)
            return (
              <button
                key={id}
                type="button"
                aria-pressed={matches}
                onClick={() => requestLoad({ template: { ...template, name: title }, id: null })}
                className={cn(
                  'flex min-w-0 items-center gap-3 rounded-xl border px-4 py-3 text-left transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring',
                  matches ? 'border-foreground/35 bg-muted/50' : 'border-border hover:bg-muted/40',
                )}
              >
                <Icon className="size-5 shrink-0 text-muted-foreground" />
                <span className="min-w-0 space-y-0.5">
                  <span className="block text-sm font-medium">{title}</span>
                  <span className="block text-xs leading-5 text-muted-foreground">
                    {description}
                  </span>
                </span>
                {matches && <Check className="ml-auto size-4 shrink-0" />}
              </button>
            )
          })}
        </div>
      </details>
      <ChatLayoutLibrary
        saved={saved}
        selectedId={selectedSaved}
        draftName={draft.name}
        invalid={!canvasValid}
        onDraftName={(name) => edit({ ...draft, name })}
        onSelect={(id) => {
          const entry = saved.find((item) => item.id === id)
          if (entry) requestLoad({ template: entry.template, id })
        }}
        onRename={rename}
        onRemove={remove}
        onSaveAs={(name) => persistDraft(name, true)}
      />
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div
          className="flex gap-1 rounded-lg bg-muted p-1"
          role="group"
          aria-label={t('chat-layout:canvas.device')}
        >
          <Button
            variant={device === 'desktop' ? 'secondary' : 'ghost'}
            aria-pressed={device === 'desktop'}
            disabled={!canvasValid}
            onClick={() => setDevice('desktop')}
          >
            <Monitor />
            {t('chat-layout:layout.desktop')}
          </Button>
          <Button
            variant={device === 'mobile' ? 'secondary' : 'ghost'}
            aria-pressed={device === 'mobile'}
            disabled={!canvasValid}
            onClick={() => setDevice('mobile')}
          >
            <Smartphone />
            {t('chat-layout:layout.mobile')}
          </Button>
        </div>
        {hasUnsaved && (
          <span className="text-xs text-muted-foreground">{t('chat-layout:library.unsaved')}</span>
        )}
      </div>
      <ChatCanvasEditor
        key={`${device}:${canvasEpoch}`}
        root={draft[device].canvas}
        mobile={device === 'mobile'}
        onChange={(canvas) => edit({ ...draft, [device]: { ...draft[device], canvas } })}
        onValidityChange={setCanvasValid}
      />
      <div className="sticky bottom-0 z-10 flex flex-wrap items-center justify-between gap-3 rounded-xl border border-border bg-background p-3 shadow-sm">
        <div className="flex flex-wrap items-center gap-1">
          <Button variant="outline" onClick={save} disabled={!canvasValid || !draft.name.trim()}>
            <Save />
            {selected ? t('chat-layout:library.saveChanges') : t('chat-layout:layout.save')}
          </Button>
          <Button
            variant="ghost"
            onClick={() => load({ template: baseline, id: selected?.id ?? null })}
            disabled={!hasUnsaved}
          >
            {t('chat-layout:layout.discard')}
          </Button>
          <Button variant="ghost" onClick={reset}>
            <RotateCcw />
            {t('chat-layout:layout.reset')}
          </Button>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs text-muted-foreground">
            {dirty ? t('chat-layout:layout.notApplied') : t('chat-layout:layout.currentLayout')}
          </span>
          <Button asChild variant="outline">
            <a href="#/chat">
              {t('chat-layout:layout.goToChat')}
              <ArrowRight />
            </a>
          </Button>
          <Button onClick={apply} disabled={!canvasValid || !draft.name.trim()}>
            <Check />
            {t('chat-layout:layout.apply')}
          </Button>
        </div>
      </div>
      <p className="text-xs leading-5 text-muted-foreground">{t('chat-layout:layout.localHint')}</p>
      {status && !storageError && (
        <p role="status" className="text-sm leading-6">
          {status}
        </p>
      )}
      {error && (
        <p
          role="alert"
          className="rounded-lg border border-border bg-muted/40 px-3 py-2 text-sm leading-6"
        >
          {error}
        </p>
      )}
      {storageError && (
        <p
          role="alert"
          className="rounded-lg border border-border bg-muted/40 px-3 py-2 text-sm leading-6"
        >
          {t('chat-layout:layout.storageError')}
        </p>
      )}
      <Dialog
        open={pendingLoad !== null}
        onOpenChange={(next) => {
          if (!next) setPendingLoad(null)
        }}
      >
        <DialogContent
          className="min-w-0 grid-cols-[minmax(0,1fr)] overflow-x-hidden"
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            if (returnFocusRef.current?.isConnected) returnFocusRef.current.focus()
            else editorRef.current?.querySelector<HTMLInputElement>('input')?.focus()
          }}
        >
          <DialogHeader>
            <DialogTitle>{t('chat-layout:library.unsavedTitle')}</DialogTitle>
            <DialogDescription>{t('chat-layout:library.unsavedHint')}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setPendingLoad(null)}>
              {t('chat-layout:library.keepEditing')}
            </Button>
            <Button
              onClick={() => {
                if (pendingLoad) load(pendingLoad)
                setPendingLoad(null)
              }}
            >
              {t('chat-layout:library.discardAndSwitch')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
