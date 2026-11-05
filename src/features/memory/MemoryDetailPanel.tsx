// 一条记忆的详情：时间线展开和演算视图选中后共用。内容、来源、证据、生效区间、版本历史和操作都在这里；
// 版本历史要单独请求，先用已有字段显示，取到后再补全。
import { useCallback, useEffect, useState } from 'react'
import { CheckCheck, Pencil, Pin, PinOff, RotateCcw, Trash2, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppError } from '@/components/ui/app-primitives'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { cn } from '@/lib/utils'
import { deleteMemory, fetchMemoryDetail, memoryAction } from './memory-api'
import {
  formatMemoryDate,
  formatMemoryTime,
  memoryBadges,
  memoryEvidenceRoleLabel,
  memoryReasonLabel,
  memorySourceLabel,
  memorySpaceLabel,
  memoryTypeLabel,
} from './memory-labels'
import type { MemoryDetail, MemoryNode, MemorySpace } from './memory-types'

// 只读键值行：左侧弱化标签，右侧值单行省略。
const KEY_VALUE =
  'flex min-h-[34px] min-w-0 items-center justify-between gap-4 border-t border-stroke-soft text-sm [&>dd]:min-w-0 [&>dd]:truncate [&>dd]:text-right [&>dd]:text-content [&>dt]:shrink-0 [&>dt]:text-content-muted'

export function MemoryBadges({ labels }: { labels: string[] }) {
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-1">
      {labels.map((label) => (
        <Badge key={label}>{label}</Badge>
      ))}
    </span>
  )
}

