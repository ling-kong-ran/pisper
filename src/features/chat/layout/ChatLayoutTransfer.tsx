import { useEffect, useId, useRef, useState } from 'react'
import { Copy, Download, Upload } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import {
  CHAT_LAYOUT_MAX_BYTES,
  parseChatLayoutJson,
  serializeChatLayout,
  type ChatLayoutTemplate,
} from './chat-layout'
import { layoutTransferErrorLabel } from './chat-layout-transfer'

export function ChatLayoutImportDialog({
  onImport,
  triggerLabel,
  confirmLabel,
  description,
  disabled,
  mode = 'preview',
}: {
  onImport: (template: ChatLayoutTemplate) => void
  triggerLabel?: string
  confirmLabel?: string
  description?: string
  disabled?: boolean
  mode?: 'preview' | 'apply'
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [json, setJson] = useState('')
  const [error, setError] = useState('')
  const [reading, setReading] = useState(false)
  const fileRef = useRef<HTMLInputElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)
  const generationRef = useRef(0)
  const id = useId()
  useEffect(
    () => () => {
      generationRef.current += 1
    },
    [],
  )
  const changeOpen = (next: boolean) => {
    generationRef.current += 1
    setOpen(next)
    setError('')
    setReading(false)
    if (!next) setJson('')
  }
  const accept = (text: string) => {
    try {
      const template = parseChatLayoutJson(text)
      onImport(template)
      changeOpen(false)
    } catch (reason) {
      setError(layoutTransferErrorLabel(reason, t))
    }
  }
  const read = async (file: File) => {
    const generation = ++generationRef.current
    setError('')
    if (file.size > CHAT_LAYOUT_MAX_BYTES) {
      setError(t('chat-layout:layout.errorTooLarge'))
      return
    }
    setReading(true)
    try {
      const text = await file.text()
      if (generation === generationRef.current) accept(text)
    } catch {
      if (generation === generationRef.current) setError(t('chat-layout:layout.errorRead'))
    } finally {
      if (generation === generationRef.current) setReading(false)
    }
  }
  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogTrigger asChild>
        <Button variant="outline" disabled={disabled}>
          <Upload />
          {triggerLabel ?? t('chat-layout:layout.import')}
        </Button>
      </DialogTrigger>
      <DialogContent
        ref={contentRef}
        className="min-w-0 grid-cols-[minmax(0,1fr)] overflow-x-hidden sm:max-w-xl"
        onCloseAutoFocus={(event) => {
          // 导入后可能打开“保留草稿”确认框，关闭旧弹窗时不要抢回新弹窗的焦点。
          const activeDialog = document.activeElement?.closest('[role="dialog"]')
          if (activeDialog && activeDialog !== contentRef.current) event.preventDefault()
        }}
      >
        <DialogHeader className="min-w-0">
          <DialogTitle>{t('chat-layout:layout.importTitle')}</DialogTitle>
          <DialogDescription className="[overflow-wrap:anywhere]">
            {description ??
              (mode === 'apply'
                ? t('chat-layout:transfer.importApplyHint')
                : t('chat-layout:transfer.importHint'))}
          </DialogDescription>
        </DialogHeader>
        <input
          ref={fileRef}
          type="file"
          accept=".json,application/json"
          className="hidden"
          aria-label={t('chat-layout:layout.chooseFile')}
          onChange={(event) => {
            const file = event.currentTarget.files?.[0]
            event.currentTarget.value = ''
            if (file) void read(file)
          }}
        />
        <Button variant="outline" disabled={reading} onClick={() => fileRef.current?.click()}>
          <Upload />
          {reading ? t('chat-layout:layout.readingFile') : t('chat-layout:layout.chooseFile')}
        </Button>
        <div className="min-w-0 space-y-2">
          <Label htmlFor={id}>{t('chat-layout:layout.pasteJson')}</Label>
          <Textarea
            id={id}
            value={json}
            disabled={reading}
            maxLength={CHAT_LAYOUT_MAX_BYTES}
            spellCheck={false}
            onChange={(event) => {
              setJson(event.currentTarget.value)
              setError('')
            }}
            className="h-52 max-h-[45dvh] min-w-0 w-full resize-y field-sizing-fixed font-mono text-xs [overflow-wrap:anywhere]"
            placeholder={t('chat-layout:layout.jsonPlaceholder')}
          />
        </div>
        {error && (
          <p
            role="alert"
            className="min-w-0 rounded-lg border border-border bg-muted/40 px-3 py-2 text-sm leading-6 [overflow-wrap:anywhere]"
          >
            {error}
          </p>
        )}
        <DialogFooter className="min-w-0">
          <Button variant="outline" onClick={() => changeOpen(false)}>
            {t('chat-layout:layout.cancel')}
          </Button>
          <Button disabled={!json.trim() || reading} onClick={() => accept(json)}>
            {confirmLabel ??
              (mode === 'apply'
                ? t('chat-layout:transfer.importApply')
                : t('chat-layout:layout.importToPreview'))}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function ChatLayoutExportDialog({
  template,
  disabled = false,
  triggerLabel,
}: {
  template: ChatLayoutTemplate
  disabled?: boolean
  triggerLabel?: string
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [json, setJson] = useState('')
  const [error, setError] = useState('')
  const [status, setStatus] = useState('')
  const [copying, setCopying] = useState(false)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const urlsRef = useRef(new Set<string>())
  const generationRef = useRef(0)
  const id = useId()
  useEffect(() => {
    const urls = urlsRef.current
    return () => {
      generationRef.current += 1
      for (const url of urls) URL.revokeObjectURL(url)
      urls.clear()
    }
  }, [])
  const changeOpen = (next: boolean) => {
    generationRef.current += 1
    setError('')
    setStatus('')
    setCopying(false)
    if (next) {
      try {
        setJson(serializeChatLayout(template))
      } catch (reason) {
        setJson('')
        setError(layoutTransferErrorLabel(reason, t))
      }
    }
    setOpen(next)
  }
  const download = () => {
    let link: HTMLAnchorElement | undefined
    try {
      const url = URL.createObjectURL(new Blob([json], { type: 'application/json;charset=utf-8' }))
      urlsRef.current.add(url)
      link = document.createElement('a')
      link.href = url
      link.download = 'pisper-chat-layout.json'
      document.body.append(link)
      link.click()
      window.setTimeout(() => {
        URL.revokeObjectURL(url)
        urlsRef.current.delete(url)
      }, 0)
      setError('')
      setStatus(t('chat-layout:transfer.downloadRequested'))
    } catch {
      setError(t('chat-layout:transfer.downloadError'))
    } finally {
      link?.remove()
    }
  }
  const copy = async () => {
    const generation = generationRef.current
    setCopying(true)
    setError('')
    setStatus('')
    try {
      if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable')
      await navigator.clipboard.writeText(json)
      if (generation === generationRef.current) setStatus(t('chat-layout:transfer.copied'))
    } catch {
      if (generation === generationRef.current) {
        setError(t('chat-layout:transfer.copyError'))
        textareaRef.current?.focus()
        textareaRef.current?.select()
      }
    } finally {
      if (generation === generationRef.current) setCopying(false)
    }
  }
  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogTrigger asChild>
        <Button variant="outline" disabled={disabled}>
          <Download />
          {triggerLabel ?? t('chat-layout:layout.export')}
        </Button>
      </DialogTrigger>
      <DialogContent className="min-w-0 grid-cols-[minmax(0,1fr)] overflow-x-hidden sm:max-w-xl">
        <DialogHeader className="min-w-0">
          <DialogTitle>{t('chat-layout:transfer.exportTitle')}</DialogTitle>
          <DialogDescription className="[overflow-wrap:anywhere]">
            {t('chat-layout:transfer.exportHint')}
          </DialogDescription>
        </DialogHeader>
        <div className="min-w-0 space-y-2">
          <Label htmlFor={id}>{t('chat-layout:transfer.jsonContent')}</Label>
          <Textarea
            ref={textareaRef}
            id={id}
            value={json}
            readOnly
            spellCheck={false}
            className="h-56 max-h-[45dvh] min-w-0 w-full resize-y field-sizing-fixed font-mono text-xs [overflow-wrap:anywhere]"
          />
        </div>
        {error && (
          <p
            role="alert"
            className="min-w-0 rounded-lg border border-border bg-muted/40 px-3 py-2 text-sm leading-6 [overflow-wrap:anywhere]"
          >
            {error}
          </p>
        )}
        {status && (
          <p role="status" className="text-sm leading-6">
            {status}
          </p>
        )}
        <DialogFooter className="min-w-0">
          <Button variant="outline" disabled={!json || copying} onClick={() => void copy()}>
            <Copy />
            {copying ? t('chat-layout:transfer.copying') : t('chat-layout:transfer.copy')}
          </Button>
          <Button disabled={!json} onClick={download}>
            <Download />
            {t('chat-layout:transfer.download')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
