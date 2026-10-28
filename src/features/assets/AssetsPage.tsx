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
  return (
    <PageFrame variant="collection" scrollMode="content">
      <div className="flex h-full min-h-0 min-w-0 flex-col gap-3">
        <div className="shrink-0">
          <PageToolbar
            actions={
              <Button
                variant="ghost"
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
                <Button ref={fallbackRef} variant="outline">
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
                variant="secondary"
                size="sm"
                onClick={() => setKind('all')}
                aria-label={t('assets:assetsPage.removeFilter', { filter: kindLabel(kind, t) })}
              >
                {kindLabel(kind, t)}
                <X />
              </Button>
            )}
            {current && (
              <Button
                variant="secondary"
                size="sm"
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
          <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
            <span aria-live="polite">
              {assets && (
                <>
                  <strong className="font-medium text-foreground">{assets.length}</strong>{' '}
                  {t('assets:assetsPage.assets')}
                </>
              )}
            </span>
            <span>
              {t('assets:assetsPage.conversationAttachmentsAndAgentOutputsGatherHereAutomatically')}
            </span>
          </div>
          {!noSession && result.isError && (
            <div
              role="alert"
              className="mt-3 flex flex-wrap items-center gap-2 text-sm text-danger"
            >
              <AlertTriangle className="shrink-0" />
              <span>{result.error.message}</span>
              <Button
                variant="ghost"
                onClick={() => {
                  void result.refetch()
                }}
              >
                {t('assets:assetsPage.retry')}
              </Button>
            </div>
          )}
          {!noSession && result.isFetching && result.data && (
            <p role="status" className="mt-2 text-xs text-muted-foreground">
              {t('assets:assetsPage.refreshingAssets')}
            </p>
          )}
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain" data-assets-results>
          {noSession ? (
            <AppEmptyState>
              <h2>{t('assets:assetsPage.noActiveSession')}</h2>
            </AppEmptyState>
          ) : result.isPending ? (
            <AppEmptyState>
              <RefreshCw className="animate-spin" />
              <h2 role="status">{t('assets:assetsPage.loadingAssets')}</h2>
            </AppEmptyState>
          ) : assets?.length ? (
            <div className="grid auto-rows-[minmax(292px,auto)] grid-cols-[repeat(auto-fill,minmax(min(260px,100%),1fr))] content-start gap-3 p-1">
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
              <StarOrbit size={46} />
              <h2>{emptyTitle}</h2>
              <p>
                {current || filtered
                  ? t('assets:assetsPage.tryOtherFilters')
                  : t(
                      'assets:assetsPage.bringInALinkOrAttachAFileInAConversationPisperWillGatherTheOutputsCreatedAlongTheWay',
                    )}
              </p>
              {!current && !filtered && (
                <Button size="lg" className="mt-4" onClick={() => setLinkOpen(true)}>
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
  return (
    <AppCard
      className="flex h-full min-h-[292px] min-w-0 flex-col overflow-hidden"
      data-asset-id={asset.id}
    >
      <button
        className="relative mb-3 grid h-44 w-full shrink-0 place-items-center overflow-hidden rounded-lg bg-surface-subtle text-muted-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
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
          <Icon size={38} />
        )}
        {failed && (
          <span className="absolute bottom-2 max-w-full rounded bg-background px-2 text-xs">
            {t('assets:assetsPage.mediaFailed')}
          </span>
        )}
      </button>
      <div className="flex min-w-0 flex-col gap-1">
        <strong className="break-words text-sm" title={asset.name}>
          {asset.name}
        </strong>
        <span className="break-words text-xs text-muted-foreground">
          {asset.kind === 'link' ? asset.url : formatFileSize(asset.size)} ·{' '}
          {asset.source === 'agent'
            ? t('assets:assetsPage.agentOutputs')
            : asset.source === 'attachment'
              ? t('assets:assetsPage.chatAttachments')
              : t('assets:assetsPage.manualUploads')}
        </span>
        {asset.sessionName && (
          <small className="break-words text-xs text-muted-foreground">
            {t('assets:assetsPage.fromName', { name: asset.sessionName })}
          </small>
        )}
      </div>
      <div className="mt-auto flex shrink-0 flex-wrap items-center gap-2 pt-3">
        {asset.kind === 'link' ? (
          <Button asChild variant="outline">
            <a href={asset.url} target="_blank" rel="noreferrer">
              <ExternalLink />
              {t('assets:assetsPage.open')}
            </a>
          </Button>
        ) : (
          <Button
            variant="outline"
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
        <Button disabled={Boolean(busy)} loading={busy === 'use'} onClick={() => onAction('use')}>
          <Paperclip />
          {t('assets:assetsPage.useInChat')}
        </Button>
        <Button
          variant="destructive"
          size="icon"
          className="ml-auto"
          disabled={Boolean(busy)}
          loading={busy === 'delete'}
          aria-label={`${t('assets:assetsPage.deleteAsset')}: ${asset.name}`}
          onClick={() => onAction('delete')}
        >
          <Trash2 />
        </Button>
      </div>
      {error && (
        <p role="alert" className="mt-2 break-words text-sm text-danger">
          <AlertTriangle className="mr-1 inline size-4" />
          {error}
        </p>
      )}
    </AppCard>
  )
}