export function MemoryDetailPanel({
  id,
  fallback = null,
  spaces,
  refreshKey = 0,
  withHeading = false,
  onClose,
  onEdit,
  onChanged,
  onDeleted,
  requestConfirm,
}: {
  id: string
  // 列表里已有的字段：详情取回前先显示，不闪空白。
  fallback?: MemoryNode | null
  spaces: MemorySpace[]
  refreshKey?: number
  // 时间线的行本身写了标题，演算视图里要带标题和关闭键。
  withHeading?: boolean
  onClose?: () => void
  onEdit: (node: MemoryNode) => void
  onChanged: (message: string) => Promise<void> | void
  onDeleted: (message: string) => Promise<void> | void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t, language } = useI18n()
  const [detail, setDetail] = useState<MemoryDetail | null>(null)
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState('')
  const [actionError, setActionError] = useState('')
  const [busy, setBusy] = useState(false)
  const [reloadKey, setReloadKey] = useState(0)

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    setLoadError('')
    fetchMemoryDetail(id, controller.signal)
      .then((result) => {
        if (controller.signal.aborted) return
        setDetail(result)
        setLoading(false)
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return
        setLoadError(error instanceof Error ? error.message : String(error))
        setLoading(false)
      })
    return () => controller.abort()
    // refreshKey：编辑、撤销等改动之后重新取一次。
  }, [id, refreshKey, reloadKey])

  const run = useCallback(
    async (action: () => Promise<unknown>, message: string) => {
      setBusy(true)
      setActionError('')
      try {
        await action()
        await onChanged(message)
      } catch (error) {
        setActionError(error instanceof Error ? error.message : String(error))
      } finally {
        setBusy(false)
      }
    },
    [onChanged],
  )

  // 换了一条记忆时不能继续显示上一条。
  const node = detail?.item?.id === id ? detail.item : fallback
  const history = detail?.item?.id === id ? detail.history : []

  const remove = async () => {
    if (!node) return
    const approved = await requestConfirm({
      title: t('memory:memoryPage.deleteMemory'),
      message: t('memory:memoryPage.deleteMemoryName', { name: node.title }),
      confirmLabel: t('memory:memoryPage.delete'),
    })
    if (!approved) return
    setBusy(true)
    setActionError('')
    try {
      await deleteMemory(node.id)
      await onDeleted(t('memory:memoryPage.memoryDeleted'))
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error))
      setBusy(false)
    }
  }

  if (!node) {
    return (
      <div className="flex min-h-16 items-center gap-2 text-xs text-content-muted">
        {loading ? (
          <OrbitSpinner className="size-4" />
        ) : (
          <>
            <AppError className="my-0">
              {loadError || t('memory:memoryPage.detailLoadFailed')}
            </AppError>
            <Button
              variant="secondary"
              size="sm"
              className="max-[650px]:h-11"
              onClick={() => setReloadKey((value) => value + 1)}
            >
              {t('memory:memoryPage.retry')}
            </Button>
          </>
        )}
      </div>
    )
  }

  const space = spaces.find((item) => item.id === node.spaceId)
  const current = node.status === 'active' || node.status === 'stale' || !node.status
  const badges = memoryBadges(node, t)
  return (
    <div className="flex min-w-0 flex-col gap-3" aria-busy={loading || busy || undefined}>
      {withHeading && (
        <header className="flex min-w-0 items-start gap-2">
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <h3 className="text-[15px] leading-6 font-medium break-words text-content">
              {node.title}
            </h3>
            {badges.length > 0 && <MemoryBadges labels={badges} />}
          </div>
          {onClose && (
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="-mt-0.5 flex-none max-[650px]:size-11"
              aria-label={t('memory:memoryPage.closeDetail')}
              onClick={onClose}
            >
              <X />
            </Button>
          )}
        </header>
      )}
      <p className="text-[13px] leading-normal break-words whitespace-pre-wrap text-content-secondary">
        {node.content}
      </p>
      {node.status === 'stale' && (
        <p className="text-xs leading-normal text-content-muted">
          {t('memory:memoryPage.staleHint')}
        </p>
      )}
      {node.tainted && (
        <p className="text-xs leading-normal text-content-muted">
          {t('memory:memoryPage.taintedHint')}
        </p>
      )}
      <dl className="m-0">
        <div className={KEY_VALUE}>
          <dt>{t('memory:memoryPage.memoryType')}</dt>
          <dd>{memoryTypeLabel(node.type, t)}</dd>
        </div>
        {space && (
          <div className={KEY_VALUE}>
            <dt>{t('memory:memoryPage.memorySpace')}</dt>
            <dd>{memorySpaceLabel(space, t)}</dd>
          </div>
        )}
        <div className={KEY_VALUE}>
          <dt>{t('memory:memoryPage.source')}</dt>
          <dd>{memorySourceLabel(node.sourceType, t)}</dd>
        </div>
        <div className={KEY_VALUE}>
          <dt>{t('memory:memoryPage.validFrom')}</dt>
          <dd className="tabular-nums">{formatMemoryTime(node.validFrom, language)}</dd>
        </div>
        <div className={KEY_VALUE}>
          <dt>{t('memory:memoryPage.validTo')}</dt>
          <dd className="tabular-nums">
            {node.validTo
              ? formatMemoryTime(node.validTo, language)
              : t('memory:memoryPage.stillValid')}
          </dd>
        </div>
        <div className={KEY_VALUE}>
          <dt>{t('memory:memoryPage.created')}</dt>
          <dd className="tabular-nums">{formatMemoryTime(node.createdAt, language)}</dd>
        </div>
        <div className={KEY_VALUE}>
          <dt>{t('memory:memoryPage.confirmCount')}</dt>
          <dd className="tabular-nums">{node.confirmCount ?? 1}</dd>
        </div>
        {node.expiresAt && (
          <div className={KEY_VALUE}>
            <dt>{t('memory:memoryPage.expires')}</dt>
            <dd className="tabular-nums">{formatMemoryTime(node.expiresAt, language)}</dd>
          </div>
        )}
        {node.sourcePath && (
          <div className={KEY_VALUE}>
            <dt>{t('memory:memoryPage.relatedFilePath')}</dt>
            <dd className="font-mono text-xs" title={node.sourcePath}>
              {node.sourcePath}
            </dd>
          </div>
        )}
        {node.archiveReason && (
          <div className={KEY_VALUE}>
            <dt>{t('memory:memoryPage.archiveReason')}</dt>
            <dd title={memoryReasonLabel(node.archiveReason, t)}>
              {memoryReasonLabel(node.archiveReason, t)}
            </dd>
          </div>
        )}
      </dl>
      {node.evidence && (
        <section className="min-w-0" aria-label={t('memory:memoryPage.evidence')}>
          <h4 className="mb-1 text-xs font-medium text-content-muted">
            {t('memory:memoryPage.evidenceFrom')} · {memoryEvidenceRoleLabel(node.evidenceRole, t)}
          </h4>
          <blockquote className="m-0 border-l-2 border-stroke pl-2.5 text-xs leading-normal break-words whitespace-pre-wrap text-content-secondary">
            {node.evidence}
          </blockquote>
        </section>
      )}
      {history.length > 1 && (
        <section className="min-w-0" aria-label={t('memory:memoryPage.versionHistory')}>
          <h4 className="mb-1 text-xs font-medium text-content-muted tabular-nums">
            {t('memory:memoryPage.versionHistory')} · {history.length}
          </h4>
          <ol data-memory-history className="m-0 flex list-none flex-col p-0">
            {history.map((version) => {
              const live = version.status === 'active' || version.status === 'stale'
              return (
                <li
                  key={version.id}
                  aria-current={version.id === node.id || undefined}
                  className={cn(
                    'flex min-w-0 flex-col gap-0.5 border-t border-stroke-soft py-1.5 text-xs leading-normal',
                    !live && 'text-content-muted',
                  )}
                >
                  <span className="flex min-w-0 flex-wrap items-center gap-1.5 tabular-nums">
                    {formatMemoryDate(version.validFrom, language)}
                    {live && <Badge>{t('memory:memoryPage.currentVersion')}</Badge>}
                  </span>
                  <span className="break-words whitespace-pre-wrap">{version.content}</span>
                </li>
              )
            })}
          </ol>
        </section>
      )}
      {actionError && <AppError className="my-0">{actionError}</AppError>}
      <div className="flex flex-wrap justify-end gap-2">
        <Button
          variant="ghost"
          size="sm"
          className="text-danger hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
          disabled={busy}
          onClick={() => void remove()}
        >
          <Trash2 />
          {t('memory:memoryPage.delete')}
        </Button>
        {current ? (
          <>
            {node.status === 'stale' && (
              <Button
                variant="secondary"
                size="sm"
                className="max-[650px]:h-11"
                disabled={busy}
                onClick={() =>
                  void run(() => memoryAction(node.id, 'confirm'), t('memory:memoryPage.confirmed'))
                }
              >
                <CheckCheck />
                {t('memory:memoryPage.confirmStillValid')}
              </Button>
            )}
            <Button
              variant="secondary"
              size="sm"
              className="max-[650px]:h-11"
              disabled={busy}
              onClick={() =>
                void run(
                  () => memoryAction(node.id, node.pinned ? 'unpin' : 'pin'),
                  node.pinned ? t('memory:memoryPage.unpinned') : t('memory:memoryPage.pinned'),
                )
              }
            >
              {node.pinned ? <PinOff /> : <Pin />}
              {node.pinned ? t('memory:memoryPage.unpin') : t('memory:memoryPage.pin')}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              className="max-[650px]:h-11"
              disabled={busy}
              onClick={() => onEdit(node)}
            >
              <Pencil />
              {t('memory:memoryPage.edit')}
            </Button>
          </>
        ) : (
          <Button
            variant="secondary"
            size="sm"
            className="max-[650px]:h-11"
            disabled={busy}
            onClick={() =>
              void run(() => memoryAction(node.id, 'restore'), t('memory:memoryPage.restored'))
            }
          >
            <RotateCcw />
            {t('memory:memoryPage.restore')}
          </Button>
        )}
      </div>
    </div>
  )
}
