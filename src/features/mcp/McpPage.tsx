import { PageFrame } from '@/components/app/PageFrame'
import { ListDetailLayout } from '@/components/app/ListDetailLayout'
import { InlineEmptyState } from '@/components/app/PageState'
// MCP 页面负责选择与操作编排；远程快照和请求生命周期由领域查询层持有。
import { useCallback, useState, type ReactNode } from 'react'
import { Check, History, RefreshCw, Server, Trash2, Wrench, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import { Switch } from '@/components/ui/switch'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { Alert } from '@/components/ui/alert'
import { mcpApi, type McpService, type McpTool } from './mcp-api'
import { McpHostPanel } from './McpHostPanel'
import { useMcpDashboard } from './useMcpDashboard'
import { relativeTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { Notify } from '@/app/route-context'
import type { I18nValues } from '@/app/i18n'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
type McpPageProps = {
  notify: Notify
  query?: string
  registerPrimaryAction: (action: () => void) => () => void
  requestText?: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
}
type Translate = (message: string, values?: I18nValues) => string
type McpTone = 'ok' | 'warn' | 'default' | 'bad'

// 卡片：1px 描边 + r-md，无投影；卡片头位于次级表面，13/500 标题 + 等宽计数。
const CARD = 'min-w-0 overflow-hidden rounded-md border border-stroke bg-panel'
const CARD_HEAD =
  'flex min-h-11 min-w-0 items-center justify-between gap-3 border-b border-stroke-soft bg-surface-subtle px-3.5 py-1.5'
const ICON_KEY =
  'grid size-7 shrink-0 place-items-center rounded-sm border border-key-border bg-key text-content-secondary shadow-key [&_svg]:size-3.5'
const SELECTED_TICK = 'absolute inset-y-2.5 left-0 w-[3px] rounded-r-[3px] bg-star'
// 只读键值行：左侧弱化标签，右侧值单行省略；路径、地址等用等宽字。
const KEY_VALUE =
  'key-value flex min-h-[34px] min-w-0 items-center justify-between gap-4 border-t border-stroke-soft text-sm [&>dt]:shrink-0 [&>dt]:text-content-muted [&>dd]:min-w-0 [&>dd]:truncate [&>dd]:text-right [&>dd]:text-content'

function CardTitle({ children, count }: { children: ReactNode; count?: ReactNode }) {
  return (
    <h2 className="flex min-w-0 items-baseline gap-2 text-sm font-medium text-content">
      {children}
      {count !== undefined && (
        <span className="font-normal text-content-muted tabular-nums">{count}</span>
      )}
    </h2>
  )
}

// 计量读数：12px 标签 + 20px 等宽数字，说明文字弱化；数值保持墨色，不做彩色强调。
function McpMetric({
  value,
  label,
  note,
}: {
  value: ReactNode
  label: ReactNode
  note: ReactNode
}) {
  return (
    <div className="metric flex min-w-0 flex-col gap-1 px-3.5 py-3 max-[650px]:px-3">
      <small className="truncate text-xs text-content-muted">{label}</small>
      <span className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-2">
        <strong className="text-3xl leading-7 font-medium text-content tabular-nums">
          {value}
        </strong>
        <span className="truncate text-xs text-content-muted tabular-nums">{note}</span>
      </span>
    </div>
  )
}

function McpBadge({ children, tone }: { children: ReactNode; tone: McpTone }) {
  return <Badge variant={tone}>{children}</Badge>
}

function mcpStatusMeta(status: string | undefined, t: Translate): [string, McpTone] {
  if (status === 'online') return [t('mcp:online'), 'ok']
  if (status === 'connecting') return [t('mcp:connecting'), 'warn']
  if (status === 'unauthorized') return [t('mcp:unauthorized'), 'default']
  if (status === 'disabled') return [t('mcp:disabled'), 'default']
  return [t('mcp:offline'), 'bad']
}

function mcpRiskLabel(risk: unknown, t: Translate) {
  if (risk === 'high' || risk === '高风险') return t('mcp:highRisk')
  if (risk === 'medium' || risk === '中风险') return t('mcp:mediumRisk')
  if (risk === 'low' || risk === '低风险') return t('mcp:lowRisk')
  return String(risk || '')
}

function mcpAuthLabel(service: McpService | null, t: Translate) {
  if (!service) return '—'
  if (service.auth === 'headers') return t('mcp:countRequestHeaders', { count: service.authCount })
  if (service.auth === 'environment') return t('mcp:configuredEnvironmentVariables')
  if (service.auth === 'local') return t('mcp:localProcess')
  return t('mcp:none')
}

export function McpPage({
  notify,
  query = '',
  registerPrimaryAction,
  requestText,
  requestConfirm,
}: McpPageProps) {
  const [detailOpen, setDetailOpen] = useState(false)

  const { t, language } = useI18n()
  const { data, busy, loading, error, change, reload } = useMcpDashboard()
  const [selectedId, setSelectedId] = useState('')

  const addService = useCallback(async () => {
    const spec = await requestText?.({
      title: t('mcp:addMCPService'),
      message: t('mcp:enterAStreamableHTTPURLAStdioCommandOrJSONContainingHeadersEnv'),
      inputLabel: t('mcp:serverConfiguration'),
      placeholder: 'https://server.example.com/mcp',
      maxLength: 12_000,
      confirmLabel: t('mcp:continue'),
    })
    if (!spec?.trim()) return
    const approved = await requestConfirm?.({
      title: t('mcp:connectMCPServer'),
      message: t(
        'mcp:mcpServersCanExposeToolsThatPerformExternalActionsConnectOnlyToServersYouTrust',
      ),
      confirmLabel: t('mcp:connect'),
      tone: 'danger',
    })
    if (approved === false) return
    try {
      const result = await change(() => mcpApi.add(spec))
      setSelectedId(result.services.at(-1)?.id || result.services[0]?.id || '')
      notify(t('mcp:mcpServerAdded'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      notify(message, 'error')
    }
  }, [change, notify, requestConfirm, requestText, t])

  usePagePrimaryAction(registerPrimaryAction, addService)

  const services = data?.services || []
  const visibleServices = services.filter((service) =>
    `${service.name} ${service.endpoint}`.toLowerCase().includes(query.toLowerCase()),
  )
  const selected =
    services.find((service) => service.id === selectedId) ||
    visibleServices[0] ||
    services[0] ||
    null
  const tools = (data?.tools || []).filter((tool) =>
    `${tool.name} ${tool.serviceName} ${tool.description}`
      .toLowerCase()
      .includes(query.toLowerCase()),
  )
  const calls = (data?.calls || []).filter((call) => !selected || call.serviceId === selected.id)
  const metrics = data?.metrics || {
    totalServices: 0,
    onlineServices: 0,
    availableTools: 0,
    restrictedTools: 0,
    errorRate: 0,
  }

  const toggleTool = async (tool: McpTool, enabled: boolean) => {
    try {
      await change(() => mcpApi.setToolEnabled(tool.serviceId, tool.name, enabled))
    } catch (caught) {
      notify(caught instanceof Error ? caught.message : String(caught), 'error')
    }
  }

  const toggleServer = async (enabled: boolean) => {
    if (!selected) return
    try {
      await change(() => mcpApi.setServerEnabled(selected.id, enabled))
      notify(enabled ? t('mcp:mcpServerEnabled') : t('mcp:mcpServerDisabled'), 'success')
    } catch (caught) {
      notify(caught instanceof Error ? caught.message : String(caught), 'error')
    }
  }

  const testConnection = async () => {
    if (!selected) return
    try {
      await change(() => mcpApi.test(selected.id))
      notify(t('mcp:mcpConnectionTestPassed'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      notify(message, 'error')
    }
  }

  const deleteServer = async () => {
    if (!selected) return
    const approved = await requestConfirm?.({
      title: t('mcp:deleteMCPServer'),
      message: t('mcp:afterDeletionToolsFromThisServerWillBeRemovedFromSubsequentAgentRuns'),
      confirmLabel: t('mcp:delete'),
      tone: 'danger',
    })
    if (approved === false) return
    try {
      const result = await change(() => mcpApi.remove(selected.id))
      setSelectedId(result?.services[0]?.id || '')
      notify(t('mcp:mcpServerDeleted'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      notify(message, 'error')
    }
  }

  return (
    <PageFrame variant="list-detail">
      <div className="flex h-full min-h-0 flex-col gap-3">
        <div className="flex min-h-0 flex-1 flex-col">
          <McpHostPanel notify={notify} requestConfirm={requestConfirm} />
          {loading && !error && (
            <p
              role="status"
              className="flex items-center gap-2 px-4 py-3 text-xs text-content-muted @min-[760px]/page:px-6"
            >
              <OrbitSpinner className="size-3.5" />
              {t('mcp:loading')}
            </p>
          )}
          {error && (
            <Alert
              role="status"
              className="mx-4 my-3 flex min-w-0 items-center justify-between gap-3 text-muted-foreground [overflow-wrap:anywhere] @min-[760px]/page:mx-6"
            >
              <span>{error.message}</span>
              <Button variant="ghost" disabled={busy} onClick={() => void reload()}>
                {t('mcp:retry')}
              </Button>
            </Alert>
          )}
          {data && (
            <ListDetailLayout
              selected={detailOpen}
              onBack={() => setDetailOpen(false)}
              backLabel={t('common:pageTemplate.backToList')}
              list={
                <section className="min-w-0">
                  {/* 列表标题 12/500 弱化文字 + 等宽计数，与其它列表页同一节奏。 */}
                  <h2 className="flex h-[30px] items-end justify-between px-1 pb-1.5 text-xs font-medium tracking-[.02em] text-content-muted">
                    <span>{t('mcp:services')}</span>
                    <span className="font-normal tabular-nums">{visibleServices.length}</span>
                  </h2>
                  {visibleServices.length ? (
                    <ul className="flex flex-col gap-0.5">
                      {visibleServices.map((service) => {
                        const [label, tone] = mcpStatusMeta(service.status, t)
                        const active = selected?.id === service.id
                        const location =
                          service.transport === 'stdio'
                            ? service.workingDirectory || service.command
                            : service.endpoint
                        return (
                          <li
                            className={cn(
                              'relative rounded-md transition-colors duration-(--d-fast) hover:bg-surface-subtle',
                              active && 'bg-surface-muted hover:bg-surface-muted',
                            )}
                            key={service.id}
                          >
                            {active && <i aria-hidden="true" className={SELECTED_TICK} />}
                            <button
                              type="button"
                              aria-current={active || undefined}
                              className={cn(
                                'service-row grid min-h-[56px] w-full min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 py-2 pr-2.5 pl-3 text-left focus-visible:-outline-offset-2',
                                active && 'active',
                              )}
                              onClick={() => {
                                setDetailOpen(true)
                                setSelectedId(service.id)
                              }}
                            >
                              <span className={ICON_KEY}>
                                <Server />
                              </span>
                              <span className="flex min-w-0 flex-col gap-0.5">
                                <strong className="truncate text-sm font-medium text-content max-[650px]:text-[15px]">
                                  {service.name}
                                </strong>
                                <small
                                  className="truncate font-mono text-xs text-content-muted"
                                  title={location}
                                >
                                  {location}
                                </small>
                              </span>
                              <McpBadge tone={tone}>{label}</McpBadge>
                            </button>
                          </li>
                        )
                      })}
                    </ul>
                  ) : (
                    <InlineEmptyState title={t('mcp:noServerConfigured')} />
                  )}
                </section>
              }
              detail={
                <div className="flex min-w-0 flex-col gap-4">
                  <div
                    className={cn('metric-grid grid grid-cols-3 divide-x divide-stroke-soft', CARD)}
                  >
                    <McpMetric
                      value={String(metrics.onlineServices)}
                      label={t('mcp:onlineServices')}
                      note={t('mcp:countServicesTotal', { count: metrics.totalServices })}
                    />
                    <McpMetric
                      value={String(metrics.availableTools)}
                      label={t('mcp:availableTools')}
                      note={t('mcp:countRestrictedTools', { count: metrics.restrictedTools })}
                    />
                    <McpMetric
                      value={`${metrics.errorRate}%`}
                      label={t('mcp:errorRate')}
                      note="24h"
                    />
                  </div>
                  <section className={cn('mcp-tools-panel', CARD)}>
                    <header className={CARD_HEAD}>
                      <CardTitle count={tools.length}>{t('mcp:toolCapabilities')}</CardTitle>
                    </header>
                    {tools.length ? (
                      <ul className="divide-y divide-stroke-soft">
                        {tools.map((tool) => (
                          <li
                            className="tool-row grid min-h-[52px] min-w-0 grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-3 py-2 pr-3 pl-3.5"
                            key={tool.piName}
                          >
                            <span className="flex min-w-0 flex-col gap-0.5">
                              <strong className="truncate font-mono text-[13px] font-medium text-content">
                                {tool.name}
                              </strong>
                              <small className="flex min-w-0 gap-1.5 text-xs text-content-muted">
                                <span className="shrink-0 font-mono">{tool.serviceName}</span>
                                <span aria-hidden="true" className="text-content-faint">
                                  ·
                                </span>
                                <span className="truncate">{tool.description}</span>
                              </small>
                            </span>
                            <McpBadge
                              tone={
                                tool.risk === 'high' || tool.risk === '高风险'
                                  ? 'bad'
                                  : tool.risk === 'medium' || tool.risk === '中风险'
                                    ? 'warn'
                                    : 'ok'
                              }
                            >
                              {mcpRiskLabel(tool.risk, t)}
                            </McpBadge>
                            <Switch
                              checked={Boolean(tool.enabled)}
                              disabled={busy || !tool.serviceEnabled}
                              aria-label={t('mcp:toggleToolName', { name: tool.name })}
                              onCheckedChange={(enabled) => void toggleTool(tool, enabled)}
                            />
                          </li>
                        ))}
                      </ul>
                    ) : (
                      <InlineEmptyState compact icon={Wrench} title={t('mcp:noToolsAvailable')} />
                    )}
                  </section>
                </div>
              }
              inspector={
                <div className="flex min-w-0 flex-col gap-4">
                  <section className={CARD}>
                    <header className="flex min-w-0 flex-col gap-1 px-3.5 pt-3 pb-2.5">
                      <small className="text-xs font-medium tracking-[.02em] text-content-muted">
                        {t('mcp:currentService')}
                      </small>
                      <h2 className="truncate text-[15px] leading-6 font-medium text-content">
                        {selected?.name || t('mcp:noServerConfigured')}
                      </h2>
                      <p
                        className={cn(
                          'text-xs leading-normal [overflow-wrap:anywhere]',
                          selected?.error ? 'font-mono text-danger' : 'text-content-muted',
                        )}
                      >
                        {selected?.error ||
                          (selected
                            ? t(
                                'mcp:thisServerExposesToolsThroughAStandardMCPTransportEnabledToolsAreRegisteredInNewAgentRuntimes',
                              )
                            : t(
                                'mcp:useTheButtonInTheUpperRightToAddAStreamableHTTPOrStdioMCPServer',
                              ))}
                      </p>
                    </header>
                    <dl className="px-3.5">
                      {(
                        [
                          [
                            t('mcp:transport'),
                            selected?.transport === 'stdio'
                              ? 'stdio'
                              : selected?.transport === 'sse'
                                ? 'HTTP + SSE'
                                : 'Streamable HTTP',
                            false,
                          ],
                          ...(selected?.transport === 'stdio'
                            ? [
                                [t('mcp:executable'), selected.command || '—', true],
                                [t('mcp:workingDirectory'), selected.workingDirectory || '—', true],
                              ]
                            : [[t('mcp:serverAddress'), selected?.endpoint || '—', true]]),
                          [
                            t('mcp:latency'),
                            selected?.latencyMs == null ? '—' : `${selected.latencyMs} ms`,
                            false,
                          ],
                          [
                            t('mcp:lastPing'),
                            selected?.lastPingAt
                              ? relativeTime(selected.lastPingAt, language)
                              : '—',
                            false,
                          ],
                          [t('mcp:authentication'), mcpAuthLabel(selected, t), false],
                        ] as Array<[string, string, boolean]>
                      ).map(([label, value, mono]) => (
                        <div className={KEY_VALUE} key={label}>
                          <dt>{label}</dt>
                          <dd className={mono ? 'font-mono text-xs' : 'tabular-nums'} title={value}>
                            {value}
                          </dd>
                        </div>
                      ))}
                      <div className="toggle-line flex min-h-[40px] items-center justify-between gap-3 border-t border-stroke-soft text-sm">
                        <span className="font-medium text-content">{t('mcp:serverEnabled')}</span>
                        <Switch
                          checked={Boolean(selected?.enabled)}
                          disabled={!selected || busy}
                          aria-label={t('mcp:toggleMCPServer')}
                          onCheckedChange={(enabled) => void toggleServer(enabled)}
                        />
                      </div>
                    </dl>
                    <footer className="flex flex-wrap items-center justify-end gap-2 border-t border-stroke-soft bg-surface-subtle px-3.5 py-2.5">
                      <Button
                        size="sm"
                        variant="ghost"
                        className="text-danger hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
                        disabled={!selected || busy}
                        onClick={deleteServer}
                      >
                        <Trash2 data-icon="inline-start" />
                        {t('mcp:delete')}
                      </Button>
                      <Button
                        size="sm"
                        variant="secondary"
                        className="max-[650px]:h-11"
                        loading={busy}
                        disabled={!selected?.enabled || busy}
                        onClick={testConnection}
                      >
                        <RefreshCw data-icon="inline-start" />
                        {t('mcp:testConnection')}
                      </Button>
                    </footer>
                  </section>
                  <section className={cn('mcp-calls-panel', CARD)}>
                    <header className={CARD_HEAD}>
                      <CardTitle count={calls.length ? Math.min(calls.length, 8) : undefined}>
                        {t('mcp:recentCalls')}
                      </CardTitle>
                    </header>
                    {calls.length ? (
                      <ul className="divide-y divide-stroke-soft">
                        {calls.slice(0, 8).map((activity) => {
                          const ok = activity.status === 'ok'
                          return (
                            <li
                              className={cn(
                                'activity-row grid min-h-[44px] min-w-0 grid-cols-[18px_minmax(0,1fr)_auto] items-center gap-2.5 py-2 pr-3.5 pl-3.5',
                                ok ? 'completed' : 'failed',
                              )}
                              key={activity.id}
                            >
                              {/* 状态方块同时用形状（勾/叉）和颜色表达结果。 */}
                              <span
                                className={cn(
                                  'grid size-[18px] place-items-center rounded-xs [&_svg]:size-3 [&_svg]:stroke-[2.5]',
                                  ok
                                    ? 'bg-success-soft text-success'
                                    : 'bg-danger-soft text-danger',
                                )}
                              >
                                {ok ? <Check /> : <X />}
                              </span>
                              <span className="flex min-w-0 flex-col gap-0.5">
                                <strong className="truncate font-mono text-xs font-medium text-content">
                                  {activity.toolName}
                                </strong>
                                <small
                                  className={cn(
                                    'truncate text-xs',
                                    ok ? 'text-content-muted' : 'text-danger',
                                  )}
                                >
                                  {ok
                                    ? t('mcp:callSucceeded')
                                    : activity.error || t('mcp:callFailed')}
                                </small>
                              </span>
                              <span className="flex flex-col items-end gap-0.5 text-xs text-content-muted tabular-nums">
                                <span className="text-content-secondary">
                                  {activity.durationMs} ms
                                </span>
                                <span>{relativeTime(activity.timestamp, language)}</span>
                              </span>
                            </li>
                          )
                        })}
                      </ul>
                    ) : (
                      <InlineEmptyState compact icon={History} title={t('mcp:noRecentCalls')} />
                    )}
                  </section>
                </div>
              }
            />
          )}
        </div>
      </div>
    </PageFrame>
  )
}
