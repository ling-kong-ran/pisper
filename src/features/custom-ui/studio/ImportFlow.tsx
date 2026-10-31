// 导入与信任确认（规格 5.7，A8）：[导入 ZIP] 只把包暂存到 Runtime，不安装、不执行；组件库里出现一行
// “待确认导入”（琥珀虚线框：它确实在等你），[检查源码并信任] 打开确认表——文件树与带高亮的源码、manifest、
// 声明的能力、静态扫描与声明的逐项对照、体积与 sha256，替换已安装组件时还有与已安装版本的 diff。
// [信任并启用] 才安装并启用；[取消] 只是关掉，待确认的那一行还在；[丢弃] 删掉暂存。
// 原生组件与应用同源执行，静态扫描只是“看得见”的辅助：文案如实写明它拦不住混淆过的代码。
import { lazy, Suspense, useEffect, useId, useRef, useState, type ReactNode } from 'react'
import { FileArchive, GitCompare, ShieldAlert } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { CodeBlock } from '@/components/ai-elements/code-block'
import { Button } from '@/components/ui/button'
import { Chip } from '@/components/ui/chip'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { formatFileSize } from '@/lib/format'
import { cn } from '@/lib/utils'
import { useQuery } from '@tanstack/react-query'
import type {
  ComponentRow,
  ComponentScan,
  ScanSeverity,
  StagedComponent,
} from '@/features/custom-ui/custom-ui-api'
import { readStagedFile } from '@/features/custom-ui/lifecycle-api'
import { customUiQueryKeys } from '@/features/custom-ui/query-keys'
import { capabilityLabel, surfaceLabel } from './labels'
import { useStagedDetail, type LifecycleActions } from './use-lifecycle'

// 确认表是模态对话框：差异内嵌在左栏（门户里的第二层对话框会被焦点陷阱挡住）。
const InlineGitDiff = lazy(() =>
  import('@/features/chat/public-diff').then((module) => ({ default: module.InlineGitDiff })),
)

type Translate = ReturnType<typeof useI18n>['t']

const PREVIEW_FILE = /\.(?:m?js|css|json|svg|md|txt)$/i

function languageOf(path: string) {
  if (/\.m?js$/i.test(path)) return 'javascript'
  if (/\.css$/i.test(path)) return 'css'
  if (/\.json$/i.test(path)) return 'json'
  if (/\.svg$/i.test(path)) return 'xml'
  return 'text'
}

/**
 * [导入 ZIP]：Runtime 支持暂存导入时是真按钮（选文件 → 暂存 → 直接打开确认表）；
 * 旧 Runtime 置灰并说明原因（按钮本身仍可聚焦读出原因）。
 */
export function StudioImportButton({
  enabled,
  lifecycle,
  onStaged,
  className,
}: {
  enabled: boolean
  lifecycle?: LifecycleActions
  onStaged?: (staged: StagedComponent) => void
  className?: string
}) {
  const { t } = useI18n()
  const input = useRef<HTMLInputElement>(null)
  const [busy, setBusy] = useState(false)
  if (!enabled || !lifecycle)
    return (
      <Button
        size="sm"
        variant="ghost"
        aria-disabled="true"
        title={t('custom-ui:importing.unsupported')}
        aria-label={`${t('custom-ui:studio.importZip')} · ${t('custom-ui:importing.unsupported')}`}
        data-component-import
        className={className}
        onClick={(event) => event.preventDefault()}
      >
        <FileArchive />
        {t('custom-ui:studio.importZip')}
      </Button>
    )
  return (
    <>
      <Button
        size="sm"
        variant="ghost"
        disabled={busy}
        data-component-import
        className={className}
        onClick={() => input.current?.click()}
      >
        <FileArchive />
        {busy ? t('custom-ui:importing.staging') : t('custom-ui:studio.importZip')}
      </Button>
      <input
        ref={input}
        type="file"
        accept=".zip,application/zip"
        hidden
        data-component-import-input
        onChange={async (event) => {
          const file = event.target.files?.[0]
          event.target.value = ''
          if (!file) return
          setBusy(true)
          const staged = await lifecycle.stageImport(file)
          setBusy(false)
          if (staged) onStaged?.(staged)
        }}
      />
    </>
  )
}

