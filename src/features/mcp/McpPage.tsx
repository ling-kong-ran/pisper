// MCP 页面负责选择与操作编排；远程快照和请求生命周期由领域查询层持有。
import { useCallback, useState, type ReactNode } from 'react'
import { CircleDot, RefreshCw, Server, Trash2, Wrench } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardTitle } from '@/components/ui/card'
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
type McpTone = 'green' | 'amber' | 'gray' | 'red'

const MCP_BADGE_TONES: Record<McpTone, string> = {
  green: 'border-[var(--success-border)] bg-[var(--success-soft)] text-[var(--success-strong)]',
  amber: 'border-[var(--warning-border)] bg-[var(--warning-soft)] text-[var(--warning-strong)]',
  gray: 'border-[var(--stroke)] bg-[var(--surface-muted)] text-[var(--text-muted)]',
  red: 'border-[var(--danger-border)] bg-[var(--danger-soft)] text-[var(--danger)]',
}

function McpPanel({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <Card
      size="sm"
      className={cn(
        'workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] gap-0 p-3.5 py-3.5',
        className,
      )}
    >
      {children}
    </Card>
  )
}

function McpSectionTitle({ children }: { children: ReactNode }) {
  return (
    <CardTitle className="workflow-section-title [.selection-list_&]:mb-[8px] [.node-library_&]:mb-[8px] text-[var(--text-soft)] text-[13px] font-[700] leading-[1.4]">
      {children}
    </CardTitle>
  )
}

function McpMetric({
  value,
  label,
  note,
  tone,
}: {
  value: ReactNode
  label: ReactNode
  note: ReactNode
  tone: 'blue' | 'green' | 'amber'
}) {
  return (
    <Card
      size="sm"
      className={cn(
        "workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] metric [transition:transform_var(--d2)_var(--ease-out),_box-shadow_var(--d2)_var(--ease-out),_border-color_var(--d2)_var(--ease-out)] hover:[transform:translateY(-2px)] hover:shadow-[var(--sh-2)] hover:border-[var(--star-border)] [&_small]:[grid-column:1/-1] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_strong]:mt-[5px] [&_strong]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_strong]:text-[23px] [&_span]:[align-self:end] [&_span]:text-[var(--text-muted)] [&_span]:text-[13px] [&.blue_strong]:text-[var(--star-strong)] [&.green_strong]:text-[var(--success)] [&.amber_strong]:text-[var(--amber)] grid grid-cols-[1fr_auto] [padding:12px] gap-0 p-3 py-3",
        tone,
      )}
    >
      <small>{label}</small>
      <strong>{value}</strong>
      <span>{note}</span>
    </Card>
  )
}

function McpBadge({ children, tone }: { children: ReactNode; tone: McpTone }) {
  return (
    <Badge variant="outline" className={MCP_BADGE_TONES[tone]}>
      {children}
    </Badge>
  )
}

