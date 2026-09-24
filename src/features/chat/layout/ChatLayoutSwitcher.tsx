import { useState } from 'react'
import { Check, ChevronDown, LayoutTemplate, Settings2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import { cn } from '@/lib/utils'
import { CHAT_LAYOUT_PRESETS, equalChatLayoutContent, type ChatLayoutTemplate } from './chat-layout'
import { useChatLayoutStore } from './chat-layout-store'
import { chatLayoutPresetLabels } from './chat-layout-preset-labels'
import { ChatLayoutExportDialog, ChatLayoutImportDialog } from './ChatLayoutTransfer'
import { layoutTransferErrorLabel } from './chat-layout-transfer'

export function ChatLayoutSwitcher({ notify, onManage }: { notify: Notify; onManage: () => void }) {
  const { t } = useI18n()
  const active = useChatLayoutStore((state) => state.active)
  const saved = useChatLayoutStore((state) => state.saved)
  const storageError = useChatLayoutStore((state) => state.storageError)
  const [open, setOpen] = useState(false)
  const [error, setError] = useState('')
  const labels = chatLayoutPresetLabels(t)
  const currentSaved = saved.find(
    ({ template }) => template.name === active.name && equalChatLayoutContent(template, active),
  )
  const currentPreset = currentSaved
    ? undefined
    : CHAT_LAYOUT_PRESETS.find(({ template }) => equalChatLayoutContent(template, active))
  const activeName =
    currentSaved?.template.name ?? (currentPreset ? labels[currentPreset.id].title : active.name)
  const apply = (template: ChatLayoutTemplate) => {
    useChatLayoutStore.getState().apply(template)
    setOpen(false)
    setError('')
    notify(t('chat-layout:switcher.applied', { name: template.name }))
  }
  const select = (template: ChatLayoutTemplate) => {
    try {
      apply(template)
    } catch (reason) {
      setError(layoutTransferErrorLabel(reason, t))
    }
  }
  const importAndApply = (template: ChatLayoutTemplate) => {
    const store = useChatLayoutStore.getState()
    const id = store.importTemplate(template)
    const imported = useChatLayoutStore.getState().saved.find((entry) => entry.id === id)
    // 导入是同一 Store 的同步事务，只应用实际保存的名称（重名时可能加序号）。
    if (imported) apply(imported.template)
  }
  return (
    <Dialog
      open={open}
      onOpenChange={(value) => {
        setOpen(value)
        setError('')
      }}
    >
      <DialogTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          className="h-[34px] min-w-[34px] gap-1.5 px-2.5 max-[650px]:px-2"
          aria-label={t('chat-layout:switcher.label')}
          title={`${t('chat-layout:switcher.label')} · ${activeName}`}
        >
          <LayoutTemplate className="size-4" />
          <span className="max-w-28 truncate max-[650px]:hidden">{activeName}</span>
          <ChevronDown className="size-3 max-[650px]:hidden" />
        </Button>
      </DialogTrigger>
      <DialogContent className="min-w-0 grid-cols-[minmax(0,1fr)] sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t('chat-layout:switcher.label')}</DialogTitle>
          <DialogDescription>{t('chat-layout:switcher.scope')}</DialogDescription>
          <p className="break-words text-xs text-muted-foreground">
            {t('chat-layout:switcher.current')} · {activeName}
          </p>
        </DialogHeader>
        <section className="min-w-0 space-y-2" aria-label={t('chat-layout:switcher.builtIn')}>
          <h3 className="text-xs font-medium text-muted-foreground">
            {t('chat-layout:switcher.builtIn')}
          </h3>
          <div className="grid grid-cols-2 gap-2">
            {[...CHAT_LAYOUT_PRESETS]
              .sort((a, b) => Number(b.id === 'studio') - Number(a.id === 'studio'))
              .map((preset) => {
                const selected = currentPreset?.id === preset.id
                return (
                  <button
                    key={preset.id}
                    type="button"
                    aria-pressed={selected}
                    onClick={() => select({ ...preset.template, name: labels[preset.id].title })}
                    className={cn(
                      'relative min-w-0 rounded-xl border p-3 text-left transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring',
                      selected
                        ? 'border-foreground/35 bg-muted/60'
                        : 'border-border hover:bg-muted/40',
                    )}
                  >
                    <div
                      aria-hidden
                      className="mb-3 flex h-14 gap-1.5 rounded-lg bg-muted/60 p-1.5"
                    >
                      {preset.id !== 'focus' && <div className="w-3 rounded bg-foreground/10" />}
                      {preset.id === 'workbench' && (
                        <div className="w-5 rounded bg-foreground/10" />
                      )}
                      <div className="flex flex-1 flex-col justify-between rounded bg-background p-1.5">
                        <div className="h-1 w-2/3 rounded bg-foreground/15" />
                        <div className="h-2.5 rounded border border-foreground/15" />
                      </div>
                      {(preset.id === 'studio' || preset.id === 'default') && (
                        <div className="w-5 rounded bg-background" />
                      )}
                    </div>
                    <span className="block pr-4 text-sm font-medium">
                      {labels[preset.id].title}
                    </span>
                    <span className="mt-1 block pr-4 text-xs leading-5 text-muted-foreground">
                      {labels[preset.id].description}
                    </span>
                    {selected && <Check className="absolute right-3 bottom-3 size-4" />}
                  </button>
                )
              })}
          </div>
        </section>
        {saved.length > 0 && (
          <section className="min-w-0 space-y-2" aria-label={t('chat-layout:switcher.saved')}>
            <h3 className="text-xs font-medium text-muted-foreground">
              {t('chat-layout:switcher.saved')}
            </h3>
            <div className="max-h-40 overflow-y-auto rounded-lg border border-border p-1">
              {saved.map((entry) => (
                <button
                  key={entry.id}
                  type="button"
                  aria-pressed={currentSaved?.id === entry.id}
                  onClick={() => select(entry.template)}
                  className="flex min-h-11 w-full min-w-0 items-center gap-2 rounded-md px-3 text-left text-sm hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring"
                >
                  <span className="min-w-0 flex-1 truncate">{entry.template.name}</span>
                  {currentSaved?.id === entry.id && <Check className="size-4 shrink-0" />}
                </button>
              ))}
            </div>
          </section>
        )}
        {error && (
          <p role="alert" className="text-sm leading-6">
            {error}
          </p>
        )}
        {storageError && (
          <p role="status" className="text-xs leading-5 text-muted-foreground">
            {t('chat-layout:layout.storageError')}
          </p>
        )}
        <div className="flex flex-wrap items-center gap-2 border-t border-border pt-3">
          <ChatLayoutImportDialog mode="apply" onImport={importAndApply} />
          <ChatLayoutExportDialog template={active} />
          <Button
            variant="ghost"
            className="ml-auto"
            onClick={() => {
              setOpen(false)
              onManage()
            }}
          >
            <Settings2 />
            {t('chat-layout:switcher.manage')}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
