// 设置栏 · 版本（规格 5.1、5.9）：“v3 当前 · v2 · v1”，旧版本各有 [对比] [回滚]。
// 对比 = 那一版 → 当前的统一 diff，用会话改动的 diff 查看器（内嵌在 Radix 对话框里：设置栏收进抽屉时
// 本身就在模态层内，门户里的另一层对话框会被焦点陷阱挡住）；回滚先确认，回滚会生成新的一版、不改写历史。
// 当前版一行写明这一版怎么来的（由代理生成 / 导入 / 回滚）与改动文件，和提示词卡片的结果行同一份数据。
import { lazy, Suspense, useRef, useState } from 'react'
import { GitCompare, History } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { useOverlayFocusReturn } from '@/hooks/useOverlayFocusReturn'
import { relativeTime } from '@/lib/format'
import { componentErrorCode, type ComponentRow } from '@/features/custom-ui/custom-ui-api'
import {
  diffComponentVersion,
  type ComponentDiff,
  type ComponentVersion,
} from '@/features/custom-ui/lifecycle-api'
import { componentErrorText } from './error-text'
import { versionSourceLabel } from './labels'
import { StudioSection } from './StudioSection'
import { useComponentVersions, type LifecycleActions } from './use-lifecycle'

const InlineGitDiff = lazy(() =>
  import('@/features/chat/public-diff').then((module) => ({ default: module.InlineGitDiff })),
)

type Translate = ReturnType<typeof useI18n>['t']

/** 改动文件芯片：“component.js +42 −8”；删除的文件单独标出。 */
export function VersionFileChips({ version }: { version: ComponentVersion }) {
  const { t } = useI18n()
  if (!version.files.length && !version.deleted.length) return null
  return (
    <span className="flex min-w-0 flex-wrap gap-1" data-version-files>
      {version.files.map((file) => (
        <span
          key={file.path}
          className="inline-flex max-w-full items-center gap-1 rounded-sm border border-stroke bg-surface-subtle px-1.5 font-mono text-[11px] text-content-secondary"
        >
          <span className="truncate">{file.path}</span>
          {file.added > 0 && <span className="text-st-done tabular-nums">+{file.added}</span>}
          {file.removed > 0 && <span className="text-st-fail tabular-nums">−{file.removed}</span>}
        </span>
      ))}
      {version.deleted.map((path) => (
        <span
          key={`deleted:${path}`}
          className="inline-flex max-w-full items-center gap-1 rounded-sm border border-stroke px-1.5 font-mono text-[11px] text-content-muted line-through"
          title={t('custom-ui:versions.deletedFile')}
        >
          {path}
        </span>
      ))}
    </span>
  )
}

function versionMeta(version: ComponentVersion, t: Translate, language: string) {
  return [
    versionSourceLabel(version, t),
    version.summary,
    version.at ? relativeTime(version.at, language) : '',
  ]
    .filter(Boolean)
    .join(' · ')
}