function mcpStatusMeta(status: string | undefined, t: Translate): [string, McpTone] {
  if (status === 'online') return [t('mcp:online'), 'green']
  if (status === 'connecting') return [t('mcp:connecting'), 'amber']
  if (status === 'unauthorized') return [t('mcp:unauthorized'), 'gray']
  if (status === 'disabled') return [t('mcp:disabled'), 'gray']
  return [t('mcp:offline'), 'red']
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
    <div className="preview-page flex min-h-[100%] flex-col">
      <McpHostPanel notify={notify} requestConfirm={requestConfirm} />
      {loading && !error && <p role="status">{t('mcp:loading')}</p>}
      {error && (
        <Alert
          role="status"
          className="mb-3 flex min-w-0 items-center justify-between gap-3 text-muted-foreground [overflow-wrap:anywhere]"
        >
          <span>{error.message}</span>
          <Button variant="ghost" disabled={busy} onClick={() => void reload()}>
            {t('mcp:retry')}
          </Button>
        </Alert>
      )}
      {data && (
        <div className="mcp-layout [.preview-page_>_&]:min-h-0 [.preview-page_>_&]:flex-1 max-[1150px]:grid-cols-[220px_minmax(340px,1fr)] max-[650px]:grid-cols-[1fr] grid min-h-[100%] grid-cols-[230px_minmax(320px,1fr)_300px] gap-[12px]">
          <McpPanel className="selection-list [.config-layout_>_&]:max-h-[calc(100dvh_-_280px)] [.config-layout_>_&]:overflow-y-auto max-[900px]:max-h-[300px] min-h-0 overflow-auto">
            <McpSectionTitle>{t('mcp:services')}</McpSectionTitle>
            {visibleServices.length ? (
              visibleServices.map((service) => {
                const [label, tone] = mcpStatusMeta(service.status, t)
                const location =
                  service.transport === 'stdio'
                    ? service.workingDirectory || service.command
                    : service.endpoint
                return (
                  <button
                    className={`service-row grid w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[9px] border-0 [border-top:1px_solid_var(--stroke-soft)] bg-transparent p-[10px_8px] text-left hover:rounded-[var(--r-sm)] hover:bg-[var(--accent-soft)] [&.active]:rounded-[var(--r-sm)] [&.active]:bg-[var(--accent-soft)] [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap ${selected?.id === service.id ? 'active' : ''}`}
                    onClick={() => setSelectedId(service.id)}
                    key={service.id}
                  >
                    <span className="list-icon [.chat-resource-list_&]:grid [.chat-resource-list_&]:w-[28px] [.chat-resource-list_&]:h-[28px] [.chat-resource-list_&]:place-items-center [.chat-resource-list_&]:rounded-[var(--r-sm)] [.chat-resource-list_&]:bg-[var(--surface-subtle)] [.chat-resource-list_&]:text-[var(--star-strong)] [.session-workflow-summary_&]:grid [.session-workflow-summary_&]:w-[28px] [.session-workflow-summary_&]:h-[28px] [.session-workflow-summary_&]:place-items-center [.session-workflow-summary_&]:rounded-[var(--r-sm)] [.session-workflow-summary_&]:bg-[var(--surface-subtle)] [.session-workflow-summary_&]:text-[var(--star-strong)] grid w-[27px] h-[27px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)] [.workflow-template-gallery_&]:grid [.workflow-template-gallery_&]:w-[32px] [.workflow-template-gallery_&]:h-[32px] [.workflow-template-gallery_&]:place-items-center [.workflow-template-gallery_&]:rounded-[var(--r-sm)] [.workflow-template-gallery_&]:bg-[var(--surface-subtle)] [.workflow-template-gallery_&]:text-[var(--star-strong)]">
                      <Server size={15} />
                    </span>
                    <span>
                      <strong>{service.name}</strong>
                      <small title={location}>{location}</small>
                    </span>
                    <McpBadge tone={tone}>{label}</McpBadge>
                  </button>
                )
              })
            ) : (
              <div className="channel-route-empty [&_strong]:mt-[9px] [&_strong]:text-[var(--text)] [&_strong]:text-[12px] [&_span]:mt-[4px] [&_span]:text-[13px] [&.compact]:min-h-[110px] [.workflow-assets-panel_&]:min-h-[150px] [.workflow-assets-panel_&]:border-0 [.workflow-assets-panel_&]:bg-transparent grid min-h-[185px] place-content-center justify-items-center text-[var(--text-muted)] text-center compact">
                {t('mcp:noServerConfigured')}
              </div>
            )}
          </McpPanel>
          <div className="mcp-center max-[650px]:min-w-0 flex min-w-0 flex-col gap-[12px]">
            <div className="metric-grid max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(3,minmax(0,1fr))] gap-[9px]">
              <McpMetric
                value={String(metrics.onlineServices)}
                label={t('mcp:onlineServices')}
                note={t('mcp:countServicesTotal', {
                  count: metrics.totalServices,
                })}
                tone="blue"
              />
              <McpMetric
                value={String(metrics.availableTools)}
                label={t('mcp:availableTools')}
                note={t('mcp:countRestrictedTools', {
                  count: metrics.restrictedTools,
                })}
                tone="green"
              />
              <McpMetric
                value={`${metrics.errorRate}%`}
                label={t('mcp:errorRate')}
                note="24h"
                tone="amber"
              />
            </div>
            <McpPanel className="mcp-tools-panel min-h-0 flex-1 overflow-y-auto">
              <McpSectionTitle>
                {t('mcp:toolCapabilities')} · {tools.length}
              </McpSectionTitle>
              {tools.length ? (
                tools.map((tool) => (
                  <div
                    className="tool-row [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap grid min-h-[48px] grid-cols-[auto_minmax(0,1fr)_auto_auto] items-center gap-[8px] [border-top:1px_solid_var(--stroke-soft)] [padding:6px_2px]"
                    key={tool.piName}
                  >
                    <span className="list-icon [.chat-resource-list_&]:grid [.chat-resource-list_&]:w-[28px] [.chat-resource-list_&]:h-[28px] [.chat-resource-list_&]:place-items-center [.chat-resource-list_&]:rounded-[var(--r-sm)] [.chat-resource-list_&]:bg-[var(--surface-subtle)] [.chat-resource-list_&]:text-[var(--star-strong)] [.session-workflow-summary_&]:grid [.session-workflow-summary_&]:w-[28px] [.session-workflow-summary_&]:h-[28px] [.session-workflow-summary_&]:place-items-center [.session-workflow-summary_&]:rounded-[var(--r-sm)] [.session-workflow-summary_&]:bg-[var(--surface-subtle)] [.session-workflow-summary_&]:text-[var(--star-strong)] grid w-[27px] h-[27px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)] [.workflow-template-gallery_&]:grid [.workflow-template-gallery_&]:w-[32px] [.workflow-template-gallery_&]:h-[32px] [.workflow-template-gallery_&]:place-items-center [.workflow-template-gallery_&]:rounded-[var(--r-sm)] [.workflow-template-gallery_&]:bg-[var(--surface-subtle)] [.workflow-template-gallery_&]:text-[var(--star-strong)]">
                      <Wrench size={15} />
                    </span>
                    <span>
                      <strong>{tool.name}</strong>
                      <small>
                        {tool.serviceName} · {tool.description}
                      </small>
                    </span>
                    <McpBadge
                      tone={
                        tool.risk === 'high' || tool.risk === '高风险'
                          ? 'red'
                          : tool.risk === 'medium' || tool.risk === '中风险'
                            ? 'amber'
                            : 'green'
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
                  </div>
                ))
              ) : (
                <div className="channel-route-empty [&_strong]:mt-[9px] [&_strong]:text-[var(--text)] [&_strong]:text-[12px] [&_span]:mt-[4px] [&_span]:text-[13px] [&.compact]:min-h-[110px] [.workflow-assets-panel_&]:min-h-[150px] [.workflow-assets-panel_&]:border-0 [.workflow-assets-panel_&]:bg-transparent grid min-h-[185px] place-content-center justify-items-center text-[var(--text-muted)] text-center compact">
                  {t('mcp:noToolsAvailable')}
                </div>
              )}
            </McpPanel>
          </div>
          <div className="detail-stack flex min-w-0 flex-col gap-[12px] [.mcp-layout_>_&]:min-h-0 max-[1150px]:[.memory-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.memory-layout_>_&]:grid max-[1150px]:[.memory-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[1150px]:[.mcp-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.mcp-layout_>_&]:grid max-[1150px]:[.mcp-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[1150px]:[.skills-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.skills-layout_>_&]:grid max-[1150px]:[.skills-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[650px]:[.memory-layout_>_&]:[grid-column:auto] max-[650px]:[.memory-layout_>_&]:grid-cols-[1fr] max-[650px]:[.mcp-layout_>_&]:[grid-column:auto] max-[650px]:[.mcp-layout_>_&]:grid-cols-[1fr] max-[650px]:[.skills-layout_>_&]:[grid-column:auto] max-[650px]:[.skills-layout_>_&]:grid-cols-[1fr]">
            <McpPanel>
              <McpSectionTitle>{t('mcp:currentService')}</McpSectionTitle>
              <h2>{selected?.name || t('mcp:noServerConfigured')}</h2>
              <p className="muted-copy m-[8px_0_14px] text-[var(--text-muted)] text-[12px] leading-[1.55]">
                {selected?.error ||
                  (selected
                    ? t(
                        'mcp:thisServerExposesToolsThroughAStandardMCPTransportEnabledToolsAreRegisteredInNewAgentRuntimes',
                      )
                    : t('mcp:useTheButtonInTheUpperRightToAddAStreamableHTTPOrStdioMCPServer'))}
              </p>
              {[
                [
                  t('mcp:transport'),
                  selected?.transport === 'stdio'
                    ? 'stdio'
                    : selected?.transport === 'sse'
                      ? 'HTTP + SSE'
                      : 'Streamable HTTP',
                ],
                ...(selected?.transport === 'stdio'
                  ? [
                      [t('mcp:executable'), selected.command || '—'],
                      [t('mcp:workingDirectory'), selected.workingDirectory || '—'],
                    ]
                  : [[t('mcp:serverAddress'), selected?.endpoint || '—']]),
                [t('mcp:latency'), selected?.latencyMs == null ? '—' : `${selected.latencyMs} ms`],
                [
                  t('mcp:lastPing'),
                  selected?.lastPingAt ? relativeTime(selected.lastPingAt, language) : '—',
                ],
                [t('mcp:authentication'), mcpAuthLabel(selected, t)],
              ].map((row) => (
                <div
                  className="key-value [&:first-of-type]:mt-[7px] [&_span]:text-[var(--text-muted)] [&_button]:flex [&_button]:items-center [&_button]:gap-[4px] [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[var(--text-soft)] [&_button]:text-[12px] [&_strong]:min-w-0 [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap flex min-h-[31px] items-center justify-between gap-[10px] [border-top:1px_solid_var(--stroke-soft)] text-[13px]"
                  key={row[0]}
                >
                  <span>{row[0]}</span>
                  <strong title={row[1]}>{row[1]}</strong>
                </div>
              ))}
              <div className="toggle-line [&_>_span]:flex [&_>_span]:items-center [&_>_span]:gap-[7px] [&_>_span]:text-[12px] flex min-h-[34px] items-center justify-between [border-top:1px_solid_var(--stroke-soft)]">
                <span>{t('mcp:serverEnabled')}</span>
                <Switch
                  checked={Boolean(selected?.enabled)}
                  disabled={!selected || busy}
                  aria-label={t('mcp:toggleMCPServer')}
                  onCheckedChange={(enabled) => void toggleServer(enabled)}
                />
              </div>
              <div className="mt-[15px] flex gap-2 max-[650px]:flex-wrap">
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={!selected?.enabled || busy}
                  onClick={testConnection}
                >
                  <RefreshCw className={busy ? 'animate-spin' : ''} data-icon="inline-start" />
                  {t('mcp:testConnection')}
                </Button>
                <Button
                  size="sm"
                  variant="destructive"
                  disabled={!selected || busy}
                  onClick={deleteServer}
                >
                  <Trash2 data-icon="inline-start" />
                  {t('mcp:delete')}
                </Button>
              </div>
            </McpPanel>
            <McpPanel className="mcp-calls-panel min-h-0 flex-1 overflow-y-auto">
              <McpSectionTitle>{t('mcp:recentCalls')}</McpSectionTitle>
              {calls.length ? (
                calls.slice(0, 8).map((activity) => (
                  <div
                    className="activity-row [&_span]:flex [&_span]:min-w-0 [&_span]:flex-col [&_span]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_span]:text-[var(--text)] [&.failed]:text-[var(--danger)] [&.running]:text-[var(--star-strong)] grid grid-cols-[auto_minmax(0,1fr)] items-center gap-[9px] [border-top:1px_solid_var(--stroke-soft)] [padding:9px_2px] text-[var(--success)]"
                    key={activity.id}
                  >
                    <CircleDot size={14} />
                    <span>
                      <strong>{activity.toolName}</strong>
                      <small>
                        {relativeTime(activity.timestamp, language)} ·{' '}
                        {activity.status === 'ok'
                          ? t('mcp:callSucceeded')
                          : activity.error || t('mcp:callFailed')}{' '}
                        · {activity.durationMs} ms
                      </small>
                    </span>
                  </div>
                ))
              ) : (
                <div className="channel-route-empty [&_strong]:mt-[9px] [&_strong]:text-[var(--text)] [&_strong]:text-[12px] [&_span]:mt-[4px] [&_span]:text-[13px] [&.compact]:min-h-[110px] [.workflow-assets-panel_&]:min-h-[150px] [.workflow-assets-panel_&]:border-0 [.workflow-assets-panel_&]:bg-transparent grid min-h-[185px] place-content-center justify-items-center text-[var(--text-muted)] text-center compact">
                  {t('mcp:noRecentCalls')}
                </div>
              )}
            </McpPanel>
          </div>
        </div>
      )}
    </div>
  )
}