/** 组件库里的“待确认导入”行：琥珀虚线框（真的在等你），只有一个主操作。 */
export function StagedRow({
  staged,
  phone = false,
  onReview,
}: {
  staged: StagedComponent
  phone?: boolean
  onReview: () => void
}) {
  const { t } = useI18n()
  return (
    <div
      data-staged-row={staged.stageId}
      className={cn(
        'flex flex-col gap-1.5 rounded-[var(--r-md)] border border-dashed border-st-attn-border bg-st-attn-soft px-2.5 py-2',
        phone && 'px-3.5 py-3',
      )}
    >
      <p className="flex min-w-0 items-center gap-1.5 text-sm">
        <span aria-hidden className="size-1.5 flex-none rounded-full bg-st-attn-fill" />
        <strong className="min-w-0 truncate font-medium">{staged.fileName}</strong>
        <span className="flex-none text-xs text-content-muted">
          {t('custom-ui:importing.shared')}
        </span>
      </p>
      <p className="text-xs text-content-secondary">{t('custom-ui:importing.needsReview')}</p>
      <Button
        size={phone ? 'sm' : 'xs'}
        variant="secondary"
        className={cn('w-fit', phone && 'min-h-11')}
        data-staged-review={staged.stageId}
        onClick={onReview}
      >
        <ShieldAlert />
        {t('custom-ui:importing.review')}
      </Button>
    </div>
  )
}

function severityTone(severity: ScanSeverity) {
  return severity === 'danger' ? 'text-st-fail' : 'text-content-secondary'
}

function ruleLabel(rule: string, t: Translate) {
  switch (rule) {
    case 'fetch':
      return t('custom-ui:scan.ruleFetch')
    case 'xhr':
      return t('custom-ui:scan.ruleXhr')
    case 'websocket':
      return t('custom-ui:scan.ruleWebsocket')
    case 'eval':
      return t('custom-ui:scan.ruleEval')
    case 'function':
      return t('custom-ui:scan.ruleFunction')
    case 'cookie':
      return t('custom-ui:scan.ruleCookie')
    case 'local-storage':
      return t('custom-ui:scan.ruleLocalStorage')
    case 'app-api':
      return t('custom-ui:scan.ruleAppApi')
    case 'dynamic-import':
      return t('custom-ui:scan.ruleDynamicImport')
    default:
      return rule
  }
}

/** 静态扫描结果：主机按声明对照（未声明标红），敏感用法逐条列出（绕过 Pisper 接口的标红）。 */
export function ScanReport({ scan }: { scan: ComponentScan }) {
  const { t } = useI18n()
  if (scan.status !== 'ok')
    return <p className="text-xs text-content-muted">{t('custom-ui:scan.unsupported')}</p>
  const clean = !scan.findings.length && !scan.summary.undeclaredHosts
  return (
    <div data-component-scan className="flex flex-col gap-2">
      <p
        className={cn(
          'text-xs',
          scan.summary.danger || scan.summary.undeclaredHosts
            ? 'text-st-fail'
            : 'text-content-secondary',
        )}
        data-scan-summary
      >
        {clean
          ? t('custom-ui:scan.clean')
          : t('custom-ui:scan.summary', {
              danger: scan.summary.danger,
              warn: scan.summary.warn + scan.summary.info,
              hosts: scan.summary.undeclaredHosts,
            })}
      </p>
      {scan.hosts.length > 0 && (
        <ul className="flex flex-col gap-0.5 text-xs" data-scan-hosts>
          {scan.hosts.map((host) => (
            <li
              key={host.host}
              data-scan-host={host.host}
              data-declared={host.declared}
              className="flex min-w-0 items-baseline gap-2"
            >
              <code
                className={cn(
                  'min-w-0 truncate font-mono',
                  host.declared ? 'text-content-secondary' : 'text-st-fail',
                )}
              >
                {host.host}
              </code>
              <span className={host.declared ? 'text-content-muted' : 'text-st-fail'}>
                {host.declared
                  ? t('custom-ui:scan.hostDeclared')
                  : t('custom-ui:scan.hostUndeclared')}
              </span>
            </li>
          ))}
        </ul>
      )}
      {scan.findings.length > 0 && (
        <ul className="flex flex-col gap-1.5" data-scan-findings>
          {scan.findings.map((finding, index) => (
            <li
              key={`${finding.file}:${finding.line}:${finding.rule}:${index}`}
              data-scan-finding={finding.rule}
              data-severity={finding.severity}
              className="flex min-w-0 flex-col gap-0.5 text-xs"
            >
              <span className={cn('flex items-center gap-1.5', severityTone(finding.severity))}>
                <span
                  aria-hidden
                  className={cn(
                    'size-1.5 flex-none rounded-full',
                    finding.severity === 'danger' ? 'bg-st-fail' : 'bg-content-muted',
                  )}
                />
                {ruleLabel(finding.rule, t)}
                <code className="ml-auto flex-none font-mono text-content-muted">
                  {finding.file}:{finding.line}
                </code>
              </span>
              {finding.excerpt && (
                <code className="block truncate rounded-sm bg-surface-subtle px-1.5 py-0.5 font-mono text-[11px] text-content-secondary">
                  {finding.excerpt}
                </code>
              )}
            </li>
          ))}
        </ul>
      )}
      {scan.truncated && (
        <p className="text-xs text-content-muted">{t('custom-ui:scan.truncated')}</p>
      )}
    </div>
  )
}

function TrustSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5">
      <h3 className="text-xs font-semibold text-content-secondary">{title}</h3>
      {children}
    </section>
  )
}

/** 信任确认表。stageId 为空时关闭。 */
export function TrustDialog({
  stageId,
  lifecycle,
  onClose,
  onTrusted,
}: {
  stageId: string
  lifecycle: LifecycleActions
  onClose: () => void
  onTrusted: (row: ComponentRow) => void
}) {
  const { t } = useI18n()
  const detail = useStagedDetail(stageId)
  const [path, setPath] = useState('')
  const [busy, setBusy] = useState<'' | 'trust' | 'discard'>('')
  const [showDiff, setShowDiff] = useState(false)
  const fileListId = useId()
  const data = detail.data
  const previewable = (data?.files ?? []).filter((file) => PREVIEW_FILE.test(file.path))
  const entry = data?.manifest?.entry ?? ''
  const selected =
    previewable.find((file) => file.path === path)?.path ??
    previewable.find((file) => file.path === entry)?.path ??
    previewable[0]?.path ??
    ''
  useEffect(() => {
    setPath('')
    setShowDiff(false)
  }, [stageId])
  const source = useQuery({
    queryKey: [...customUiQueryKeys.staged(stageId), 'file', selected],
    queryFn: ({ signal }) => readStagedFile(stageId, selected, signal),
    enabled: Boolean(stageId && selected),
    retry: false,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  })
  const staged = data?.staged
  const capabilities = data?.manifest?.capabilities ?? {}
  const declared = (
    ['network', 'notify', 'sessions.read', 'sessions.open', 'storage'] as const
  ).filter((type) => capabilities[type])
  const trust = async () => {
    setBusy('trust')
    const result = await lifecycle.trust(stageId)
    setBusy('')
    if (result) onTrusted(result.component)
  }
  const discard = async () => {
    setBusy('discard')
    const result = await lifecycle.discard(stageId)
    setBusy('')
    if (result) onClose()
  }
  return (
    <Dialog open={Boolean(stageId)} onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent
        data-trust-dialog={stageId}
        scrollMode="body"
        className="h-[min(820px,calc(100dvh-2rem))] max-w-[min(1040px,calc(100vw-2rem))]"
      >
        <DialogHeader>
          <DialogTitle>
            {staged
              ? t('custom-ui:importing.reviewTitle', {
                  name: data?.manifest?.name || staged.name,
                  version: staged.version ? ` v${staged.version}` : '',
                })
              : t('custom-ui:importing.reviewLoading')}
          </DialogTitle>
          <DialogDescription className="flex flex-wrap gap-x-2 gap-y-0.5 font-mono text-xs">
            {staged && (
              <>
                <span>{staged.fileName}</span>
                <span>{formatFileSize(staged.bytes)}</span>
                <span className="break-all" data-trust-sha256>
                  sha256 {staged.sha256}
                </span>
              </>
            )}
          </DialogDescription>
        </DialogHeader>
        <DialogBody className="-mx-4 border-y border-stroke">
          {detail.isPending ? (
            <p className="p-4 text-sm text-content-muted">
              {t('custom-ui:importing.reviewLoading')}
            </p>
          ) : detail.error || !data ? (
            <p className="p-4 text-sm text-content-muted">
              {t('custom-ui:importing.reviewFailed')}
            </p>
          ) : (
            <div className="grid min-h-full grid-cols-1 md:grid-cols-[minmax(0,1fr)_320px]">
              <div className="flex min-h-0 min-w-0 flex-col gap-2 p-4 md:border-r md:border-stroke">
                <div
                  role="group"
                  id={fileListId}
                  aria-label={t('custom-ui:importing.files')}
                  className="flex flex-wrap gap-1"
                  data-trust-files
                >
                  {data.files.map((file) =>
                    PREVIEW_FILE.test(file.path) ? (
                      <Chip
                        key={file.path}
                        size="sm"
                        aria-pressed={file.path === selected}
                        data-trust-file={file.path}
                        className="font-mono"
                        onClick={() => {
                          setShowDiff(false)
                          setPath(file.path)
                        }}
                      >
                        {file.path}
                        <span className="text-content-muted">{formatFileSize(file.size)}</span>
                      </Chip>
                    ) : (
                      <span
                        key={file.path}
                        className="inline-flex items-center gap-1 px-1.5 font-mono text-xs text-content-muted"
                      >
                        {file.path} · {formatFileSize(file.size)}
                      </span>
                    ),
                  )}
                </div>
                {showDiff && data.diff ? (
                  <div
                    data-trust-diff-view
                    className="min-h-[240px] flex-1 overflow-auto rounded-[var(--r-md)] border border-stroke"
                  >
                    <Suspense fallback={null}>
                      <InlineGitDiff
                        diff={data.diff.diff}
                        truncated={data.diff.truncated}
                        mode="unified"
                      />
                    </Suspense>
                  </div>
                ) : selected && source.data ? (
                  <CodeBlock
                    code={source.data.content}
                    language={languageOf(selected)}
                    data-trust-source={selected}
                    className="min-h-[240px] flex-1 overflow-auto bg-code [&>div]:h-full"
                  />
                ) : (
                  <p className="text-xs text-content-muted">
                    {source.isPending && selected
                      ? t('custom-ui:studio.sourceLoading')
                      : t('custom-ui:studio.sourceUnavailable')}
                  </p>
                )}
                {source.data?.truncated && (
                  <p className="text-xs text-content-muted">{t('custom-ui:importing.truncated')}</p>
                )}
              </div>
              <div className="flex min-w-0 flex-col gap-4 p-4">
                {data.installed && (
                  <div
                    data-trust-replaces
                    className="flex flex-col gap-1.5 rounded-[var(--r-md)] border border-stroke bg-surface-subtle px-3 py-2 text-xs text-content-secondary"
                  >
                    <p>
                      {t('custom-ui:importing.replaces', {
                        version: data.installed.version ? `v${data.installed.version}` : '',
                      })}
                    </p>
                    {data.diff && data.diff.files.length > 0 && (
                      <Button
                        size="xs"
                        variant="secondary"
                        className="w-fit"
                        data-trust-diff
                        aria-pressed={showDiff}
                        onClick={() => setShowDiff((value) => !value)}
                      >
                        <GitCompare />
                        {showDiff
                          ? t('custom-ui:importing.hideDiff')
                          : t('custom-ui:importing.viewDiff', { count: data.diff.files.length })}
                      </Button>
                    )}
                  </div>
                )}
                <TrustSection title={t('custom-ui:importing.manifest')}>
                  {data.manifest ? (
                    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 text-xs">
                      <dt className="text-content-muted">id</dt>
                      <dd className="truncate font-mono">{data.manifest.id}</dd>
                      <dt className="text-content-muted">{t('custom-ui:importing.entry')}</dt>
                      <dd className="truncate font-mono">{data.manifest.entry}</dd>
                      <dt className="text-content-muted">{t('custom-ui:studio.placements')}</dt>
                      <dd>
                        {data.manifest.placements
                          .map((placement) => surfaceLabel(placement.surface, t))
                          .join(' · ') || '—'}
                      </dd>
                    </dl>
                  ) : (
                    <p className="text-xs text-st-fail">
                      {t('custom-ui:importing.manifestInvalid')}
                    </p>
                  )}
                </TrustSection>
                <TrustSection title={t('custom-ui:importing.capabilities')}>
                  {declared.length ? (
                    <ul className="flex flex-col gap-1 text-xs" data-trust-capabilities>
                      {declared.map((type) => (
                        <li key={type} className="flex min-w-0 flex-col">
                          <span>
                            {capabilityLabel(type, t)}
                            {type === 'network' && (
                              <code className="ml-1.5 font-mono text-content-secondary">
                                {capabilities.network?.hosts.join(', ')}
                              </code>
                            )}
                          </span>
                          {capabilities[type]?.reason && (
                            <span className="text-content-muted">{capabilities[type]?.reason}</span>
                          )}
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <p className="text-xs text-content-muted">{t('custom-ui:studio.capNone')}</p>
                  )}
                </TrustSection>
                <TrustSection title={t('custom-ui:importing.scan')}>
                  <ScanReport scan={data.scan} />
                </TrustSection>
                <p className="text-xs leading-relaxed text-content-muted">
                  {t('custom-ui:importing.explainer')}
                </p>
              </div>
            </div>
          )}
        </DialogBody>
        <DialogFooter className="flex-row flex-wrap items-center gap-2 sm:justify-between">
          <Button
            variant="ghost"
            size="sm"
            className="text-danger"
            disabled={Boolean(busy)}
            data-trust-discard
            onClick={() => void discard()}
          >
            {t('custom-ui:importing.discard')}
          </Button>
          <span className="ml-auto flex gap-2">
            <Button variant="outline" size="sm" disabled={Boolean(busy)} onClick={onClose}>
              {t('custom-ui:studio.cancel')}
            </Button>
            <Button
              size="sm"
              disabled={Boolean(busy) || !data?.manifest}
              data-trust-confirm
              onClick={() => void trust()}
            >
              {t('custom-ui:importing.trust')}
            </Button>
          </span>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
