// 资源（资产）页：浏览/搜索/下载工作区生成的资源文件（图片/文档等）。
import { useCallback, useEffect, useState, type FormEvent } from 'react'
import {
  AlertTriangle,
  Download,
  ExternalLink,
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
import {
  AppCard as Panel,
  SegmentedTabs as Segmented,
  AppCardHeader,
  AppError,
  AppEmptyState,
} from '@/components/ui/app-primitives'
import { StarOrbit } from '@/components/StarOrbit'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { apiJson } from '@/lib/api'
import { formatFileSize } from '@/lib/format'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { ChatAttachment, EntityRecord } from '@/types/chat'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

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

  // 加载资源列表：按标签页（全部/图片/文件/链接/当前会话）与搜索词
  // 构造查询参数，current 页用活动会话 id 过滤。
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

  // 删除资源（确认后），成功后从列表本地移除。
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

  // 把资源加入聊天：取内容后通过 onUse 投递到活动会话。
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

  // 预览资源：链接直接取 URL，文本类拉取内容，其余走运行时预览。
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
    <div className="asset-page flex min-h-[100%] flex-col gap-[12px]">
      <div className="asset-toolbar [&_>_div:last-child]:flex [&_>_div:last-child]:gap-[7px] max-[650px]:items-stretch max-[650px]:flex-col max-[650px]:[&_>_div:last-child]:justify-end flex items-center justify-between gap-[12px]">
        <Segmented
          options={ASSET_TABS.map((item) => assetTabLabel(item, t))}
          value={assetTabLabel(tab, t)}
          onChange={(label) =>
            setTab(ASSET_TABS.find((item) => assetTabLabel(item, t) === label) || 'all')
          }
        />
      </div>
      <div className="asset-summary [&_strong]:text-[var(--text)] [&_strong]:text-[13px] max-[650px]:items-start max-[650px]:flex-col flex items-center justify-between gap-[12px] text-[var(--text-muted)] text-[12px]">
        <span>
          <strong>{assets.length}</strong> {t('assets:assetsPage.assets')}
        </span>
        <span>
          {t('assets:assetsPage.conversationAttachmentsAndAgentOutputsGatherHereAutomatically')}
        </span>
      </div>
      {error && (
        <AppError>
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
      {loading ? (
        <AppEmptyState>
          <RefreshCw className="animate-spin" size={23} />
          <h2>{t('assets:assetsPage.loadingAssets')}</h2>
        </AppEmptyState>
      ) : assets.length ? (
        <div className="asset-grid max-[650px]:grid-cols-[1fr] functional grid auto-rows-[minmax(292px,auto)] grid-cols-[repeat(auto-fill,minmax(260px,1fr))] content-start gap-3">
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
              <Panel
                className="asset-card [transition:transform_var(--d2)_var(--ease-out),_box-shadow_var(--d2)_var(--ease-out),_border-color_var(--d2)_var(--ease-out)] hover:[transform:translateY(-2px)] hover:shadow-[var(--sh-2)] hover:border-[var(--star-border)] [&_>_strong]:text-[13px] [&_>_span]:m-[5px_0_10px] [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[12px] [&_>_div:last-child]:flex [&_>_div:last-child]:gap-[6px] [&_>_div:last-child]:mt-[auto] max-[650px]:min-h-[200px] functional flex h-full min-h-[292px] flex-col overflow-hidden"
                key={asset.id}
              >
                <button
                  className={`asset-preview [&.blue]:bg-[var(--asset-preview-blue-bg)] [&.blue]:text-[var(--asset-preview-blue-text)] [&.violet]:bg-[var(--asset-preview-violet-bg)] [&.violet]:text-[var(--violet-strong)] [&.yellow]:bg-[var(--warning-soft)] [&.yellow]:text-[var(--amber)] [&.red]:bg-[var(--danger-soft)] [&.red]:text-[var(--danger)] [.asset-card.functional_&::after]:absolute [.asset-card.functional_&::after]:inset-0 [.asset-card.functional_&::after]:rounded-[var(--r-sm)] [.asset-card.functional_&::after]:bg-[rgba(0,_0,_0,_0)] [.asset-card.functional_&::after]:[transition:background_var(--d1)_var(--ease-out)] [.asset-card.functional_&::after]:[content:''] [.asset-card.functional_&::after]:pointer-events-none [.asset-card.functional_&:hover::after]:bg-[rgba(0,_0,_0,_.16)] [.asset-card.functional_&.image]:bg-[var(--surface-hover)] [.asset-card.functional_&.link]:bg-[var(--surface-hover)] [.asset-card.functional_&.link]:text-[var(--success)] [.asset-card.functional_&.file]:bg-[var(--surface-hover)] [.asset-card.functional_&.file]:text-[var(--violet-strong)] [.asset-card.functional_&_img]:relative [.asset-card.functional_&_img]:z-[0] [.asset-card.functional_&_img]:w-full [.asset-card.functional_&_img]:h-full [.asset-card.functional_&_img]:rounded-[var(--r-sm)] [.asset-card.functional_&_img]:object-cover [.asset-card.functional_&_video]:relative [.asset-card.functional_&_video]:z-[0] [.asset-card.functional_&_video]:w-full [.asset-card.functional_&_video]:h-full [.asset-card.functional_&_video]:rounded-[var(--r-sm)] [.asset-card.functional_&_video]:object-cover ${asset.kind}    ${isVideo ? 'video' : ''} relative -mx-1.5 -mt-1.5 mb-2.5 grid h-44 min-h-0 w-[calc(100%+12px)] shrink-0 place-items-center overflow-hidden rounded-[var(--r-sm)] border-0 bg-[var(--surface-hover)] text-[var(--text-muted)]`}
                  title={t('assets:assetsPage.preview')}
                  onClick={() => previewAsset(asset)}
                >
                  {asset.kind === 'image' ? (
                    <>
                      <Icon className="absolute" size={38} />
                      <img
                        className="block size-full min-h-0 object-cover"
                        src={`/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`}
                        alt=""
                        onError={(event) => {
                          event.currentTarget.hidden = true
                        }}
                      />
                    </>
                  ) : isVideo ? (
                    <video
                      className="block size-full min-h-0 object-cover"
                      src={`/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`}
                      muted
                      preload="metadata"
                    />
                  ) : (
                    <Icon size={38} />
                  )}
                </button>
                <div className="asset-card-copy [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_span]:overflow-hidden [&_span]:text-[var(--text-muted)] [&_span]:text-[13px] [&_span]:text-ellipsis [&_span]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap flex min-w-0 flex-col gap-[4px]">
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
                <div className="asset-card-actions flex-none [.asset-card.functional:hover_&]:opacity-100 [.asset-card.functional:hover_&]:[transform:translateY(0)] [.asset-card.functional:focus-within_&]:opacity-100 [.asset-card.functional:focus-within_&]:[transform:translateY(0)] [&_a]:no-underline flex flex-wrap items-center gap-[5px] [margin-top:auto] [padding-top:10px] opacity-0 [transform:translateY(4px)] [transition:opacity_var(--d1)_var(--ease-out),_transform_var(--d1)_var(--ease-out)]">
                  {asset.kind === 'link' ? (
                    <Button asChild variant="outline">
                      <a href={asset.url} target="_blank" rel="noreferrer">
                        <ExternalLink size={13} />
                        {t('assets:assetsPage.open')}
                      </a>
                    </Button>
                  ) : (
                    <Button asChild variant="outline">
                      <a href={`/api/assets/${encodeURIComponent(asset.id)}/download`}>
                        <Download size={13} />
                        {t('assets:assetsPage.download')}
                      </a>
                    </Button>
                  )}
                  <Button onClick={() => attachAsset(asset)}>
                    <Paperclip size={13} />
                    {t('assets:assetsPage.useInChat')}
                  </Button>
                  <Button
                    variant="destructive"
                    size="icon"
                    className="ml-auto"
                    title={t('assets:assetsPage.deleteAsset')}
                    onClick={() => deleteAsset(asset)}
                  >
                    <Trash2 size={13} />
                  </Button>
                </div>
              </Panel>
            )
          })}
        </div>
      ) : (
        <AppEmptyState>
          <StarOrbit size={46} />
          <h2>{t('assets:assetsPage.nothingHasGatheredHereYet')}</h2>
          <p>
            {t(
              'assets:assetsPage.bringInALinkOrAttachAFileInAConversationPisperWillGatherTheOutputsCreatedAlongTheWay',
            )}
          </p>
          <Button size="lg" className="mt-4" onClick={() => setLinkModal(true)}>
            <Link2 size={14} />
            {t('assets:assetsPage.addLink')}
          </Button>
        </AppEmptyState>
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
      className="modal-backdrop max-[650px]:p-[8px] fixed z-[70] inset-0 grid place-items-center overflow-y-auto bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)] [padding:20px] [overscroll-behavior:contain] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)] asset-preview-modal w-[min(96vw,1600px)]">
        <AppCardHeader>
          <div>
            <h2>{asset.name}</h2>
            <p>
              {asset.kind === 'link'
                ? asset.url
                : `${asset.mimeType} · ${formatFileSize(asset.size)}`}
            </p>
          </div>
          <Button
            variant="ghost"
            size="icon"
            aria-label={t('assets:assetsPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </Button>
        </AppCardHeader>
        <div className="asset-modal-content [&_>_img]:max-w-[100%] [&_>_img]:max-h-[76vh] [&_>_img]:rounded-[var(--r-sm)] [&_>_img]:object-contain [&_>_video]:max-w-[100%] [&_>_video]:max-h-[76vh] [&_>_video]:rounded-[var(--r-sm)] [&_>_video]:object-contain [&_>_pre]:w-full [&_>_pre]:h-full [&_>_pre]:m-0 [&_>_pre]:overflow-auto [&_>_pre]:whitespace-pre-wrap [&_>_pre]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_>_pre]:text-[12px] [&_>_pre]:leading-[1.55] [&_>_a]:flex [&_>_a]:items-center [&_>_a]:gap-[7px] [&_>_a]:text-[var(--text-soft)] [&_>_a]:text-[12px] [&_>_a]:[text-decoration:underline] [&_>_a]:[text-underline-offset:2px] [&_>_a]:[word-break:break-all] grid min-h-[min(420px,60dvh)] max-h-[82dvh] place-items-center overflow-auto [margin-top:14px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:12px]">
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
            <div className="asset-file-preview [&_strong]:text-[var(--text)] [&_strong]:text-[13px] [&_span]:text-[12px] [&_span]:leading-[1.55] flex max-w-[400px] flex-col items-center gap-[9px] text-[var(--text-muted)] text-center">
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
        <div className="flex justify-end gap-[8px] [margin-top:18px]">
          {asset.kind !== 'link' && (
            <Button asChild variant="outline" size="lg" className="bg-surface-subtle">
              <a href={`/api/assets/${encodeURIComponent(asset.id)}/download`}>
                <Download size={14} />
                {t('assets:assetsPage.download')}
              </a>
            </Button>
          )}
          <Button size="lg" onClick={onUse}>
            <Paperclip size={14} />
            {t('assets:assetsPage.useInChat')}
          </Button>
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
      className="modal-backdrop max-[650px]:p-[8px] fixed z-[70] inset-0 grid place-items-center overflow-y-auto bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)] [padding:20px] [overscroll-behavior:contain] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form
        className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)]"
        onSubmit={submit}
      >
        <AppCardHeader>
          <div>
            <h2>{t('assets:assetsPage.addLinkAsset')}</h2>
            <p>{t('assets:assetsPage.linksCanBeArchivedOpenedOrAddedToAChatAsContext')}</p>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={t('assets:assetsPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </Button>
        </AppCardHeader>
        <FieldLabel variant="control">
          {t('assets:assetsPage.name')}
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={t('assets:assetsPage.forExampleOpenAIAPIDocs')}
          />
        </FieldLabel>
        <FieldLabel variant="control">
          URL
          <input
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            placeholder="https://example.com/docs"
          />
        </FieldLabel>
        {error && (
          <AppError>
            <AlertTriangle size={13} />
            {error}
          </AppError>
        )}
        <div className="flex justify-end gap-[8px] [margin-top:18px]">
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            onClick={onClose}
          >
            {t('assets:assetsPage.cancel')}
          </Button>
          <Button size="lg" disabled={saving || !url.trim()}>
            {saving ? <RefreshCw className="animate-spin" size={14} /> : <Plus size={14} />}
            {saving ? t('assets:assetsPage.adding') : t('assets:assetsPage.addLink')}
          </Button>
        </div>
      </form>
    </div>
  )
}
