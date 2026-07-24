import { useCallback, useEffect, useState, type FormEvent } from 'react'
import {
  AlertTriangle,
  Download,
  ExternalLink,
  Eye,
  File,
  FileImage,
  FileVideo,
  Link2,
  Paperclip,
  Plus,
  RefreshCw,
  Trash2,
  X,
} from 'lucide-react'
import { STORAGE_KEYS } from '@/app/storage'
import { useI18n } from '@/app/use-i18n'
import { Panel, Segmented } from '@/components/ui'
import { StarOrbit } from '@/components/StarOrbit'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { apiJson } from '@/lib/api'
import { formatFileSize } from '@/lib/format'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { ChatAttachment, EntityRecord } from '@/types/chat'

type Asset = ChatAttachment & {
  id: string
  name: string
  kind: string
  url?: string
  source?: string
  sessionName?: string
}
type PreviewAsset = Asset & { text?: string }
type AssetsPageProps = {
  query?: string
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  onUse: (asset: ChatAttachment) => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

const TEXT_PREVIEW_EXTENSIONS = new Set([
  'txt',
  'md',
  'json',
  'js',
  'jsx',
  'ts',
  'tsx',
  'css',
  'html',
  'xml',
  'yaml',
  'yml',
  'csv',
  'log',
  'py',
  'java',
  'go',
  'rs',
  'sh',
  'ps1',
  'toml',
  'sql',
])
type AssetTab = 'all' | 'image' | 'file' | 'link' | 'current'
const ASSET_TABS: AssetTab[] = ['all', 'image', 'file', 'link', 'current']

function assetTabLabel(tab: AssetTab, t: ReturnType<typeof useI18n>['t']) {
  if (tab === 'image') return t('assets:assetsPage.images')
  if (tab === 'file') return t('assets:assetsPage.files')
  if (tab === 'link') return t('assets:assetsPage.links')
  if (tab === 'current') return t('assets:assetsPage.fromCurrentChat')
  return t('assets:assetsPage.all')
}

function fileExtension(name: unknown) {
  return (
    String(name || '')
      .split('.')
      .at(-1)
      ?.toLowerCase() || ''
  )
}

export function AssetsPage({
  query = '',
  notify,
  registerPrimaryAction,
  onUse,
  requestConfirm,
}: AssetsPageProps) {
  const { t } = useI18n()
  const [tab, setTab] = useState<AssetTab>('all')
  const [assets, setAssets] = useState<Asset[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [preview, setPreview] = useState<PreviewAsset | null>(null)
  const [linkModal, setLinkModal] = useState(false)
  usePagePrimaryAction(registerPrimaryAction, () => setLinkModal(true))

  const loadAssets = useCallback(async () => {
    setLoading(true)
    setError('')
    try {
      const params = new URLSearchParams()
      if (query) params.set('query', query)
      if (tab === 'image') params.set('kind', 'image')
      if (tab === 'file') params.set('kind', 'file')
      if (tab === 'link') params.set('kind', 'link')
      if (tab === 'current')
        params.set('sessionId', localStorage.getItem(STORAGE_KEYS.activeSession) || '__none__')
      const data = await apiJson<{ assets: Asset[] }>(`/api/assets?${params}`)
      setAssets(data.assets)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setLoading(false)
    }
  }, [query, tab])

  useEffect(() => {
    loadAssets()
  }, [loadAssets])

  const deleteAsset = async (asset: Asset) => {
    const approved = await requestConfirm({
      title: t('assets:assetsPage.deleteAsset'),
      message: t('assets:assetsPage.deleteAssetName', { name: asset.name }),
      confirmLabel: t('assets:assetsPage.delete'),
    })
    if (!approved) return
    try {
      await apiJson(`/api/assets/${encodeURIComponent(asset.id)}`, { method: 'DELETE' })
      setAssets((current) => current.filter((item) => item.id !== asset.id))
      notify(t('assets:assetsPage.assetDeleted'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  const attachAsset = async (asset: Asset) => {
    try {
      const content = await apiJson<ChatAttachment>(
        `/api/assets/${encodeURIComponent(asset.id)}/content`,
      )
      onUse(content)
      notify(t('assets:assetsPage.nameAddedToChat', { name: asset.name }))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  const previewAsset = async (asset: Asset) => {
    let text = ''
    if (asset.kind === 'link') text = asset.url || ''
    else if (
      asset.mimeType?.startsWith('text/') ||
      TEXT_PREVIEW_EXTENSIONS.has(fileExtension(asset.name))
    ) {
      const content = await apiJson<EntityRecord>(
        `/api/assets/${encodeURIComponent(asset.id)}/content`,
      )
      text = content.text || ''
    }
    setPreview({ ...asset, text })
  }

  return (
    <div className="asset-page">
      <div className="asset-toolbar">
        <Segmented
          options={ASSET_TABS.map((item) => assetTabLabel(item, t))}
          value={assetTabLabel(tab, t)}
          onChange={(label) =>
            setTab(ASSET_TABS.find((item) => assetTabLabel(item, t) === label) || 'all')
          }
        />
      </div>
      <div className="asset-summary">
        <span>
          <strong>{assets.length}</strong> {t('assets:assetsPage.assets')}
        </span>
        <span>
          {t('assets:assetsPage.conversationAttachmentsAndAgentOutputsGatherHereAutomatically')}
        </span>
      </div>
      {error && (
        <div className="config-error">
          <AlertTriangle size={13} />
          {error}
        </div>
      )}
      {loading ? (
        <Panel className="empty-state">
          <RefreshCw className="spin" size={23} />
          <h2>{t('assets:assetsPage.loadingAssets')}</h2>
        </Panel>
      ) : assets.length ? (
        <div className="asset-grid functional">
          {assets.map((asset) => {
            const isVideo = asset.mimeType?.startsWith('video/')
            const Icon =
              asset.kind === 'image'
                ? FileImage
                : isVideo
                  ? FileVideo
                  : asset.kind === 'link'
                    ? Link2
                    : File
            return (
              <Panel className="asset-card functional" key={asset.id}>
                <button
                  className={`asset-preview ${asset.kind} ${isVideo ? 'video' : ''}`}
                  onClick={() => previewAsset(asset)}
                >
                  {asset.kind === 'image' ? (
                    <img
                      src={`/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`}
                      alt=""
                    />
                  ) : isVideo ? (
                    <video
                      src={`/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`}
                      muted
                      preload="metadata"
                    />
                  ) : (
                    <Icon size={38} />
                  )}
                </button>
                <div className="asset-card-copy">
                  <strong title={asset.name}>{asset.name}</strong>
                  <span>
                    {asset.kind === 'link'
                      ? asset.url
                        ? new URL(asset.url).hostname
                        : ''
                      : formatFileSize(asset.size)}{' '}
                    ·{' '}
                    {asset.source === 'agent'
                      ? t('assets:assetsPage.agentOutputs')
                      : asset.source === 'attachment'
                        ? t('assets:assetsPage.chatAttachments')
                        : t('assets:assetsPage.manualUploads')}
                  </span>
                  {asset.sessionName && (
                    <small title={asset.sessionName}>
                      {t('assets:assetsPage.fromName', { name: asset.sessionName })}
                    </small>
                  )}
                </div>
                <div className="asset-card-actions">
                  <button className="button tiny" onClick={() => previewAsset(asset)}>
                    <Eye size={13} />
                    {t('assets:assetsPage.preview')}
                  </button>
                  {asset.kind === 'link' ? (
                    <a className="button tiny" href={asset.url} target="_blank" rel="noreferrer">
                      <ExternalLink size={13} />
                      {t('assets:assetsPage.open')}
                    </a>
                  ) : (
                    <a
                      className="button tiny"
                      href={`/api/assets/${encodeURIComponent(asset.id)}/download`}
                    >
                      <Download size={13} />
                      {t('assets:assetsPage.download')}
                    </a>
                  )}
                  <button className="button tiny primary" onClick={() => attachAsset(asset)}>
                    <Paperclip size={13} />
                    {t('assets:assetsPage.useInChat')}
                  </button>
                  <button
                    className="icon-button danger"
                    title={t('assets:assetsPage.deleteAsset')}
                    onClick={() => deleteAsset(asset)}
                  >
                    <Trash2 size={13} />
                  </button>
                </div>
              </Panel>
            )
          })}
        </div>
      ) : (
        <Panel className="empty-state">
          <StarOrbit size={46} />
          <h2>{t('assets:assetsPage.nothingHasGatheredHereYet')}</h2>
          <p>
            {t(
              'assets:assetsPage.bringInALinkOrAttachAFileInAConversationVesperWillGatherTheOutputsCreatedAlongTheWay',
            )}
          </p>
          <button className="button primary" onClick={() => setLinkModal(true)}>
            <Link2 size={14} />
            {t('assets:assetsPage.addLink')}
          </button>
        </Panel>
      )}
      {preview && (
        <AssetPreviewModal
          asset={preview}
          onClose={() => setPreview(null)}
          onUse={() => attachAsset(preview)}
        />
      )}
      {linkModal && (
        <AssetLinkModal
          onClose={() => setLinkModal(false)}
          onCreated={() => {
            setLinkModal(false)
            loadAssets()
            notify(t('assets:assetsPage.linkAssetAdded'))
          }}
        />
      )}
    </div>
  )
}

function AssetPreviewModal({
  asset,
  onClose,
  onUse,
}: {
  asset: PreviewAsset
  onClose: () => void
  onUse: () => void
}) {
  const { t } = useI18n()
  const isVideo = asset.mimeType?.startsWith('video/')
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section className="modal asset-preview-modal">
        <div className="card-head">
          <div>
            <h2>{asset.name}</h2>
            <p>
              {asset.kind === 'link'
                ? asset.url
                : `${asset.mimeType} · ${formatFileSize(asset.size)}`}
            </p>
          </div>
          <button
            className="icon-button"
            aria-label={t('assets:assetsPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </div>
        <div className="asset-modal-content">
          {asset.kind === 'image' ? (
            <img
              src={`/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`}
              alt={asset.name}
            />
          ) : isVideo ? (
            <video controls src={`/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`} />
          ) : asset.kind === 'link' ? (
            <a href={asset.url} target="_blank" rel="noreferrer">
              <ExternalLink size={16} />
              {asset.url}
            </a>
          ) : asset.text ? (
            <pre>{asset.text}</pre>
          ) : (
            <div className="asset-file-preview">
              <File size={42} />
              <strong>{asset.name}</strong>
              <span>
                {t(
                  'assets:assetsPage.thisFileTypeCanBeDownloadedSupportedDocumentsCanAlsoBeAddedToAChatForAnalysis',
                )}
              </span>
            </div>
          )}
        </div>
        <div className="modal-actions">
          {asset.kind !== 'link' && (
            <a
              className="button secondary"
              href={`/api/assets/${encodeURIComponent(asset.id)}/download`}
            >
              <Download size={14} />
              {t('assets:assetsPage.download')}
            </a>
          )}
          <button className="button primary" onClick={onUse}>
            <Paperclip size={14} />
            {t('assets:assetsPage.useInChat')}
          </button>
        </div>
      </section>
    </div>
  )
}

function AssetLinkModal({ onClose, onCreated }: { onClose: () => void; onCreated: () => void }) {
  const { t } = useI18n()
  const [name, setName] = useState('')
  const [url, setUrl] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setSaving(true)
    setError('')
    try {
      await apiJson('/api/assets', {
        method: 'POST',
        body: JSON.stringify({ kind: 'link', name, url, source: 'upload' }),
      })
      onCreated()
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form className="modal" onSubmit={submit}>
        <div className="card-head">
          <div>
            <h2>{t('assets:assetsPage.addLinkAsset')}</h2>
            <p>{t('assets:assetsPage.linksCanBeArchivedOpenedOrAddedToAChatAsContext')}</p>
          </div>
          <button
            type="button"
            className="icon-button"
            aria-label={t('assets:assetsPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </div>
        <label className="field-label">
          {t('assets:assetsPage.name')}
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={t('assets:assetsPage.forExampleOpenAIAPIDocs')}
          />
        </label>
        <label className="field-label">
          URL
          <input
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            placeholder="https://example.com/docs"
          />
        </label>
        {error && (
          <div className="config-error">
            <AlertTriangle size={13} />
            {error}
          </div>
        )}
        <div className="modal-actions">
          <button type="button" className="button secondary" onClick={onClose}>
            {t('assets:assetsPage.cancel')}
          </button>
          <button className="button primary" disabled={saving || !url.trim()}>
            {saving ? <RefreshCw className="spin" size={14} /> : <Plus size={14} />}
            {saving ? t('assets:assetsPage.adding') : t('assets:assetsPage.addLink')}
          </button>
        </div>
      </form>
    </div>
  )
}
