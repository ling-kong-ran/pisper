import { useEffect, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowDown, ArrowUp, Download, LoaderCircle, Plus, Trash2 } from 'lucide-react'
import {
  normalizeWorkflowImageSettings,
  WORKFLOW_IMAGE_DIRECTIONS,
} from '@shared/workflow/workflow-image-nodes.mjs'
import type {
  WorkflowFrameTransform,
  WorkflowImageSettings,
} from '@shared/workflow/workflow-image-nodes.mjs'
import { AppSelect } from '@/components/common/AppSelect'
import { ContentInput } from '@/components/app/ContentInput'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { workflowImageApi } from '@/features/workflows/api/workflow-image-api'
import { spriteEnginesApi, SPRITE_ENGINES_QUERY_KEY } from '@/features/workflows/api/workflow-image-engine-api'
import type { WorkflowInput, WorkflowNode } from '@/features/workflows/model/types'
import type { WorkflowTranslate } from '@/features/workflows/model/workflow-templates'

function NumericField({
  label,
  value,
  min,
  max,
  step = 1,
  onChange,
}: {
  label: string
  value: number
  min: number
  max: number
  step?: number
  onChange: (value: number) => void
}) {
  return (
    <FieldLabel variant="control">
      {label}
      <Input
        type="number"
        value={value}
        min={min}
        max={max}
        step={step}
        onChange={(event) => {
          if (!event.target.value) return
          const next = event.target.valueAsNumber
          if (Number.isFinite(next)) onChange(Math.max(min, Math.min(max, next)))
        }}
      />
    </FieldLabel>
  )
}

function ModelDownload({
  t,
  id = 'background',
}: {
  t: WorkflowTranslate
  id?: 'background' | 'inpaint'
}) {
  const client = useQueryClient()
  const [error, setError] = useState(false)
  const [busy, setBusy] = useState(false)
  const controller = useRef<AbortController | null>(null)
  useEffect(() => () => controller.current?.abort(), [])
  const catalog = useQuery({
    queryKey: SPRITE_ENGINES_QUERY_KEY,
    queryFn: ({ signal }) => spriteEnginesApi.catalog(signal),
    refetchInterval: (query) =>
      query.state.data?.engines.some((engine) => engine.status === 'downloading') ? 1000 : false,
  })
  const engine = catalog.data?.engines.find((item) => item.id === id)
  const download = async () => {
    if (controller.current) return
    const next = new AbortController()
    controller.current = next
    setBusy(true)
    setError(false)
    try {
      client.setQueryData(
        SPRITE_ENGINES_QUERY_KEY,
        await spriteEnginesApi.download(id, next.signal),
      )
    } catch {
      if (!next.signal.aborted) setError(true)
    } finally {
      if (!next.signal.aborted) setBusy(false)
      if (controller.current === next) controller.current = null
    }
  }
  return (
    <div className="space-y-2 rounded-lg border p-3 text-xs">
      <p>{t('workflows:imageNodes.modelHint')}</p>
      <p>
        {engine?.status === 'ready'
          ? t('workflows:imageNodes.engineReady')
          : engine?.status === 'downloading'
            ? t('workflows:imageNodes.engineDownloading', {
                percent: Math.round(engine.total ? (100 * engine.received) / engine.total : 0),
              })
            : t('workflows:imageNodes.engineMissing')}
      </p>
      {(error || catalog.isError || engine?.status === 'failed') && (
        <p role="alert">{t('workflows:imageNodes.engineFailed')}</p>
      )}
      {engine?.status !== 'ready' && (
        <Button
          size="sm"
          variant="outline"
          disabled={busy || engine?.status === 'downloading'}
          onClick={() => void download()}
        >
          {busy || engine?.status === 'downloading' ? (
            <LoaderCircle className="animate-spin" />
          ) : (
            <Download />
          )}
          {t('workflows:imageNodes.download')}
        </Button>
      )}
    </div>
  )
}

