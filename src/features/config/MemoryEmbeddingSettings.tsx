import { useCallback, useEffect, useState } from 'react'
import {
  AlertTriangle,
  BrainCircuit,
  Check,
  ChevronDown,
  Download,
  HardDrive,
  RefreshCw,
  Trash2,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { Badge, Panel, SectionTitle, Toggle } from '@/components/ui'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

type DownloadState = {
  status: 'downloading' | 'ready' | 'error'
  receivedBytes: number
  totalBytes: number
  percent: number
  file?: string
  error?: string
}

type EmbeddingModel = {
  id: string
  name: string
  description: string
  dimensions: number
  languages: string[]
  size: number
  installed: boolean
  downloading: DownloadState | null
}

type IndexingState = {
  pending: number
  ready: number
  failed: number
  running: boolean
  error?: string
}

type EmbeddingState = {
  enabled: boolean
  selectedModelId: string
  source: 'huggingface' | 'mirror'
  provider: string
  models: EmbeddingModel[]
  indexing: IndexingState | null
}

function formatBytes(value: number) {
  if (value >= 1024 * 1024)
    return `${(value / 1024 / 1024).toFixed(value >= 100 * 1024 * 1024 ? 0 : 1)} MB`
  return `${Math.round(value / 1024)} KB`
}

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export function MemoryEmbeddingSettings({
  notify,
  requestConfirm,
}: {
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t } = useI18n()
  const [state, setState] = useState<EmbeddingState | null>(null)
  const [modelId, setModelId] = useState('')
  const [source, setSource] = useState<'huggingface' | 'mirror'>('huggingface')
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    const next = await apiJson<EmbeddingState>('/api/memory/embedding')
    setState(next)
    setModelId((current) => current || next.selectedModelId || next.models[0]?.id || '')
    setSource(next.source || 'huggingface')
    return next
  }, [])

  useEffect(() => {
    void load().catch((caught) => setError(message(caught)))
  }, [load])

  useEffect(() => {
    const active =
      Boolean(busy) ||
      Boolean(state?.models.some((model) => model.downloading)) ||
      Boolean(state?.indexing?.running)
    if (!active) return
    const timer = window.setInterval(() => void load().catch(() => {}), 900)
    return () => window.clearInterval(timer)
  }, [busy, load, state?.indexing?.running, state?.models])

  const run = async (key: string, action: () => Promise<unknown>, notice: string) => {
    setBusy(key)
    setError('')
    try {
      await action()
      await load()
      notify(notice)
    } catch (caught) {
      setError(message(caught))
    } finally {
      setBusy('')
    }
  }

  const removeModel = async (model: EmbeddingModel) => {
    const approved = await requestConfirm({
      title: t('config:configPage.removeLocalModel'),
      message: t('config:configPage.removeEmbeddingModelName', { name: model.name }),
      confirmLabel: t('config:configPage.delete'),
    })
    if (!approved) return
    await run(
      `remove:${model.id}`,
      () =>
        apiJson(`/api/memory/embedding/models/${encodeURIComponent(model.id)}`, {
          method: 'DELETE',
        }),
      t('config:configPage.localModelRemoved'),
    )
  }

  if (!state) {
    return (
      <Panel className="empty-state">
        <RefreshCw className="spin" size={24} />
        <h2>{t('config:configPage.loadingEmbeddingModels')}</h2>
      </Panel>
    )
  }

  const selected = state.models.find((model) => model.id === modelId) || state.models[0]
  const indexing = state.indexing
  const indexedTotal =
    Number(indexing?.ready || 0) + Number(indexing?.pending || 0) + Number(indexing?.failed || 0)
  const indexPercent = indexedTotal
    ? Math.round((Number(indexing?.ready || 0) / indexedTotal) * 100)
    : 100

  return (
    <div className="embedding-settings">
      <Panel>
        <div className="language-settings-heading">
          <span className="language-settings-icon">
            <BrainCircuit size={19} />
          </span>
          <div>
            <h2>{t('config:configPage.memoryEmbedding')}</h2>
            <p>{t('config:configPage.memoryEmbeddingStatus', { provider: state.provider })}</p>
          </div>
        </div>
        <div className="embedding-enable-row">
          <span>
            <strong>{t('config:configPage.semanticRetrieval')}</strong>
            <small>{state.selectedModelId || t('config:configPage.lexicalRetrievalOnly')}</small>
          </span>
          <Toggle
            value={state.enabled}
            disabled={
              Boolean(busy) || (!state.enabled && !state.models.some((model) => model.installed))
            }
            onChange={(enabled) =>
              void run(
                'toggle',
                () =>
                  apiJson('/api/memory/embedding', {
                    method: 'PUT',
                    body: JSON.stringify({
                      modelId:
                        state.selectedModelId ||
                        state.models.find((model) => model.installed)?.id ||
                        '',
                      enabled,
                      source,
                    }),
                  }),
                enabled
                  ? t('config:configPage.semanticRetrievalEnabled')
                  : t('config:configPage.semanticRetrievalDisabled'),
              )
            }
          />
        </div>
      </Panel>

      <Panel>
        <SectionTitle title={t('config:configPage.localEmbeddingModel')} />
        <div className="embedding-selector-grid">
          <label className="field-label">
            {t('config:configPage.embeddingModel')}
            <span className="select-wrap">
              <AppSelect value={modelId} onChange={(event) => setModelId(event.target.value)}>
                {state.models.map((model) => (
                  <option key={model.id} value={model.id}>
                    {model.name}
                  </option>
                ))}
              </AppSelect>
              <ChevronDown size={13} />
            </span>
          </label>
          <label className="field-label">
            {t('config:configPage.downloadSource')}
            <span className="select-wrap">
              <AppSelect
                value={source}
                onChange={(event) => setSource(event.target.value as 'huggingface' | 'mirror')}
              >
                <option value="huggingface">Hugging Face</option>
                <option value="mirror">HF Mirror</option>
              </AppSelect>
              <ChevronDown size={13} />
            </span>
          </label>
        </div>
        {selected && (
          <div className="embedding-model-summary">
            <span className="list-icon">
              <HardDrive size={16} />
            </span>
            <span>
              <strong>{selected.name}</strong>
              <small>
                {selected.description} · {formatBytes(selected.size)} · {selected.dimensions}d
              </small>
            </span>
            <Badge tone={selected.installed ? 'green' : 'gray'}>
              {selected.installed
                ? t('config:configPage.downloaded')
                : t('config:configPage.notDownloaded')}
            </Badge>
          </div>
        )}
        {selected?.downloading && (
          <div className="embedding-progress">
            <div>
              <span>{selected.downloading.file}</span>
              <strong>{selected.downloading.percent}%</strong>
            </div>
            <div className="progress">
              <i style={{ width: `${selected.downloading.percent}%` }} />
            </div>
            <small>
              {formatBytes(selected.downloading.receivedBytes)} /{' '}
              {formatBytes(selected.downloading.totalBytes)}
            </small>
          </div>
        )}
        <div className="embedding-actions">
          {selected?.installed ? (
            <>
              <button
                className="button primary"
                disabled={Boolean(busy) || (state.enabled && state.selectedModelId === selected.id)}
                onClick={() =>
                  void run(
                    `select:${selected.id}`,
                    () =>
                      apiJson('/api/memory/embedding', {
                        method: 'PUT',
                        body: JSON.stringify({ modelId: selected.id, enabled: true, source }),
                      }),
                    t('config:configPage.embeddingModelActivated'),
                  )
                }
              >
                {busy === `select:${selected.id}` ? (
                  <RefreshCw className="spin" size={14} />
                ) : (
                  <Check size={14} />
                )}
                {state.enabled && state.selectedModelId === selected.id
                  ? t('config:configPage.inUse')
                  : t('config:configPage.useModel')}
              </button>
              <button
                className="icon-button danger"
                title={t('config:configPage.removeLocalModel')}
                disabled={Boolean(busy)}
                onClick={() => void removeModel(selected)}
              >
                <Trash2 size={14} />
              </button>
            </>
          ) : (
            <button
              className="button primary"
              disabled={Boolean(busy) || !selected}
              onClick={() =>
                selected &&
                void run(
                  `download:${selected.id}`,
                  () =>
                    apiJson('/api/memory/embedding/download', {
                      method: 'POST',
                      body: JSON.stringify({ modelId: selected.id, source, activate: true }),
                    }),
                  t('config:configPage.embeddingModelDownloaded'),
                )
              }
            >
              {busy === `download:${selected?.id}` ? (
                <RefreshCw className="spin" size={14} />
              ) : (
                <Download size={14} />
              )}
              {busy === `download:${selected?.id}`
                ? t('config:configPage.downloadingModel')
                : t('config:configPage.downloadAndUse')}
            </button>
          )}
        </div>
      </Panel>

      <Panel>
        <SectionTitle title={t('config:configPage.memoryIndex')} />
        <div className="usage-number">
          <span>{t('config:configPage.indexedMemories')}</span>
          <strong>{indexing?.ready || 0}</strong>
        </div>
        <div className="usage-number">
          <span>{t('config:configPage.pendingIndex')}</span>
          <strong>{indexing?.pending || 0}</strong>
        </div>
        <div className="progress">
          <i style={{ width: `${indexPercent}%` }} />
        </div>
        {indexing?.failed ? (
          <div className="config-error">
            <AlertTriangle size={13} />
            {t('config:configPage.indexFailures', { count: indexing.failed })}
          </div>
        ) : null}
        {(error || indexing?.error) && (
          <div className="config-error">
            <AlertTriangle size={13} />
            {error || indexing?.error}
          </div>
        )}
      </Panel>
    </div>
  )
}
