// 资产页面持有筛选和动作意图；服务器实体只归同域 Query 缓存。
import { useEffect, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  AlertTriangle,
  Download,
  ExternalLink,
  File,
  FileImage,
  FileVideo,
  Link2,
  ListFilter,
  Paperclip,
  RefreshCw,
  Trash2,
  X,
} from 'lucide-react'
import { PageFrame } from '@/components/app/PageFrame'
import { PageToolbar } from '@/components/app/PageToolbar'
import { useI18n } from '@/app/use-i18n'
import { AppCard, AppEmptyState } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { StarOrbit } from '@/components/StarOrbit'
import { OrbitSpinner } from '@/components/ui/spinner'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { formatFileSize } from '@/lib/format'
import { canOpenAssetInApplication, openAssetInApplication } from '@/lib/open-asset'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { ChatAttachment } from '@/types/chat'
import { AssetLinkDialog } from './AssetLinkDialog'
import { AssetPreviewDialog } from './AssetPreviewDialog'
import { assetMediaUrl, assetsApi, type Asset, type AssetKind } from './assets-api'
import { ASSETS_LIST_KEY, assetsListQueryOptions, refreshAssetLists } from './assets-queries'

type AssetsPageProps = {
  query?: string
  activeSessionId?: string | null
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  onUse: (asset: ChatAttachment) => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}
type Action = 'use' | 'open' | 'delete'
const KINDS: AssetKind[] = ['all', 'image', 'video', 'file', 'link']
function kindLabel(kind: AssetKind, t: ReturnType<typeof useI18n>['t']) {
  if (kind === 'image') return t('assets:assetsPage.images')
  if (kind === 'video') return t('assets:assetsPage.videos')
  if (kind === 'file') return t('assets:assetsPage.files')
  if (kind === 'link') return t('assets:assetsPage.links')
  return t('assets:assetsPage.all')
}

