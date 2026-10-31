// 资产页（功能 › 会话与资料）：页面持有搜索、筛选和动作意图；服务器实体只归同域 Query 缓存。
// 用功能页模板（规格 v2 第 10 节 A6 第 2 批）：“‹ 功能”· 标题 · 工具行（搜索、添加链接；筛选菜单与已生效条件、刷新）·
// 资产列表行（缩略图、名称即预览、大小 / 链接与来源、行内操作）。
import { useEffect, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
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
import {
  FeatureAlert,
  FeatureList,
  FeaturePage,
  FeatureRow,
  FeatureSearch,
  FeatureSection,
  FeatureToolbar,
} from '@/components/app/FeaturePage'
import { PageState } from '@/components/app/PageState'
import { FEATURE_SEARCH_INPUT_ID } from '@/app/more-pages'
import { useI18n } from '@/app/use-i18n'
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
import { OrbitSpinner } from '@/components/ui/spinner'
import { formatFileSize } from '@/lib/format'
import { useShortcutLabel } from '@/lib/shortcuts'
import { canOpenAssetInApplication, openAssetInApplication } from '@/lib/open-asset'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { ChatAttachment } from '@/types/chat'
import { AssetLinkDialog } from './AssetLinkDialog'
import { AssetPreviewDialog } from './AssetPreviewDialog'
import { assetMediaUrl, assetsApi, type Asset, type AssetKind } from './assets-api'
import { ASSETS_LIST_KEY, assetsListQueryOptions, refreshAssetLists } from './assets-queries'

type AssetsPageProps = {
  activeSessionId?: string | null
  // 从“功能”进入时由应用壳提供，渲染标题上方的“‹ 功能”。
  onBack?: () => void
  notify: Notify
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
  activeSessionId,
  onBack,
  notify,
  onUse,
  requestConfirm,
}: AssetsPageProps) {
  const { t } = useI18n()
  const client = useQueryClient()
  const searchShortcut = useShortcutLabel('focusSearch')
  // 搜索词归页面自己（换页即清空，与原共享页头一致）；外壳的“/”按固定 id 聚焦这里的搜索框。
  const [query, setQuery] = useState('')
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
  const title = t('navigation:navigation.assets')
  const retry = (
    <Button
      variant="secondary"
      size="sm"
      className="max-[650px]:h-11"
      onClick={() => {
        void result.refetch()
      }}
    >
      {t('assets:assetsPage.retry')}
    </Button>
  )
  return (
    <FeaturePage
      title={title}
      description={t(
        'assets:assetsPage.conversationAttachmentsAndAgentOutputsGatherHereAutomatically',
      )}
      back={
        onBack && {
          label: t('navigation:sidebar.features'),
          ariaLabel: t('navigation:shell.backToFeaturesLabel'),
          onBack,
        }
      }
      toolbar={
        <FeatureToolbar
          search={
            <FeatureSearch
              id={FEATURE_SEARCH_INPUT_ID}
              value={query}
              onValueChange={setQuery}
              placeholder={t('navigation:pageHeader.searchPage', { page: title })}
              shortcut={searchShortcut}
              clearLabel={t('navigation:more.clearSearch')}
            />
          }
          actions={
            // 主操作（原共享页头的“添加链接”）。
            <Button type="button" className="h-9" onClick={() => setLinkOpen(true)}>
              <Link2 />
              {t('assets:assetsPage.addLink')}
            </Button>
          }
          filters={
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    ref={fallbackRef}
                    variant="secondary"
                    size="sm"
                    className="max-[650px]:h-11"
                  >
                    <ListFilter />
                    {t('assets:assetsPage.filters')}
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start">
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
                        className="max-[650px]:min-h-11"
                        onSelect={(event) => event.preventDefault()}
                      >
                        {kindLabel(item, t)}
                      </DropdownMenuRadioItem>
                    ))}
                  </DropdownMenuRadioGroup>
                  <DropdownMenuSeparator />
                  <DropdownMenuCheckboxItem
                    checked={current}
                    className="max-[650px]:min-h-11"
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
              <Button
                variant="ghost"
                size="icon-sm"
                className="ml-auto text-content-muted max-[650px]:size-11 pointer-coarse:size-11"
                loading={result.isFetching && !noSession}
                disabled={noSession || result.isFetching}
                aria-label={t('assets:assetsPage.refreshAssets')}
                title={t('assets:assetsPage.refreshAssets')}
                onClick={() => {
                  void result.refetch()
                }}
              >
                <RefreshCw />
              </Button>
            </div>
          }
        />
      }
    >
      {noSession ? (
        <PageState kind="empty" title={t('assets:assetsPage.noActiveSession')} />
      ) : result.isPending && !result.isError ? (
        <PageState kind="loading" title={t('assets:assetsPage.loadingAssets')} />
      ) : !assets ? (
        // 首次读取失败（含换了搜索词后的读取）：写明错误并给重试，不显示成“还没有资产”。
        <PageState
          kind="error"
          title={t('assets:assetsPage.loadFailed')}
          description={result.error?.message}
          action={retry}
        />
      ) : (
        <FeatureSection
          data-assets-results
          title={kindLabel(kind, t)}
          count={assets.length}
          meta={
            // 刷新中保留现有结果，只在分区标题旁说一句。
            result.isFetching && (
              <span role="status" className="inline-flex items-center gap-1.5">
                <OrbitSpinner className="size-3.5" />
                {t('assets:assetsPage.refreshingAssets')}
              </span>
            )
          }
        >
          {/* 同一条件刷新失败：保留可操作的结果，错误与重试写在列表上方。 */}
          {result.isError && (
            <FeatureAlert className="mt-0 mb-2">
              <div className="flex flex-wrap items-center gap-2">
                <span className="min-w-0 flex-1">{result.error.message}</span>
                {retry}
              </div>
            </FeatureAlert>
          )}
          {assets.length ? (
            <FeatureList>
              {assets.map((asset) => (
                <AssetRow
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
            </FeatureList>
          ) : (
            !result.isError && (
              <PageState
                kind="empty"
                title={emptyTitle}
                description={
                  current || filtered
                    ? t('assets:assetsPage.tryOtherFilters')
                    : t(
                        'assets:assetsPage.bringInALinkOrAttachAFileInAConversationPisperWillGatherTheOutputsCreatedAlongTheWay',
                      )
                }
              />
            )
          )}
        </FeatureSection>
      )}
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
    </FeaturePage>
  )
}

// 资产行（规格 A6：列表行而不是卡片墙）：左侧 36 缩略图（图片 / 视频首帧，读不出来时换类型图标），
// 名称区是“预览”按钮；附注是扩展名或域名，说明是大小或链接 · 来源，再下一行是来自哪个会话。
// 右侧：用于对话 · 打开 / 下载 · 删除；手机上按钮只留图标（读屏名称不变），名称不被挤掉。
function AssetRow({
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
  // 附注只复述已有元数据：链接显示域名，文件显示扩展名，均用等宽字。
  const tag =
    asset.kind === 'link'
      ? asset.url && new URL(asset.url).hostname
      : /\.([a-z0-9]{1,6})$/i.exec(asset.name)?.[1]
  const thumbnail =
    !failed && asset.kind === 'image' ? (
      <img
        className="size-full rounded-[inherit] object-cover"
        src={assetMediaUrl(asset.id)}
        alt=""
        loading="lazy"
        onError={() => setFailed(true)}
      />
    ) : !failed && video ? (
      <video
        className="size-full rounded-[inherit] object-cover"
        src={assetMediaUrl(asset.id)}
        muted
        preload="metadata"
        onError={() => setFailed(true)}
      />
    ) : (
      <Icon />
    )
  return (
    <FeatureRow
      className="asset-row"
      data-asset-id={asset.id}
      icon={thumbnail}
      onOpen={(event) => onPreview(event.currentTarget)}
      openLabel={`${t('assets:assetsPage.preview')}: ${asset.name}`}
      name={<span title={asset.name}>{asset.name}</span>}
      nameMeta={tag || undefined}
      description={
        <>
          <span
            className={asset.kind === 'link' ? 'font-mono' : 'tabular-nums'}
            title={asset.kind === 'link' ? asset.url : undefined}
          >
            {asset.kind === 'link' ? asset.url : formatFileSize(asset.size)}
          </span>
          <span aria-hidden="true"> · </span>
          {asset.source === 'agent'
            ? t('assets:assetsPage.agentOutputs')
            : asset.source === 'attachment'
              ? t('assets:assetsPage.chatAttachments')
              : t('assets:assetsPage.manualUploads')}
        </>
      }
      meta={
        (asset.sessionName || failed) && (
          <>
            {asset.sessionName && (
              <span className="min-w-0 truncate" title={asset.sessionName}>
                {t('assets:assetsPage.fromName', { name: asset.sessionName })}
              </span>
            )}
            {asset.sessionName && failed && <span aria-hidden="true">·</span>}
            {failed && <span className="shrink-0">{t('assets:assetsPage.mediaFailed')}</span>}
          </>
        )
      }
      actions={
        // 行内只放键帽与幽灵按钮：页面唯一的实心主操作留给工具行的「添加链接」。
        <div className="asset-row-actions flex items-center gap-1">
          <Button
            variant="secondary"
            size="sm"
            className="max-[650px]:size-11 max-[650px]:px-0"
            disabled={Boolean(busy)}
            loading={busy === 'use'}
            onClick={() => onAction('use')}
          >
            <Paperclip />
            <span className="max-[650px]:sr-only">{t('assets:assetsPage.useInChat')}</span>
          </Button>
          {asset.kind === 'link' ? (
            <Button
              asChild
              variant="ghost"
              size="sm"
              className="max-[650px]:size-11 max-[650px]:px-0"
            >
              <a href={asset.url} target="_blank" rel="noreferrer">
                <ExternalLink />
                <span className="max-[650px]:sr-only">{t('assets:assetsPage.open')}</span>
              </a>
            </Button>
          ) : (
            <Button
              variant="ghost"
              size="sm"
              className="max-[650px]:size-11 max-[650px]:px-0"
              disabled={Boolean(busy)}
              loading={busy === 'open'}
              onClick={() => onAction('open')}
            >
              {canOpenAssetInApplication() ? <ExternalLink /> : <Download />}
              <span className="max-[650px]:sr-only">
                {canOpenAssetInApplication()
                  ? t('assets:assetsPage.openInApplication')
                  : t('assets:assetsPage.download')}
              </span>
            </Button>
          )}
          <Button
            variant="ghost"
            size="icon-sm"
            className="text-content-muted hover:bg-danger-soft hover:text-danger max-[650px]:size-11"
            disabled={Boolean(busy)}
            loading={busy === 'delete'}
            aria-label={`${t('assets:assetsPage.deleteAsset')}: ${asset.name}`}
            onClick={() => onAction('delete')}
          >
            <Trash2 />
          </Button>
        </div>
      }
      detail={error ? <FeatureAlert className="mt-0">{error}</FeatureAlert> : undefined}
    />
  )
}