export function VersionsSection({
  row,
  enabled,
  notify,
  actions,
}: {
  row: ComponentRow
  enabled: boolean
  notify: (message: string) => void
  actions: LifecycleActions
}) {
  const { t, language } = useI18n()
  const versions = useComponentVersions(row.id, enabled)
  const [diff, setDiff] = useState<(ComponentDiff & { n: number }) | null>(null)
  const [loadingDiff, setLoadingDiff] = useState(0)
  const [confirming, setConfirming] = useState<ComponentVersion | null>(null)
  const [busy, setBusy] = useState(false)
  const lastTrigger = useRef<HTMLElement | null>(null)
  const returnFocus = useOverlayFocusReturn(Boolean(confirming))
  if (!enabled) return null
  const data = versions.data
  const compare = async (version: ComponentVersion, trigger: HTMLElement) => {
    lastTrigger.current = trigger
    setLoadingDiff(version.n)
    try {
      setDiff({ ...(await diffComponentVersion(row.id, version.n)), n: version.n })
    } catch (error) {
      const code = componentErrorCode(error)
      notify(
        code
          ? `${t('custom-ui:versions.diffFailed')} ${componentErrorText(code, t)}`
          : t('custom-ui:versions.diffFailed'),
      )
    } finally {
      setLoadingDiff(0)
    }
  }
  const restore = async () => {
    if (!confirming) return
    setBusy(true)
    const result = await actions.restoreVersion(row, confirming.n)
    setBusy(false)
    if (result) setConfirming(null)
  }
  return (
    <StudioSection
      title={t('custom-ui:versions.title')}
      aside={data ? t('custom-ui:versions.keep', { count: data.max }) : undefined}
    >
      {versions.isPending ? (
        <p className="text-xs text-content-muted">{t('custom-ui:versions.loading')}</p>
      ) : versions.error || !data ? (
        <p className="text-xs text-content-muted">{t('custom-ui:versions.failed')}</p>
      ) : (
        <ol
          data-component-versions
          className="overflow-hidden rounded-[var(--r-md)] border border-stroke"
        >
          <li
            data-component-version={data.current.n}
            data-version-current
            className="flex min-w-0 flex-col gap-1 border-b border-stroke px-3 py-2 last:border-b-0"
          >
            <span className="flex min-w-0 items-baseline gap-2 text-sm">
              <strong className="font-mono font-medium tabular-nums">v{data.current.n}</strong>
              <span className="text-xs text-content-secondary">
                {t('custom-ui:versions.current')}
              </span>
              <span className="min-w-0 truncate text-xs text-content-muted">
                {versionMeta(data.current, t, language)}
              </span>
            </span>
            <VersionFileChips version={data.current} />
          </li>
          {data.versions.map((version) => (
            <li
              key={version.n}
              data-component-version={version.n}
              className="flex min-w-0 items-center gap-2 border-b border-stroke px-3 py-1.5 last:border-b-0"
            >
              <strong className="flex-none font-mono text-sm font-medium tabular-nums">
                v{version.n}
              </strong>
              <span className="min-w-0 flex-1 truncate text-xs text-content-muted">
                {versionMeta(version, t, language) || t('custom-ui:versions.noSummary')}
              </span>
              <Button
                size="xs"
                variant="ghost"
                disabled={loadingDiff === version.n}
                data-version-compare={version.n}
                onClick={(event) => void compare(version, event.currentTarget)}
              >
                <GitCompare />
                {t('custom-ui:versions.compare')}
              </Button>
              <Button
                size="xs"
                variant="ghost"
                data-version-restore={version.n}
                onClick={(event) => {
                  lastTrigger.current = event.currentTarget
                  setConfirming(version)
                }}
              >
                <History />
                {t('custom-ui:versions.restore')}
              </Button>
            </li>
          ))}
          {!data.versions.length && (
            <li className="px-3 py-2 text-xs text-content-muted">
              {t('custom-ui:versions.onlyCurrent')}
            </li>
          )}
        </ol>
      )}
      <Dialog
        open={Boolean(confirming)}
        onOpenChange={(open) => !open && !busy && setConfirming(null)}
      >
        <DialogContent onCloseAutoFocus={returnFocus}>
          <DialogHeader>
            <DialogTitle>
              {t('custom-ui:versions.restoreTitle', { n: confirming?.n ?? 0 })}
            </DialogTitle>
            <DialogDescription>
              {t('custom-ui:versions.restoreHint', {
                n: confirming?.n ?? 0,
                next: (data?.current.n ?? 0) + 1,
                current: data?.current.n ?? 0,
              })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" disabled={busy} onClick={() => setConfirming(null)}>
              {t('custom-ui:studio.cancel')}
            </Button>
            <Button disabled={busy} data-version-restore-confirm onClick={() => void restore()}>
              {t('custom-ui:versions.restoreConfirm', { n: confirming?.n ?? 0 })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog open={Boolean(diff)} onOpenChange={(open) => !open && setDiff(null)}>
        <DialogContent
          data-version-diff={diff?.n}
          scrollMode="body"
          className="h-[min(760px,calc(100dvh-2rem))] max-w-[min(960px,calc(100vw-2rem))]"
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            lastTrigger.current?.focus()
          }}
        >
          <DialogHeader>
            <DialogTitle>
              {t('custom-ui:versions.diffTitle', {
                n: diff?.n ?? 0,
                current: data?.current.n ?? 0,
              })}
            </DialogTitle>
            <DialogDescription>
              {diff?.files.length
                ? diff.files.map((file) => file.path).join(' · ')
                : t('custom-ui:versions.noDiff')}
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="-mx-4 border-t border-stroke">
            {diff && diff.files.length > 0 && (
              <Suspense fallback={null}>
                <InlineGitDiff diff={diff.diff} truncated={diff.truncated} mode="unified" />
              </Suspense>
            )}
          </DialogBody>
        </DialogContent>
      </Dialog>
    </StudioSection>
  )
}