export function WorkflowImageNodeInspector({
  node,
  inputs,
  t,
  onChange,
}: {
  node: WorkflowNode
  inputs: WorkflowInput[]
  t: WorkflowTranslate
  onChange: (patch: Partial<WorkflowNode>) => void
}) {
  const settings = normalizeWorkflowImageSettings(node.image)
  const update = (patch: Partial<WorkflowImageSettings>) =>
    onChange({ image: { ...settings, ...patch } })
  const models = useQuery({
    queryKey: ['workflows', 'image-models'],
    queryFn: ({ signal }) => workflowImageApi.models(signal),
    enabled: node.kind === 'media-generate',
  })
  const [frameIndex, setFrameIndex] = useState(0)
  useEffect(() => setFrameIndex(0), [node.id])
  const transform = settings.transforms.find((item) => item.index === frameIndex) ?? {
    index: frameIndex,
    x: 0,
    y: 0,
    rotation: 0,
    scale: 1,
    opacity: 1,
    durationMs: settings.durationMs,
    enabled: true,
  }
  const updateTransform = (patch: Partial<WorkflowFrameTransform>) =>
    update({
      transforms: [
        ...settings.transforms.filter((item) => item.index !== frameIndex),
        { ...transform, ...patch },
      ],
    })
  const order = settings.frameOrder
  const move = (index: number, delta: number) => {
    const next = [...order]
    const target = index + delta
    if (target < 0 || target >= next.length) return
    ;[next[index], next[target]] = [next[target], next[index]]
    update({ frameOrder: next })
  }
  return (
    <div className="my-3 space-y-3 border-y py-3">
      {node.kind === 'media-input' && (
        <FieldLabel variant="control">
          {t('workflows:imageNodes.inputField')}
          <AppSelect
            value={settings.inputName}
            onChange={(event) => update({ inputName: event.target.value })}
          >
            {!inputs.some(
              (input) => input.type === 'image' && input.name === settings.inputName,
            ) && (
              <option value={settings.inputName}>{t('workflows:imageNodes.chooseInput')}</option>
            )}
            {inputs
              .filter((input) => input.type === 'image')
              .map((input) => (
                <option key={input.id} value={input.name}>
                  {input.label}
                </option>
              ))}
          </AppSelect>
          <small>{t('workflows:imageNodes.inputHint')}</small>
        </FieldLabel>
      )}
      {node.kind === 'media-generate' && (
        <>
          <FieldLabel variant="control">
            {t('workflows:imageNodes.imageModel')}
            <AppSelect
              value={node.model ? `${node.model.provider}/${node.model.model}` : ''}
              onChange={(event) => {
                const model = models.data?.find((item) => item.id === event.target.value)
                onChange({
                  model: model
                    ? {
                        provider: model.providerId,
                        model: model.id.slice(model.providerId.length + 1),
                      }
                    : null,
                })
              }}
            >
              <option value="">{t('workflows:imageNodes.defaultImageModel')}</option>
              {models.data?.map((model) => (
                <option key={model.id} value={model.id}>
                  {model.providerName} / {model.name}
                </option>
              ))}
            </AppSelect>
            {models.isError && <small role="alert">{t('workflows:imageNodes.modelsFailed')}</small>}
          </FieldLabel>
          <FieldLabel variant="control">
            {t('workflows:imageNodes.action')}
            <Input
              value={settings.action}
              maxLength={160}
              onChange={(event) => update({ action: event.target.value })}
            />
          </FieldLabel>
          <ContentInput
            label={t('workflows:imageNodes.prompt')}
            value={node.prompt}
            onValueChange={(prompt) => onChange({ prompt })}
            allowFiles={false}
            maxLength={16000}
            attachmentLabel={t('workflows:inputs.chooseImage')}
            removeLabel={t('workflows:inputs.removeMedia')}
          />
          <NumericField
            label={t('workflows:imageNodes.frameCount')}
            value={settings.frameCount}
            min={1}
            max={16}
            onChange={(frameCount) => update({ frameCount })}
          />
          <fieldset className="space-y-2">
            <legend className="text-sm">{t('workflows:imageNodes.directions')}</legend>
            <div className="grid grid-cols-4 gap-1">
              {WORKFLOW_IMAGE_DIRECTIONS.map((direction) => (
                <Button
                  key={direction}
                  size="sm"
                  variant={settings.directions.includes(direction) ? 'secondary' : 'outline'}
                  aria-pressed={settings.directions.includes(direction)}
                  disabled={
                    settings.directions.length === 1 && settings.directions[0] === direction
                  }
                  onClick={() =>
                    update({
                      directions: settings.directions.includes(direction)
                        ? settings.directions.filter((item) => item !== direction)
                        : WORKFLOW_IMAGE_DIRECTIONS.filter(
                            (item) => settings.directions.includes(item) || item === direction,
                          ),
                    })
                  }
                >
                  {direction}
                </Button>
              ))}
            </div>
          </fieldset>
          <p className="text-xs text-muted-foreground">
            {t('workflows:imageNodes.generateHint', { count: settings.directions.length })}
          </p>
        </>
      )}
      {node.kind === 'media-background' && (
        <>
          <FieldLabel variant="control">
            {t('workflows:imageNodes.method')}
            <AppSelect
              value={settings.method}
              onChange={(event) =>
                update({ method: event.target.value === 'model' ? 'model' : 'color' })
              }
            >
              <option value="color">{t('workflows:imageNodes.colorKey')}</option>
              <option value="model">{t('workflows:imageNodes.localModel')}</option>
            </AppSelect>
          </FieldLabel>
          {settings.method === 'model' ? (
            <ModelDownload t={t} />
          ) : (
            <>
              <div className="space-y-2">
                <p className="text-sm">{t('workflows:imageNodes.colors')}</p>
                <p className="text-xs text-muted-foreground">
                  {t('workflows:imageNodes.colorsHint')}
                </p>
                <div className="flex flex-wrap gap-2">
                  {settings.colors.map((color, index) => (
                    <div className="flex items-center gap-1" key={index}>
                      <input
                        className="h-8 w-10 cursor-pointer rounded border"
                        type="color"
                        value={color}
                        aria-label={t('workflows:imageNodes.colorAt', { index: index + 1 })}
                        onChange={(event) =>
                          update({
                            colors: settings.colors.map((item, current) =>
                              current === index ? event.target.value : item,
                            ),
                          })
                        }
                      />
                      <Button
                        size="icon-xs"
                        variant="ghost"
                        aria-label={t('workflows:imageNodes.removeColor')}
                        onClick={() =>
                          update({
                            colors: settings.colors.filter((_, current) => current !== index),
                          })
                        }
                      >
                        <Trash2 />
                      </Button>
                    </div>
                  ))}
                </div>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={settings.colors.length >= 8}
                  onClick={() => update({ colors: [...settings.colors, '#00ff00'] })}
                >
                  <Plus />
                  {t('workflows:imageNodes.addColor')}
                </Button>
              </div>
              <div className="grid grid-cols-2 gap-2">
                <NumericField
                  label={t('workflows:imageNodes.tolerance')}
                  value={settings.tolerance}
                  min={0}
                  max={255}
                  onChange={(tolerance) => update({ tolerance })}
                />
                <NumericField
                  label={t('workflows:imageNodes.softness')}
                  value={settings.softness}
                  min={0}
                  max={64}
                  onChange={(softness) => update({ softness })}
                />
              </div>
              <label className="flex items-center justify-between gap-3 text-sm">
                {t('workflows:imageNodes.edgeConnected')}
                <Switch
                  checked={settings.edgeConnected}
                  onCheckedChange={(edgeConnected) => update({ edgeConnected })}
                />
              </label>
            </>
          )}
        </>
      )}
      {node.kind === 'media-inpaint' && (
        <>
          <p className="text-xs text-muted-foreground">{t('workflows:imageNodes.regionHint')}</p>
          <ModelDownload t={t} id="inpaint" />
          <div className="grid grid-cols-2 gap-2">
            <NumericField
              label={t('workflows:imageNodes.regionX')}
              value={settings.region.x}
              min={0}
              max={99}
              onChange={(x) => update({ region: { ...settings.region, x } })}
            />
            <NumericField
              label={t('workflows:imageNodes.regionY')}
              value={settings.region.y}
              min={0}
              max={99}
              onChange={(y) => update({ region: { ...settings.region, y } })}
            />
            <NumericField
              label={t('workflows:imageNodes.regionWidth')}
              value={settings.region.width}
              min={1}
              max={100}
              onChange={(width) => update({ region: { ...settings.region, width } })}
            />
            <NumericField
              label={t('workflows:imageNodes.regionHeight')}
              value={settings.region.height}
              min={1}
              max={100}
              onChange={(height) => update({ region: { ...settings.region, height } })}
            />
          </div>
        </>
      )}
      {node.kind === 'media-frames' && (
        <>
          <p className="text-xs text-muted-foreground">{t('workflows:imageNodes.gridHint')}</p>
          <div className="grid grid-cols-2 gap-2">
            <NumericField
              label={t('workflows:imageNodes.columns')}
              value={settings.columns}
              min={1}
              max={16}
              onChange={(columns) => update({ columns })}
            />
            <NumericField
              label={t('workflows:imageNodes.rows')}
              value={settings.rows}
              min={1}
              max={16}
              onChange={(rows) => update({ rows })}
            />
          </div>
          <NumericField
            label={t('workflows:imageNodes.duration')}
            value={settings.durationMs}
            min={16}
            max={10000}
            onChange={(durationMs) => update({ durationMs })}
          />
        </>
      )}
      {node.kind === 'media-transform' && (
        <>
          <label className="flex items-center justify-between gap-3 text-sm">
            {t('workflows:imageNodes.trim')}
            <Switch checked={settings.trim} onCheckedChange={(trim) => update({ trim })} />
          </label>
          <FieldLabel variant="control">
            {t('workflows:imageNodes.align')}
            <AppSelect
              value={settings.align}
              onChange={(event) =>
                update({
                  align:
                    event.target.value === 'center'
                      ? 'center'
                      : event.target.value === 'none'
                        ? 'none'
                        : 'bottom-center',
                })
              }
            >
              <option value="bottom-center">{t('workflows:imageNodes.bottomCenter')}</option>
              <option value="center">{t('workflows:imageNodes.center')}</option>
              <option value="none">{t('workflows:imageNodes.noAlign')}</option>
            </AppSelect>
          </FieldLabel>
          <NumericField
            label={t('workflows:imageNodes.maxFrameSize')}
            value={settings.maxFrameSize}
            min={16}
            max={1024}
            onChange={(maxFrameSize) => update({ maxFrameSize: Math.round(maxFrameSize) })}
          />
          <p className="text-xs text-muted-foreground">
            {t('workflows:imageNodes.maxFrameSizeHint', { size: settings.maxFrameSize })}
          </p>
          <NumericField
            label={t('workflows:imageNodes.padding')}
            value={settings.padding}
            min={0}
            max={64}
            onChange={(padding) => update({ padding })}
          />
          <details className="rounded-lg border p-3">
            <summary className="cursor-pointer text-sm">
              {t('workflows:imageNodes.frameEdits')}
            </summary>
            <div className="mt-3 space-y-3">
              <NumericField
                label={t('workflows:imageNodes.frameNumber')}
                value={frameIndex + 1}
                min={1}
                max={512}
                onChange={(value) => setFrameIndex(Math.round(value) - 1)}
              />
              <label className="flex justify-between text-sm">
                {t('workflows:imageNodes.frameEnabled')}
                <Switch
                  checked={transform.enabled}
                  onCheckedChange={(enabled) => updateTransform({ enabled })}
                />
              </label>
              <div className="grid grid-cols-2 gap-2">
                <NumericField
                  label="X"
                  value={transform.x}
                  min={-4096}
                  max={4096}
                  onChange={(x) => updateTransform({ x })}
                />
                <NumericField
                  label="Y"
                  value={transform.y}
                  min={-4096}
                  max={4096}
                  onChange={(y) => updateTransform({ y })}
                />
                <NumericField
                  label={t('workflows:imageNodes.rotation')}
                  value={transform.rotation}
                  min={-360}
                  max={360}
                  onChange={(rotation) => updateTransform({ rotation })}
                />
                <NumericField
                  label={t('workflows:imageNodes.scale')}
                  value={transform.scale}
                  min={0.05}
                  max={8}
                  step={0.05}
                  onChange={(scale) => updateTransform({ scale })}
                />
                <NumericField
                  label={t('workflows:imageNodes.opacity')}
                  value={transform.opacity}
                  min={0}
                  max={1}
                  step={0.05}
                  onChange={(opacity) => updateTransform({ opacity })}
                />
                <NumericField
                  label={t('workflows:imageNodes.duration')}
                  value={transform.durationMs}
                  min={16}
                  max={10000}
                  onChange={(durationMs) => updateTransform({ durationMs })}
                />
              </div>
              <p className="text-xs text-muted-foreground">{t('workflows:imageNodes.orderHint')}</p>
              <Button
                size="sm"
                variant="outline"
                disabled={order.includes(frameIndex)}
                onClick={() => update({ frameOrder: [...order, frameIndex] })}
              >
                {t('workflows:imageNodes.addToOrder')}
              </Button>
              {order.map((frame, index) => (
                <div className="flex items-center gap-1" key={frame}>
                  <span className="mr-auto text-xs">
                    {t('workflows:imageNodes.frameAt', { index: frame + 1 })}
                  </span>
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    aria-label={t('workflows:imageNodes.earlier')}
                    disabled={index === 0}
                    onClick={() => move(index, -1)}
                  >
                    <ArrowUp />
                  </Button>
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    aria-label={t('workflows:imageNodes.later')}
                    disabled={index === order.length - 1}
                    onClick={() => move(index, 1)}
                  >
                    <ArrowDown />
                  </Button>
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    aria-label={t('workflows:imageNodes.resetOrder')}
                    onClick={() => update({ frameOrder: order.filter((item) => item !== frame) })}
                  >
                    <Trash2 />
                  </Button>
                </div>
              ))}
            </div>
          </details>
        </>
      )}
      {node.kind === 'media-preview' && (
        <p className="text-xs text-muted-foreground">{t('workflows:imageNodes.previewHint')}</p>
      )}
      {node.kind === 'media-export' && (
        <FieldLabel variant="control">
          {t('workflows:imageNodes.filename')}
          <Input
            value={settings.filename}
            maxLength={100}
            onChange={(event) => update({ filename: event.target.value })}
          />
          <small>{t('workflows:imageNodes.exportHint')}</small>
        </FieldLabel>
      )}
    </div>
  )
}
