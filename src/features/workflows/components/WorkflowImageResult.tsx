import { useEffect, useMemo, useRef, useState } from 'react'
import { ChevronLeft, ChevronRight, Download, LoaderCircle, Pause, Play } from 'lucide-react'
import { parseWorkflowImageOutput } from '@shared/workflow/workflow-image-nodes.mjs'
import type { WorkflowImageFrame } from '@shared/workflow/workflow-image-nodes.mjs'
import { AppSelect } from '@/components/common/AppSelect'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { workflowMediaApi } from '@/features/workflows/api/workflow-media-api'
import type { WorkflowTranslate } from '@/features/workflows/model/workflow-templates'

function outputOrNull(value: unknown) {
  try {
    return parseWorkflowImageOutput(value)
  } catch {
    return null
  }
}

function useFrameImage(frame: WorkflowImageFrame | undefined) {
  const [state, setState] = useState<{
    id: string
    image: HTMLImageElement | null
    error: boolean
  }>({ id: '', image: null, error: false })
  const id = frame?.media.id
  useEffect(() => {
    setState({ id: id ?? '', image: null, error: false })
    if (!id) return
    const controller = new AbortController()
    let url = ''
    let image: HTMLImageElement | null = null
    void workflowMediaApi
      .preview(id, controller.signal)
      .then((blob) => {
        if (controller.signal.aborted) return
        url = URL.createObjectURL(blob)
        image = new Image()
        const loaded = image
        image.onload = () => {
          if (!controller.signal.aborted) setState({ id, image: loaded, error: false })
        }
        image.onerror = () => {
          if (!controller.signal.aborted) setState({ id, image: null, error: true })
        }
        image.src = url
      })
      .catch(() => {
        if (!controller.signal.aborted) setState({ id, image: null, error: true })
      })
    return () => {
      controller.abort()
      if (image) {
        image.onload = null
        image.onerror = null
        image.src = ''
      }
      if (url) URL.revokeObjectURL(url)
    }
  }, [id])
  return state.id === id ? state : { id: id ?? '', image: null, error: false }
}

function workflowImageErrorLabel(code: string, t: WorkflowTranslate) {
  if (code === 'workflow_image_source_stale') return t('workflows:imageNodes.sourceStale')
  if (code === 'workflow_image_source_required') return t('workflows:imageNodes.sourceRequired')
  if (code === 'workflow_image_engine_missing') return t('workflows:imageNodes.engineMissing')
  if (code === 'workflow_image_decode_failed') return t('workflows:imageNodes.decodeFailed')
  if (code === 'workflow_image_generation_failed') return t('workflows:imageNodes.generationFailed')
  if (code === 'workflow_image_too_large') return t('workflows:imageNodes.tooLarge')
  if (code === 'workflow_image_cancelled') return t('workflows:imageNodes.cancelled')
  if (code === 'workflow_image_timeout') return t('workflows:imageNodes.timeout')
  return t('workflows:imageNodes.invalidOutput')
}

