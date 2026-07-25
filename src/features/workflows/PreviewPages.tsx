import { useCallback, useEffect, useState } from 'react'
import { CircleDot, RefreshCw, Server, Trash2, Wrench } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Badge, Metric, Panel, SectionTitle, Toggle } from '@/components/ui'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { apiJson } from '@/lib/api'
import { relativeTime } from '@/lib/format'
import type { Notify } from '@/app/route-context'
import type { I18nValues } from '@/app/i18n'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { EntityRecord } from '@/types/chat'

type McpService = EntityRecord & { id: string; name: string; status?: string }
type McpTool = EntityRecord & { serviceId: string; name: string }
type McpCall = EntityRecord & { id: string; serviceId: string }
type McpData = EntityRecord & {
  services: McpService[]
  tools?: McpTool[]
  calls?: McpCall[]
  metrics?: EntityRecord
}
type McpPageProps = {
  notify: Notify
  query?: string
  registerPrimaryAction: (action: () => void) => () => void
  requestText?: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
}
type Translate = (message: string, values?: I18nValues) => string

function mcpStatusMeta(
  status: string | undefined,
  t: Translate,
): [string, 'green' | 'amber' | 'gray' | 'red'] {
  if (status === 'online') return [t('workflows:previewPages.online'), 'green']
  if (status === 'connecting') return [t('workflows:previewPages.connecting'), 'amber']
  if (status === 'unauthorized') return [t('workflows:previewPages.unauthorized'), 'gray']
  if (status === 'disabled') return [t('workflows:previewPages.disabled'), 'gray']
  return [t('workflows:previewPages.offline'), 'red']
}

function mcpRiskLabel(risk: unknown, t: Translate) {
  if (risk === 'high' || risk === '高风险') return t('workflows:previewPages.highRisk')
  if (risk === 'medium' || risk === '中风险') return t('workflows:previewPages.mediumRisk')
  if (risk === 'low' || risk === '低风险') return t('workflows:previewPages.lowRisk')
  return String(risk || '')
}

function mcpAuthLabel(service: McpService | null, t: Translate) {
  if (!service) return '—'
  if (service.auth === 'headers')
    return t('workflows:previewPages.countRequestHeaders', { count: service.authCount })
  if (service.auth === 'environment')
    return t('workflows:previewPages.configuredEnvironmentVariables')
  if (service.auth === 'local') return t('workflows:previewPages.localProcess')
  return t('workflows:previewPages.none')
}