export function AssetsPage({
  query = '',
  activeSessionId,
  notify,
  registerPrimaryAction,
  onUse,
  requestConfirm,
}: AssetsPageProps) {
  const { t } = useI18n()
  const client = useQueryClient()
  const [kind, setKind] = useState<AssetKind>('all')
  const [current, setCurrent] = useState(false)
  const [linkOpen, setLinkOpen] = useState(false)
  const [preview, setPreview] = useState<{
    asset: Asset
    generation: number
    trigger: HTMLElement | null
  } | null>(null)
  const generation = useRef(0)
  const fallbackRef = useRef<HTMLButtonElement>(null)
  const mounted = useRef(true)
  const sessionRef = useRef(activeSessionId)
  sessionRef.current = activeSessionId
  const actionLocks = useRef(new Map<string, { action: Action; controller?: AbortController }>())
  const [pending, setPending] = useState<Record<string, Action | undefined>>({})
  const [actionErrors, setActionErrors] = useState<Record<string, string>>({})
  usePagePrimaryAction(registerPrimaryAction, () => setLinkOpen(true))
  const noSession = current && !activeSessionId
  const result = useQuery({
    ...assetsListQueryOptions({ query, kind, sessionId: current ? activeSessionId : '' }),
    enabled: !noSession,
  })
  const assets = noSession ? [] : result.data
  useEffect(() => {
    mounted.current = true
    const locks = actionLocks.current
    return () => {
      mounted.current = false
      for (const lock of locks.values()) lock.controller?.abort()
    }
  }, [])
  useEffect(() => {
    // 加入对话属于发起动作时的会话，切会话取消读取，不能投递到新的活动会话。
    const locks = actionLocks.current
    return () => {
      for (const lock of locks.values()) lock.controller?.abort()
    }
  }, [activeSessionId])

  const act = async (asset: Asset, action: Action) => {
    if (actionLocks.current.has(asset.id)) return
    const controller = action === 'use' ? new AbortController() : undefined
    const session = activeSessionId
    actionLocks.current.set(asset.id, { action, controller })
    setPending((value) => ({ ...value, [asset.id]: action }))
    setActionErrors((value) => ({ ...value, [asset.id]: '' }))
    try {
      if (action === 'delete') {
        const approved = await requestConfirm({
          title: t('assets:assetsPage.deleteAsset'),
          message: t('assets:assetsPage.deleteAssetName', { name: asset.name }),
          confirmLabel: t('assets:assetsPage.delete'),
        })
        if (!approved || !mounted.current) return
        await client.cancelQueries({ queryKey: ASSETS_LIST_KEY })
        await assetsApi.remove(asset.id)
        client.setQueriesData<Asset[]>({ queryKey: ASSETS_LIST_KEY }, (data) =>
          data?.filter((item) => item.id !== asset.id),
        )
        if (mounted.current) notify(t('assets:assetsPage.assetDeleted'))
        await refreshAssetLists(client)
      } else if (action === 'use') {
        const content = await assetsApi.content(asset.id, controller?.signal)
        if (!mounted.current || controller?.signal.aborted || sessionRef.current !== session) return
        onUse(content)
        notify(t('assets:assetsPage.nameAddedToChat', { name: asset.name }))
      } else {
        const opened = await openAssetInApplication(asset)
        if (mounted.current)
          notify(
            opened === 'application'
              ? t('assets:assetsPage.openedInApplication')
              : t('assets:assetsPage.downloadStarted'),
          )
      }
    } catch (caught) {
      if (mounted.current && !controller?.signal.aborted)
        setActionErrors((value) => ({
          ...value,
          [asset.id]: caught instanceof Error ? caught.message : String(caught),
        }))
    } finally {
      actionLocks.current.delete(asset.id)
      if (mounted.current) setPending((value) => ({ ...value, [asset.id]: undefined }))
    }
  }
  const filtered = Boolean(query.trim() || kind !== 'all')
  const emptyTitle = current
    ? t('assets:assetsPage.noCurrentAssets')
    : filtered
      ? t('assets:assetsPage.noMatchingAssets')
      : t('assets:assetsPage.nothingHasGatheredHereYet')
  // 已生效的筛选条件是「开启」态芯片：强调色软底 + 强调描边，点击即移除该条件。
  const chip =
    'border-star-border bg-star-soft text-star-strong hover:bg-star-soft hover:text-star-strong max-[650px]:h-11'
  return (
    <PageFrame variant="collection" scrollMode="content">
      <div className="flex h-full min-h-0 min-w-0 flex-col gap-3">
        <div className="shrink-0">
          <PageToolbar
            actions={
              <Button
                variant="ghost"
                size="icon"
                className="max-[650px]:size-11"
                loading={result.isFetching && !noSession}
                disabled={noSession || result.isFetching}
                aria-label={t('assets:assetsPage.refreshAssets')}
                onClick={() => {
                  void result.refetch()
                }}
              >
                <RefreshCw />
              </Button>
            }
          >
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button ref={fallbackRef} variant="secondary" className="max-[650px]:h-11">
                  <ListFilter />
                  {t('assets:assetsPage.filters')}
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent>
                <DropdownMenuLabel>{t('assets:assetsPage.assetType')}</DropdownMenuLabel>
                <DropdownMenuRadioGroup
                  value={kind}
                  onValueChange={(value) => {
                    if (KINDS.includes(value as AssetKind)) setKind(value as AssetKind)
                  }}
                >
                  {KINDS.map((item) => (
                    <DropdownMenuRadioItem
                      key={item}
                      value={item}
                      onSelect={(event) => event.preventDefault()}
                    >
                      {kindLabel(item, t)}
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
                <DropdownMenuSeparator />
                <DropdownMenuCheckboxItem
                  checked={current}
                  onCheckedChange={(value) => setCurrent(value === true)}
                >
                  {t('assets:assetsPage.fromCurrentChat')}
                </DropdownMenuCheckboxItem>
              </DropdownMenuContent>
            </DropdownMenu>
            {kind !== 'all' && (
              <Button
                variant="outline"
                size="sm"
                className={chip}
                onClick={() => setKind('all')}
                aria-label={t('assets:assetsPage.removeFilter', { filter: kindLabel(kind, t) })}
              >
                {kindLabel(kind, t)}
                <X />
              </Button>
            )}
            {current && (
              <Button
                variant="outline"
                size="sm"
                className={chip}
                onClick={() => setCurrent(false)}
                aria-label={t('assets:assetsPage.removeFilter', {
                  filter: t('assets:assetsPage.fromCurrentChat'),
                })}
              >
                {t('assets:assetsPage.fromCurrentChat')}
                <X />
              </Button>
            )}
          </PageToolbar>
          <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 text-xs text-content-muted">
            <span aria-live="polite">
              {assets && (
                <>
                  <strong className="font-medium text-content tabular-nums">{assets.length}</strong>{' '}
                  {t('assets:assetsPage.assets')}
                </>
              )}
            </span>
            <span className="min-w-0">
              {t('assets:assetsPage.conversationAttachmentsAndAgentOutputsGatherHereAutomatically')}
            </span>
          </div>
          {!noSession && result.isError && (
            <div
              role="alert"
              className="mt-3 flex flex-wrap items-center gap-2 rounded-sm border border-danger-border bg-danger-soft py-1 pr-1 pl-3 text-sm text-danger"
            >
              <AlertTriangle className="size-4 shrink-0" />
              <span className="min-w-0 flex-1 break-words">{result.error.message}</span>
              <Button
                variant="ghost"
                size="sm"
                className="text-danger"
                onClick={() => {
                  void result.refetch()
                }}
              >
                {t('assets:assetsPage.retry')}
              </Button>
            </div>
          )}
          {!noSession && result.isFetching && result.data && (
            <p role="status" className="mt-2 flex items-center gap-1.5 text-xs text-content-muted">
              <OrbitSpinner className="size-3.5" />
              {t('assets:assetsPage.refreshingAssets')}
            </p>
          )}
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain" data-assets-results>
          {noSession ? (
            <AppEmptyState>
              {/* 包一层，避免空状态容器给首个 svg 再套一圈虚线圆：星轨插画自带轨道。 */}
              <span aria-hidden="true">
                <StarOrbit size={54} />
              </span>
              <h2>{t('assets:assetsPage.noActiveSession')}</h2>
            </AppEmptyState>
          ) : result.isPending ? (
            <AppEmptyState>
              <OrbitSpinner className="size-5" />
              <h2 role="status">{t('assets:assetsPage.loadingAssets')}</h2>
            </AppEmptyState>
          ) : assets?.length ? (
            <div className="asset-grid grid auto-rows-[minmax(292px,auto)] grid-cols-[repeat(auto-fill,minmax(min(240px,100%),1fr))] content-start gap-3 p-px">
              {assets.map((asset) => (
                <AssetCard
                  key={asset.id}
                  asset={asset}
                  busy={pending[asset.id]}
                  error={actionErrors[asset.id]}
                  onPreview={(trigger) =>
                    setPreview({ asset, trigger, generation: ++generation.current })
                  }
                  onAction={(action) => {
                    void act(asset, action)
                  }}
                />
              ))}
            </div>
          ) : !result.isError ? (
            <AppEmptyState>
              {/* 包一层，避免空状态容器给首个 svg 再套一圈虚线圆：星轨插画自带轨道。 */}
              <span aria-hidden="true">
                <StarOrbit size={54} />
              </span>
              <h2>{emptyTitle}</h2>
              <p>
                {current || filtered
                  ? t('assets:assetsPage.tryOtherFilters')
                  : t(
                      'assets:assetsPage.bringInALinkOrAttachAFileInAConversationPisperWillGatherTheOutputsCreatedAlongTheWay',
                    )}
              </p>
              {!current && !filtered && (
                <Button className="mt-4 max-[650px]:h-11" onClick={() => setLinkOpen(true)}>
                  <Link2 />
                  {t('assets:assetsPage.addLink')}
                </Button>
              )}
            </AppEmptyState>
          ) : null}
        </div>
        {preview && (
          <AssetPreviewDialog
            key={preview.generation}
            {...preview}
            fallback={() => fallbackRef.current}
            onClose={() => setPreview(null)}
            onUse={() => {
              void act(preview.asset, 'use')
            }}
            onOpen={() => {
              void act(preview.asset, 'open')
            }}
            busy={pending[preview.asset.id]}
            actionError={actionErrors[preview.asset.id]}
          />
        )}
        {linkOpen && (
          <AssetLinkDialog
            onClose={() => setLinkOpen(false)}
            onCreated={() => {
              if (mounted.current) {
                setLinkOpen(false)
                notify(t('assets:assetsPage.linkAssetAdded'))
              }
              void refreshAssetLists(client)
            }}
          />
        )}
      </div>
    </PageFrame>
  )
}

function AssetCard({
  asset,
  busy,
  error,
  onPreview,
  onAction,
}: {
  asset: Asset
  busy?: Action
  error?: string
  onPreview: (trigger: HTMLElement) => void
  onAction: (action: Action) => void
}) {
  const { t } = useI18n()
  const [failed, setFailed] = useState(false)
  const video = asset.mimeType?.startsWith('video/')
  const Icon =
    asset.kind === 'image' ? FileImage : video ? FileVideo : asset.kind === 'link' ? Link2 : File
  // 预览角标只复述已有元数据：链接显示域名，文件显示扩展名，均用等宽字。
  const tag =
    asset.kind === 'link'
      ? asset.url && new URL(asset.url).hostname
      : /\.([a-z0-9]{1,6})$/i.exec(asset.name)?.[1]
  return (
    <AppCard
      className="asset-card flex h-full min-h-[292px] min-w-0 flex-col overflow-hidden p-0 py-0"
      data-asset-id={asset.id}
    >
      <button
        className="asset-preview relative grid h-44 w-full shrink-0 place-items-center overflow-hidden border-b border-stroke-soft bg-surface-subtle text-content-faint focus-visible:-outline-offset-2"
        aria-label={`${t('assets:assetsPage.preview')}: ${asset.name}`}
        onClick={(event) => onPreview(event.currentTarget)}
      >
        {!failed && asset.kind === 'image' ? (
          <img
            className="size-full object-cover"
            src={assetMediaUrl(asset.id)}
            alt=""
            onError={() => setFailed(true)}
          />
        ) : !failed && video ? (
          <video
            className="size-full object-cover"
            src={assetMediaUrl(asset.id)}
            muted
            preload="metadata"
            onError={() => setFailed(true)}
          />
        ) : (
          <Icon className="size-8" strokeWidth={1.5} />
        )}
        {tag && (
          <span className="absolute top-2 left-2 max-w-[calc(100%-16px)] truncate rounded-xs border border-stroke bg-panel px-1.5 font-mono text-xs leading-5 text-content-secondary">
            {tag}
          </span>
        )}
        {failed && (
          <span className="absolute bottom-2 max-w-[calc(100%-16px)] rounded-xs bg-panel px-2 text-xs leading-5 text-content-muted">
            {t('assets:assetsPage.mediaFailed')}
          </span>
        )}
      </button>
      <div className="flex min-w-0 flex-col gap-1 px-3 pt-2.5">
        <strong
          className="line-clamp-2 text-sm font-medium text-content [overflow-wrap:anywhere]"
          title={asset.name}
        >
          {asset.name}
        </strong>
        <span className="flex min-w-0 items-center gap-1.5 text-xs text-content-muted">
          <span
            className={
              asset.kind === 'link' ? 'min-w-0 truncate font-mono' : 'shrink-0 tabular-nums'
            }
            title={asset.kind === 'link' ? asset.url : undefined}
          >
            {asset.kind === 'link' ? asset.url : formatFileSize(asset.size)}
          </span>
          <span aria-hidden="true" className="text-content-faint">
            ·
          </span>
          <span className="shrink-0">
            {asset.source === 'agent'
              ? t('assets:assetsPage.agentOutputs')
              : asset.source === 'attachment'
                ? t('assets:assetsPage.chatAttachments')
                : t('assets:assetsPage.manualUploads')}
          </span>
        </span>
        {asset.sessionName && (
          <small className="truncate text-xs text-content-muted" title={asset.sessionName}>
            {t('assets:assetsPage.fromName', { name: asset.sessionName })}
          </small>
        )}
      </div>
      {/* 卡片内只放键帽与幽灵按钮：页面唯一的强调色实心键留给页头的「添加链接」。 */}
      <div className="asset-card-actions flex-none mt-auto flex flex-wrap items-center gap-1.5 p-3">
        <Button
          variant="secondary"
          size="sm"
          className="max-[650px]:h-11"
          disabled={Boolean(busy)}
          loading={busy === 'use'}
          onClick={() => onAction('use')}
        >
          <Paperclip />
          {t('assets:assetsPage.useInChat')}
        </Button>
        {asset.kind === 'link' ? (
          <Button asChild variant="ghost" size="sm" className="max-[650px]:h-11">
            <a href={asset.url} target="_blank" rel="noreferrer">
              <ExternalLink />
              {t('assets:assetsPage.open')}
            </a>
          </Button>
        ) : (
          <Button
            variant="ghost"
            size="sm"
            className="max-[650px]:h-11"
            disabled={Boolean(busy)}
            loading={busy === 'open'}
            onClick={() => onAction('open')}
          >
            {canOpenAssetInApplication() ? <ExternalLink /> : <Download />}
            {canOpenAssetInApplication()
              ? t('assets:assetsPage.openInApplication')
              : t('assets:assetsPage.download')}
          </Button>
        )}
        <Button
          variant="ghost"
          size="icon"
          className="ml-auto text-content-muted hover:bg-danger-soft hover:text-danger max-[650px]:size-11"
          disabled={Boolean(busy)}
          loading={busy === 'delete'}
          aria-label={`${t('assets:assetsPage.deleteAsset')}: ${asset.name}`}
          onClick={() => onAction('delete')}
        >
          <Trash2 />
        </Button>
      </div>
      {error && (
        <p role="alert" className="mx-3 mb-3 flex gap-1.5 text-xs break-words text-danger">
          <AlertTriangle className="mt-px size-3.5 shrink-0" />
          {error}
        </p>
      )}
    </AppCard>
  )
}