export function WorkflowImageResult({
  value,
  previousValue,
  error,
  t,
}: {
  value: unknown
  previousValue?: unknown
  error?: string
  t: WorkflowTranslate
}) {
  const output = useMemo(() => outputOrNull(value), [value])
  const input = useMemo(() => outputOrNull(previousValue), [previousValue])
  const [action, setAction] = useState('')
  const [direction, setDirection] = useState('')
  const [index, setIndex] = useState(0)
  const [playing, setPlaying] = useState(false)
  const [onion, setOnion] = useState(false)
  const [original, setOriginal] = useState(false)
  const [downloadError, setDownloadError] = useState(false)
  const [downloading, setDownloading] = useState(false)
  const downloadController = useRef<AbortController | null>(null)
  const canvas = useRef<HTMLCanvasElement>(null)
  const actions = [...new Set(output?.frames.map((frame) => frame.action).filter(Boolean) ?? [])]
  const directions = [
    ...new Set(output?.frames.map((frame) => frame.direction).filter(Boolean) ?? []),
  ]
  const selectedAction = actions.includes(action) ? action : (actions[0] ?? '')
  const selectedDirection = directions.includes(direction) ? direction : (directions[0] ?? '')
  const frames = useMemo(
    () =>
      output?.frames.filter(
        (frame) =>
          (!selectedAction || frame.action === selectedAction) &&
          (!selectedDirection || frame.direction === selectedDirection),
      ) ?? [],
    [output, selectedAction, selectedDirection],
  )
  const current = Math.min(index, Math.max(0, frames.length - 1))
  const referenceFrames = input?.frames.filter(
    (frame) =>
      frame.action === frames[current]?.action && frame.direction === frames[current]?.direction,
  )
  const reference = referenceFrames?.[current] ?? referenceFrames?.[0] ?? input?.frames[0]
  const activeImage = useFrameImage(original ? reference : frames[current])
  const previousImage = useFrameImage(
    onion && !original ? frames[(current + frames.length - 1) % frames.length] : undefined,
  )
  const nextImage = useFrameImage(
    onion && !original ? frames[(current + 1) % frames.length] : undefined,
  )
  useEffect(() => {
    setIndex(0)
    setPlaying(false)
    setOriginal(false)
  }, [selectedAction, selectedDirection, value])
  useEffect(() => () => downloadController.current?.abort(), [])
  useEffect(() => {
    if (!playing || frames.length < 2) return
    const timer = window.setTimeout(() => {
      if (!document.hidden) setIndex((current + 1) % frames.length)
    }, frames[current]?.durationMs ?? 125)
    const pause = () => {
      if (document.hidden) setPlaying(false)
    }
    document.addEventListener('visibilitychange', pause)
    return () => {
      window.clearTimeout(timer)
      document.removeEventListener('visibilitychange', pause)
    }
  }, [playing, frames, current])
  useEffect(() => {
    const context = canvas.current?.getContext('2d')
    if (!context) return
    const width = 512,
      height = 384
    context.clearRect(0, 0, width, height)
    const images = [previousImage.image, activeImage.image, nextImage.image].filter(
      (image): image is HTMLImageElement => Boolean(image),
    )
    const scale =
      Math.min(
        1,
        width / Math.max(1, ...images.map((image) => image.naturalWidth)),
        height / Math.max(1, ...images.map((image) => image.naturalHeight)),
      ) * 0.9
    const draw = (image: HTMLImageElement | null, alpha: number) => {
      if (!image) return
      context.save()
      context.imageSmoothingEnabled = false
      context.globalAlpha = alpha
      const w = image.naturalWidth * scale,
        h = image.naturalHeight * scale
      context.drawImage(image, (width - w) / 2, height * 0.95 - h, w, h)
      context.restore()
    }
    draw(previousImage.image, 0.2)
    draw(nextImage.image, 0.15)
    draw(activeImage.image, 1)
  }, [activeImage.image, previousImage.image, nextImage.image])
  const download = async (kind: 'atlas' | 'json' | 'frame') => {
    if (!output || downloadController.current) return
    const controller = new AbortController()
    downloadController.current = controller
    setDownloading(true)
    setDownloadError(false)
    let url = ''
    try {
      const media = kind === 'atlas' ? output.atlas?.media : frames[current]?.media
      const blob =
        kind === 'json'
          ? new Blob([JSON.stringify(output.atlas ?? output, null, 2)], {
              type: 'application/json',
            })
          : media
            ? await workflowMediaApi.preview(media.id, controller.signal)
            : null
      if (!blob || controller.signal.aborted) return
      url = URL.createObjectURL(blob)
      const anchor = document.createElement('a')
      anchor.href = url
      anchor.download =
        kind === 'json'
          ? 'animation.frames.json'
          : (media?.name ?? 'animation.png').replace(/[\\/:*?"<>|\p{Cc}]/gu, '_')
      anchor.click()
    } catch {
      if (!controller.signal.aborted) setDownloadError(true)
    } finally {
      if (url) URL.revokeObjectURL(url)
      if (!controller.signal.aborted) setDownloading(false)
      if (downloadController.current === controller) downloadController.current = null
    }
  }
  if (!output && !error)
    return (
      <p className="my-3 text-xs text-muted-foreground">{t('workflows:imageNodes.noResult')}</p>
    )
  return (
    <section
      className="my-3 space-y-3 rounded-xl border p-3"
      aria-label={t('workflows:imageNodes.result')}
    >
      <h3 className="text-sm font-medium">{t('workflows:imageNodes.result')}</h3>
      {error && (
        <p role="alert" className="text-xs text-muted-foreground">
          {workflowImageErrorLabel(error, t)}
        </p>
      )}
      {frames.length > 0 && (
        <>
          <div className="grid grid-cols-2 gap-2">
            {actions.length > 0 && (
              <AppSelect
                aria-label={t('workflows:imageNodes.action')}
                value={selectedAction}
                onChange={(event) => setAction(event.target.value)}
              >
                {actions.map((value) => (
                  <option key={value} value={value}>
                    {value}
                  </option>
                ))}
              </AppSelect>
            )}
            {directions.length > 0 && (
              <AppSelect
                aria-label={t('workflows:imageNodes.directions')}
                value={selectedDirection}
                onChange={(event) => setDirection(event.target.value)}
              >
                {directions.map((value) => (
                  <option key={value} value={value}>
                    {value}
                  </option>
                ))}
              </AppSelect>
            )}
          </div>
          <div className="relative overflow-hidden rounded-lg border bg-[repeating-conic-gradient(#d4d4d8_0%_25%,#fafafa_0%_50%)] bg-size-[16px_16px] dark:bg-[repeating-conic-gradient(#27272a_0%_25%,#3f3f46_0%_50%)]">
            <canvas
              width={512}
              height={384}
              ref={canvas}
              className="w-full"
              role="img"
              aria-label={t('workflows:imageNodes.frameAt', { index: current + 1 })}
            />
            {!activeImage.image && !activeImage.error && (
              <LoaderCircle className="absolute top-1/2 left-1/2 size-5 animate-spin" />
            )}
          </div>
          {activeImage.error && (
            <p role="alert" className="text-xs">
              {t('workflows:inputs.previewFailed')}
            </p>
          )}
          <div className="flex flex-wrap items-center justify-center gap-1">
            <Button
              size="icon-sm"
              variant="outline"
              aria-label={t('workflows:imageNodes.previousFrame')}
              disabled={frames.length < 2}
              onClick={() => {
                setPlaying(false)
                setIndex((current + frames.length - 1) % frames.length)
              }}
            >
              <ChevronLeft />
            </Button>
            <Button
              size="icon-sm"
              variant="outline"
              aria-label={
                playing ? t('workflows:imageNodes.pause') : t('workflows:imageNodes.play')
              }
              disabled={frames.length < 2}
              onClick={() => setPlaying(!playing)}
            >
              {playing ? <Pause /> : <Play />}
            </Button>
            <Button
              size="icon-sm"
              variant="outline"
              aria-label={t('workflows:imageNodes.nextFrame')}
              disabled={frames.length < 2}
              onClick={() => {
                setPlaying(false)
                setIndex((current + 1) % frames.length)
              }}
            >
              <ChevronRight />
            </Button>
            <span className="px-1 text-xs tabular-nums">
              {current + 1} / {frames.length}
            </span>
          </div>
          <input
            className="w-full"
            type="range"
            min={0}
            max={Math.max(0, frames.length - 1)}
            value={current}
            aria-label={t('workflows:imageNodes.frameNumber')}
            onChange={(event) => {
              setPlaying(false)
              setIndex(Number(event.target.value))
            }}
          />
          <label className="flex items-center justify-between text-xs">
            {t('workflows:imageNodes.onionSkin')}
            <Switch checked={onion} onCheckedChange={setOnion} />
          </label>
          {reference && (
            <label className="flex items-center justify-between text-xs">
              {t('workflows:imageNodes.showInput')}
              <Switch checked={original} onCheckedChange={setOriginal} />
            </label>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={downloading}
              onClick={() => void download('frame')}
            >
              <Download />
              {t('workflows:imageNodes.saveFrame')}
            </Button>
            {output?.atlas && (
              <Button
                size="sm"
                variant="outline"
                disabled={downloading}
                onClick={() => void download('atlas')}
              >
                <Download />
                PNG
              </Button>
            )}
            <Button
              size="sm"
              variant="outline"
              disabled={downloading}
              onClick={() => void download('json')}
            >
              <Download />
              JSON
            </Button>
          </div>
        </>
      )}
      {downloadError && (
        <p role="alert" className="text-xs">
          {t('workflows:imageNodes.downloadFailed')}
        </p>
      )}
    </section>
  )
}
