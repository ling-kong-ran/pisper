import { useEffect, useRef, useState } from 'react'
import { BookOpen, CheckCircle2, Download, LoaderCircle, Play, Square } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Progress } from '@/components/ui/progress'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { playLocalSpeech } from './speech-output'
import { SpeechResourceNoticesDialog } from './SpeechResourceNoticesDialog'
import { hasSpeechResourceNotices } from './speech-resource-notices'
import type { useSpeechModels } from './use-speech-models'
import type { LocalSpeechModel } from './speech-models'

type Manager = ReturnType<typeof useSpeechModels>
const size = (bytes: number) =>
  `${new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(bytes / 1_048_576)} MiB`

export function SpeechModelsDialog({
  manager,
  onClose,
}: {
  manager: Manager
  onClose: () => void
}) {
  const { t } = useI18n()
  const [previewing, setPreviewing] = useState(false)
  const [previewError, setPreviewError] = useState('')
  const [noticesModel, setNoticesModel] = useState<LocalSpeechModel | null>(null)
  const preview = useRef<AbortController | null>(null)
  const noticesTrigger = useRef<HTMLButtonElement | null>(null)
  const stopPreview = () => {
    preview.current?.abort()
    preview.current = null
    setPreviewing(false)
  }
  useEffect(() => {
    if (!manager.open) {
      preview.current?.abort()
      preview.current = null
      setPreviewing(false)
      setNoticesModel(null)
    }
    return () => {
      preview.current?.abort()
      preview.current = null
    }
  }, [manager.open])
  const statuses: Record<LocalSpeechModel['status'], string> = {
    'not-installed': t('chat:speechModels.notInstalled'),
    downloading: t('chat:speechModels.downloading'),
    verifying: t('chat:speechModels.verifying'),
    installed: t('chat:speechModels.installed'),
    cancelled: t('chat:speechModels.cancelled'),
    error: t('chat:speechModels.failed'),
  }
  const voices = manager.models.flatMap((model) => model.voices || [])
  const selectedVoice = voices.find((voice) => voice.id === manager.selectedVoice)
  const ttsReady = manager.models.some(
    (model) => model.kind === 'tts' && model.status === 'installed',
  )
  const missing = manager.models.filter((model) => model.status !== 'installed')
  const busy = missing.some((model) => ['downloading', 'verifying'].includes(model.status))
  const close = () => {
    setNoticesModel(null)
    stopPreview()
    manager.close()
    onClose()
  }
  const previewVoice = async () => {
    if (preview.current) {
      stopPreview()
      return
    }
    if (!selectedVoice || !ttsReady || missing.length) return
    const controller = new AbortController()
    preview.current = controller
    setPreviewing(true)
    setPreviewError('')
    try {
      await playLocalSpeech(
        selectedVoice.language === 'en'
          ? t('chat:speechModels.previewEnglish')
          : t('chat:speechModels.previewChinese'),
        selectedVoice.id,
        controller.signal,
      )
    } catch (caught) {
      if (!controller.signal.aborted)
        setPreviewError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      if (preview.current === controller) {
        preview.current = null
        setPreviewing(false)
      }
    }
  }

  return (
    <Dialog
      open={manager.open}
      onOpenChange={(open) => {
        if (!open) close()
      }}
    >
      <DialogContent
        className="z-[110] max-h-[85dvh] overflow-y-auto rounded-lg sm:max-w-md"
        overlayClassName="z-[109]"
        onEscapeKeyDown={(event) => event.stopPropagation()}
      >
        <DialogHeader>
          <DialogTitle>{t('chat:speechModels.title')}</DialogTitle>
          <DialogDescription className="sr-only">{t('chat:speechModels.title')}</DialogDescription>
        </DialogHeader>
        {!manager.catalog && !manager.error && (
          <LoaderCircle
            className="mx-auto my-4 size-5 animate-spin"
            aria-label={t('chat:speechModels.loading')}
          />
        )}
        <div className="min-w-0 divide-y">
          {manager.models.map((model) => {
            const downloading = ['downloading', 'verifying'].includes(model.status)
            const percent = model.totalBytes
              ? Math.min(100, Math.floor((model.downloadedBytes / model.totalBytes) * 100))
              : 0
            const licenseUrl = model.license?.url.startsWith('https://')
              ? model.license.url
              : undefined
            return (
              <section
                key={model.id}
                className="min-w-0 space-y-2 py-3 first:pt-0"
                aria-label={model.name}
              >
                <div className="flex min-w-0 items-center gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="text-xs text-muted-foreground">
                      {model.kind === 'asr'
                        ? t('chat:speechModels.recognition')
                        : t('chat:speechModels.synthesis')}
                    </div>
                    <div className="break-words font-medium">{model.name}</div>
                  </div>
                  {model.status === 'installed' ? (
                    <CheckCircle2
                      className="size-5 shrink-0 text-emerald-600 dark:text-emerald-400"
                      aria-label={statuses.installed}
                    />
                  ) : (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Button
                          size="icon-sm"
                          variant="outline"
                          disabled={manager.loading}
                          onClick={() => {
                            void (downloading
                              ? manager.cancelDownload(model.id)
                              : manager.download(model.id))
                          }}
                          aria-label={
                            downloading
                              ? t('chat:speechModels.cancelDownload')
                              : t('chat:speechModels.download')
                          }
                        >
                          {downloading ? <Square /> : <Download />}
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent>
                        {downloading
                          ? t('chat:speechModels.cancelDownload')
                          : t('chat:speechModels.download')}
                      </TooltipContent>
                    </Tooltip>
                  )}
                </div>
                <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1 text-xs text-muted-foreground">
                  <span>{statuses[model.status]}</span>
                  <span className="tabular-nums">
                    {downloading
                      ? `${size(model.downloadedBytes)} / ${size(model.totalBytes)}`
                      : size(model.totalBytes)}
                  </span>
                </div>
                {downloading && (
                  <Progress
                    value={percent}
                    aria-label={model.name}
                    aria-valuenow={percent}
                    aria-valuemin={0}
                    aria-valuemax={100}
                  />
                )}
                {model.error && (
                  <p role="alert" className="break-words text-xs text-destructive">
                    {model.error}
                  </p>
                )}
                {model.license && (
                  <div className="flex min-w-0 items-center justify-between gap-2">
                    <a
                      href={licenseUrl}
                      target="_blank"
                      rel="noreferrer"
                      className="block break-words text-[11px] text-muted-foreground underline underline-offset-2"
                    >
                      {model.license.name}
                    </a>
                    {hasSpeechResourceNotices(model.id) && (
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button
                            size="icon-sm"
                            variant="ghost"
                            aria-label={t('chat:speechModels.notices')}
                            onClick={(event) => {
                              noticesTrigger.current = event.currentTarget
                              stopPreview()
                              setNoticesModel(model)
                            }}
                          >
                            <BookOpen />
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent>{t('chat:speechModels.notices')}</TooltipContent>
                      </Tooltip>
                    )}
                  </div>
                )}
              </section>
            )
          })}
        </div>
        {voices.length > 0 && (
          <div className="space-y-2 border-t pt-3">
            <div className="flex min-w-0 items-center gap-2">
              <span className="min-w-0 flex-1 text-sm">{t('chat:speechModels.voice')}</span>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    size="icon"
                    variant="outline"
                    disabled={!ttsReady || missing.length > 0}
                    onClick={() => void previewVoice()}
                    aria-label={
                      previewing
                        ? t('chat:speechModels.stopPreview')
                        : t('chat:speechModels.preview')
                    }
                  >
                    {previewing ? <Square /> : <Play />}
                  </Button>
                </TooltipTrigger>
                <TooltipContent>
                  {previewing ? t('chat:speechModels.stopPreview') : t('chat:speechModels.preview')}
                </TooltipContent>
              </Tooltip>
            </div>
          </div>
        )}
        {(manager.error || previewError) && (
          <p role="alert" className="break-words text-sm text-destructive">
            {manager.error || previewError}
          </p>
        )}
        <div className="flex justify-end border-t pt-3">
          {missing.length ? (
            <Button disabled={manager.loading || busy} onClick={() => void manager.downloadAll()}>
              <Download />
              {t('chat:speechModels.downloadAll')}
            </Button>
          ) : (
            <Button disabled={!manager.catalog} onClick={close}>
              {t('chat:speechModels.done')}
            </Button>
          )}
        </div>
      </DialogContent>
      {manager.open && noticesModel && (
        <SpeechResourceNoticesDialog
          modelId={noticesModel.id}
          modelName={noticesModel.name}
          returnFocus={noticesTrigger}
          onClose={() => setNoticesModel(null)}
        />
      )}
    </Dialog>
  )
}