export function McpPage({
  notify,
  query = '',
  registerPrimaryAction,
  requestText,
  requestConfirm,
}: McpPageProps) {
  const { t, language } = useI18n()
  const [data, setData] = useState<McpData | null>(null)
  const [selectedId, setSelectedId] = useState('')
  const [busy, setBusy] = useState(false)
  const [, setError] = useState('')

  const load = useCallback(async (refresh = true) => {
    setError('')
    try {
      const result = await apiJson<McpData>(`/api/mcp?refresh=${refresh ? '1' : '0'}`)
      setData(result)
      setSelectedId((current) =>
        result.services.some((service) => service.id === current)
          ? current
          : result.services[0]?.id || '',
      )
      return result
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
      return null
    }
  }, [])

  useEffect(() => {
    void load(true)
    const timer = window.setInterval(() => {
      void load(false)
    }, 10_000)
    return () => window.clearInterval(timer)
  }, [load])

  const addService = useCallback(async () => {
    const spec = await requestText?.({
      title: t('workflows:previewPages.addMCPService'),
      message: t(
        'workflows:previewPages.enterAStreamableHTTPURLAStdioCommandOrJSONContainingHeadersEnv',
      ),
      inputLabel: t('workflows:previewPages.serverConfiguration'),
      placeholder: 'https://server.example.com/mcp',
      maxLength: 12_000,
      confirmLabel: t('workflows:previewPages.continue'),
    })
    if (!spec?.trim()) return
    const approved = await requestConfirm?.({
      title: t('workflows:previewPages.connectMCPServer'),
      message: t(
        'workflows:previewPages.mcpServersCanExposeToolsThatPerformExternalActionsConnectOnlyToServersYouTrust',
      ),
      confirmLabel: t('workflows:previewPages.connect'),
      tone: 'danger',
    })
    if (approved === false) return
    setBusy(true)
    setError('')
    try {
      const result = await apiJson<McpData>('/api/mcp', {
        method: 'POST',
        body: JSON.stringify({ spec }),
      })
      setData(result)
      setSelectedId(result.services.at(-1)?.id || result.services[0]?.id || '')
      notify(t('workflows:previewPages.mcpServerAdded'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }, [notify, requestConfirm, requestText, t])

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
    setBusy(true)
    setError('')
    try {
      setData(
        await apiJson<McpData>(
          `/api/mcp/${encodeURIComponent(tool.serviceId)}/tools/${encodeURIComponent(tool.name)}`,
          {
            method: 'PATCH',
            body: JSON.stringify({ enabled }),
          },
        ),
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }

  const toggleServer = async (enabled: boolean) => {
    if (!selected) return
    setBusy(true)
    setError('')
    try {
      setData(
        await apiJson<McpData>(`/api/mcp/${encodeURIComponent(selected.id)}`, {
          method: 'PATCH',
          body: JSON.stringify({ enabled }),
        }),
      )
      notify(
        enabled
          ? t('workflows:previewPages.mcpServerEnabled')
          : t('workflows:previewPages.mcpServerDisabled'),
        'success',
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }

  const testConnection = async () => {
    if (!selected) return
    setBusy(true)
    setError('')
    try {
      setData(
        await apiJson<McpData>(`/api/mcp/${encodeURIComponent(selected.id)}/test`, {
          method: 'POST',
          body: '{}',
        }),
      )
      notify(t('workflows:previewPages.mcpConnectionTestPassed'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }

  const deleteServer = async () => {
    if (!selected) return
    const approved = await requestConfirm?.({
      title: t('workflows:previewPages.deleteMCPServer'),
      message: t(
        'workflows:previewPages.afterDeletionToolsFromThisServerWillBeRemovedFromSubsequentAgentRuns',
      ),
      confirmLabel: t('workflows:previewPages.delete'),
      tone: 'danger',
    })
    if (approved === false) return
    setBusy(true)
    setError('')
    try {
      await apiJson(`/api/mcp/${encodeURIComponent(selected.id)}`, { method: 'DELETE' })
      const result = await load(false)
      setSelectedId(result?.services[0]?.id || '')
      notify(t('workflows:previewPages.mcpServerDeleted'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="preview-page">
      <div className="mcp-layout">
        <Panel className="selection-list">
          <SectionTitle title={t('workflows:previewPages.services')} />
          {visibleServices.map((service) => {
            const [label, tone] = mcpStatusMeta(service.status, t)
            const location =
              service.transport === 'stdio'
                ? service.workingDirectory || service.command
                : service.endpoint
            return (
              <button
                className={`service-row ${selected?.id === service.id ? 'active' : ''}`}
                onClick={() => setSelectedId(service.id)}
                key={service.id}
              >
                <span className="list-icon">
                  <Server size={15} />
                </span>
                <span>
                  <strong>{service.name}</strong>
                  <small title={location}>{location}</small>
                </span>
                <Badge tone={tone}>{label}</Badge>
              </button>
            )
          })}
        </Panel>
        <div className="mcp-center">
          <div className="metric-grid">
            <Metric
              value={String(metrics.onlineServices)}
              label={t('workflows:previewPages.onlineServices')}
              note={t('workflows:previewPages.countServicesTotal', {
                count: metrics.totalServices,
              })}
              tone="blue"
            />
            <Metric
              value={String(metrics.availableTools)}
              label={t('workflows:previewPages.availableTools')}
              note={t('workflows:previewPages.countRestrictedTools', {
                count: metrics.restrictedTools,
              })}
              tone="green"
            />
            <Metric
              value={`${metrics.errorRate}%`}
              label={t('workflows:previewPages.errorRate')}
              note="24h"
              tone="amber"
            />
          </div>
          <Panel>
            <SectionTitle title={t('workflows:previewPages.toolCapabilities')} />
            {tools.map((tool) => (
              <div className="tool-row" key={tool.piName}>
                <span className="list-icon">
                  <Wrench size={15} />
                </span>
                <span>
                  <strong>{tool.name}</strong>
                  <small>
                    {tool.serviceName} · {tool.description}
                  </small>
                </span>
                <Badge
                  tone={tool.risk === 'high' || tool.risk === '高风险' ? 'red' : tool.risk === 'medium' || tool.risk === '中风险' ? 'amber' : 'green'}
                >
                  {mcpRiskLabel(tool.risk, t)}
                </Badge>
                <Toggle
                  value={tool.enabled}
                  disabled={busy || !tool.serviceEnabled}
                  ariaLabel={t('workflows:previewPages.toggleToolName', { name: tool.name })}
                  onChange={(enabled) => void toggleTool(tool, enabled)}
                />
              </div>
            ))}
          </Panel>
        </div>
        <div className="detail-stack">
          <Panel>
            <SectionTitle title={t('workflows:previewPages.currentService')} />
            <h2>{selected?.name || t('workflows:previewPages.noServerConfigured')}</h2>
            <p className="muted-copy">
              {selected?.error ||
                (selected
                  ? t(
                      'workflows:previewPages.thisServerExposesToolsThroughAStandardMCPTransportEnabledToolsAreRegisteredInNewAgentRuntimes',
                    )
                  : t(
                      'workflows:previewPages.useTheButtonInTheUpperRightToAddAStreamableHTTPOrStdioMCPServer',
                    ))}
            </p>
            {[
              [
                t('workflows:previewPages.transport'),
                selected?.transport === 'stdio'
                  ? 'stdio'
                  : selected?.transport === 'sse'
                    ? 'HTTP + SSE'
                    : 'Streamable HTTP',
              ],
              ...(selected?.transport === 'stdio'
                ? [
                    [t('workflows:previewPages.executable'), selected.command || '—'],
                    [
                      t('workflows:previewPages.workingDirectory'),
                      selected.workingDirectory || '—',
                    ],
                  ]
                : [[t('workflows:previewPages.serverAddress'), selected?.endpoint || '—']]),
              [
                t('workflows:previewPages.latency'),
                selected?.latencyMs == null ? '—' : `${selected.latencyMs} ms`,
              ],
              [
                t('workflows:previewPages.lastPing'),
                selected?.lastPingAt ? relativeTime(selected.lastPingAt, language) : '—',
              ],
              [t('workflows:previewPages.authentication'), mcpAuthLabel(selected, t)],
            ].map((row) => (
              <div className="key-value" key={row[0]}>
                <span>{row[0]}</span>
                <strong title={row[1]}>{row[1]}</strong>
              </div>
            ))}
            <div className="toggle-line">
              <span>{t('workflows:previewPages.serverEnabled')}</span>
              <Toggle
                value={Boolean(selected?.enabled)}
                disabled={!selected || busy}
                ariaLabel={t('workflows:previewPages.toggleMCPServer')}
                onChange={(enabled) => void toggleServer(enabled)}
              />
            </div>
            <div className="button-row">
              <button
                className="button secondary"
                disabled={!selected?.enabled || busy}
                onClick={testConnection}
              >
                <RefreshCw className={busy ? 'spin' : ''} size={14} />
                {t('workflows:previewPages.testConnection')}
              </button>
              <button className="button danger" disabled={!selected || busy} onClick={deleteServer}>
                <Trash2 size={14} />
                {t('workflows:previewPages.delete')}
              </button>
            </div>
          </Panel>
          <Panel>
            <SectionTitle title={t('workflows:previewPages.recentCalls')} />
            {calls.map((activity) => (
              <div className="activity-row" key={activity.id}>
                <CircleDot size={14} />
                <span>
                  <strong>{activity.toolName}</strong>
                  <small>
                    {relativeTime(activity.timestamp, language)} ·{' '}
                    {activity.status === 'ok' ? 'OK' : activity.error || 'Error'} ·{' '}
                    {activity.durationMs} ms
                  </small>
                </span>
              </div>
            ))}
          </Panel>
        </div>
      </div>
    </div>
  )
}
