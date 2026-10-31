// MCP 页（功能 › 扩展 › MCP）：页面负责选择与操作编排；远程快照和请求生命周期由领域查询层持有。
// 外形用功能页模板（规格 A6 第 1 批）：工具行放搜索与主操作“添加服务”；内置服务、外部服务、工具能力
// 都是列表行。服务行展开后是原右侧检查器的内容（传输方式、地址、延迟、认证、最近调用、测试与删除），
// 服务的启停开关直接在行上；数据流与改版前相同。
import {
  FeatureList,
  FeaturePage,
  FeatureRow,
  FeatureSearch,
  FeatureSection,
  FeatureToolbar,
} from '@/components/app/FeaturePage'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { useCallback, useState } from 'react'
import { Check, History, Plus, RefreshCw, Server, Trash2, Wrench, X } from 'lucide-react'
import { FEATURE_SEARCH_INPUT_ID } from '@/app/more-pages'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Alert } from '@/components/ui/alert'
import { mcpApi, type McpService, type McpTool } from './mcp-api'
import { McpHostPanel } from './McpHostPanel'
import { useMcpDashboard } from './useMcpDashboard'
import { relativeTime } from '@/lib/format'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import type { Notify } from '@/app/route-context'
import type { I18nValues } from '@/app/i18n'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
type McpPageProps = {
  notify: Notify
  // 从功能进入时由应用壳提供：标题上方的“‹ 功能”。
  onBack?: () => void
  requestText?: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
}
type Translate = (message: string, values?: I18nValues) => string

// 展开详情里的只读键值：左侧弱化标签，右侧值可换行；路径、地址等用等宽字。
const KEY_VALUES =
  'grid grid-cols-[88px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs [&_dd]:min-w-0 [&_dd]:[overflow-wrap:anywhere] [&_dd]:text-content-soft [&_dt]:text-content-muted [&>div]:contents'

// 服务状态：形状 + 文字。琥珀只表示“需要你”（规格 6.1）——离线与未授权要人去处理（与渠道“断开”同一规则），
// 在线、连接中、已禁用都是中性灰。
function mcpStatusMeta(status: string | undefined, t: Translate): [string, boolean] {
  if (status === 'online') return [t('mcp:online'), false]
  if (status === 'connecting') return [t('mcp:connecting'), false]
  if (status === 'unauthorized') return [t('mcp:unauthorized'), true]
  if (status === 'disabled') return [t('mcp:disabled'), false]
  return [t('mcp:offline'), true]
}

function mcpRiskLabel(risk: unknown, t: Translate) {
  if (risk === 'high' || risk === '高风险') return t('mcp:highRisk')
  if (risk === 'medium' || risk === '中风险') return t('mcp:mediumRisk')
  if (risk === 'low' || risk === '低风险') return t('mcp:lowRisk')
  return String(risk || '')
}

function mcpAuthLabel(service: McpService, t: Translate) {
  if (service.auth === 'headers') return t('mcp:countRequestHeaders', { count: service.authCount })
  if (service.auth === 'environment') return t('mcp:configuredEnvironmentVariables')
  if (service.auth === 'local') return t('mcp:localProcess')
  return t('mcp:none')
}

export function McpPage({ notify, onBack, requestText, requestConfirm }: McpPageProps) {
  const { t, language } = useI18n()
  const searchShortcut = useShortcutLabel('focusSearch')
  const { data, busy, loading, error, change, reload } = useMcpDashboard()
  const [query, setQuery] = useState('')
  // 展开详情的服务（同一时刻只展开一行，原右侧检查器的“当前服务”）。
  const [expandedId, setExpandedId] = useState('')

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
      setExpandedId(result.services.at(-1)?.id || '')
      notify(t('mcp:mcpServerAdded'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      notify(message, 'error')
    }
  }, [change, notify, requestConfirm, requestText, t])

  const normalizedQuery = query.trim().toLowerCase()
  const services = data?.services || []
  const visibleServices = services.filter((service) =>
    `${service.name} ${service.endpoint}`.toLowerCase().includes(normalizedQuery),
  )
  const tools = (data?.tools || []).filter((tool) =>
    `${tool.name} ${tool.serviceName} ${tool.description}`.toLowerCase().includes(normalizedQuery),
  )
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

  const toggleServer = async (service: McpService, enabled: boolean) => {
    try {
      await change(() => mcpApi.setServerEnabled(service.id, enabled))
      notify(enabled ? t('mcp:mcpServerEnabled') : t('mcp:mcpServerDisabled'), 'success')
    } catch (caught) {
      notify(caught instanceof Error ? caught.message : String(caught), 'error')
    }
  }

  const testConnection = async (service: McpService) => {
    try {
      await change(() => mcpApi.test(service.id))
      notify(t('mcp:mcpConnectionTestPassed'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      notify(message, 'error')
    }
  }

  const deleteServer = async (service: McpService) => {
    const approved = await requestConfirm?.({
      title: t('mcp:deleteMCPServer'),
      message: t('mcp:afterDeletionToolsFromThisServerWillBeRemovedFromSubsequentAgentRuns'),
      confirmLabel: t('mcp:delete'),
      tone: 'danger',
    })
    if (approved === false) return
    try {
      await change(() => mcpApi.remove(service.id))
      setExpandedId('')
      notify(t('mcp:mcpServerDeleted'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      notify(message, 'error')
    }
  }

  // 服务展开详情：说明或错误、连接信息、最近调用（最多 8 条），底部删除与测试连接。
  const renderServiceDetail = (service: McpService) => {
    const calls = (data?.calls || []).filter((call) => call.serviceId === service.id)
    return (
      <div className="mcp-service-detail grid gap-3">
        <p
          className={cn(
            'text-xs leading-normal [overflow-wrap:anywhere]',
            service.error ? 'font-mono text-content-secondary' : 'text-content-muted',
          )}
        >
          {service.error ||
            t(
              'mcp:thisServerExposesToolsThroughAStandardMCPTransportEnabledToolsAreRegisteredInNewAgentRuntimes',
            )}
        </p>
        <dl className={KEY_VALUES}>
          {(
            [
              [
                t('mcp:transport'),
                service.transport === 'stdio'
                  ? 'stdio'
                  : service.transport === 'sse'
                    ? 'HTTP + SSE'
                    : 'Streamable HTTP',
                false,
              ],
              ...(service.transport === 'stdio'
                ? [
                    [t('mcp:executable'), service.command || '—', true],
                    [t('mcp:workingDirectory'), service.workingDirectory || '—', true],
                  ]
                : [[t('mcp:serverAddress'), service.endpoint || '—', true]]),
              [
                t('mcp:latency'),
                service.latencyMs == null ? '—' : `${service.latencyMs} ms`,
                false,
              ],
              [
                t('mcp:lastPing'),
                service.lastPingAt ? relativeTime(service.lastPingAt, language) : '—',
                false,
              ],
              [t('mcp:authentication'), mcpAuthLabel(service, t), false],
            ] as Array<[string, string, boolean]>
          ).map(([label, value, mono]) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd className={mono ? 'font-mono' : 'tabular-nums'}>{value}</dd>
            </div>
          ))}
        </dl>
        <section className="mcp-calls-panel min-w-0" aria-label={t('mcp:recentCalls')}>
          <h3 className="mb-1 text-xs font-medium text-content-muted tabular-nums">
            {t('mcp:recentCalls')}
            {calls.length > 0 && <span> · {Math.min(calls.length, 8)}</span>}
          </h3>
          {calls.length ? (
            <ul role="list" className="m-0 list-none divide-y divide-stroke p-0">
              {calls.slice(0, 8).map((activity) => {
                const ok = activity.status === 'ok'
                return (
                  <li
                    className={cn(
                      'activity-row grid min-h-10 min-w-0 grid-cols-[16px_minmax(0,1fr)_auto] items-center gap-2.5 py-1.5',
                      ok ? 'completed' : 'failed',
                    )}
                    key={activity.id}
                  >
                    {/* 结果用形状（勾 / 叉）+ 文字区分，中性灰：历史调用不是要人处理的事（规格 6.1、6.5）。 */}
                    <span
                      aria-hidden="true"
                      className="grid size-4 place-items-center text-content-muted [&_svg]:size-3.5 [&_svg]:stroke-[2]"
                    >
                      {ok ? <Check /> : <X />}
                    </span>
                    <span className="flex min-w-0 flex-col">
                      <strong className="truncate font-mono text-xs font-medium text-content">
                        {activity.toolName}
                      </strong>
                      <small className="truncate text-xs text-content-muted">
                        {ok ? t('mcp:callSucceeded') : activity.error || t('mcp:callFailed')}
                      </small>
                    </span>
                    <span className="flex flex-col items-end text-xs text-content-muted tabular-nums">
                      <span className="text-content-secondary">{activity.durationMs} ms</span>
                      <span>{relativeTime(activity.timestamp, language)}</span>
                    </span>
                  </li>
                )
              })}
            </ul>
          ) : (
            <p className="flex items-center gap-1.5 py-1 text-xs text-content-muted">
              <History className="size-3.5" aria-hidden="true" />
              {t('mcp:noRecentCalls')}
            </p>
          )}
        </section>
        <div className="flex flex-wrap items-center justify-end gap-2">
          <Button
            size="sm"
            variant="ghost"
            className="text-danger hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
            disabled={busy}
            onClick={() => void deleteServer(service)}
          >
            <Trash2 data-icon="inline-start" />
            {t('mcp:delete')}
          </Button>
          <Button
            size="sm"
            variant="secondary"
            className="max-[650px]:h-11"
            loading={busy}
            disabled={!service.enabled || busy}
            onClick={() => void testConnection(service)}
          >
            <RefreshCw data-icon="inline-start" />
            {t('mcp:testConnection')}
          </Button>
        </div>
      </div>
    )
  }

  return (
    <FeaturePage
      title={t('navigation:navigation.mcp')}
      description={t('mcp:lede')}
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
              placeholder={t('navigation:pageHeader.searchServicesOrTools')}
              shortcut={searchShortcut}
              clearLabel={t('navigation:more.clearSearch')}
            />
          }
          actions={
            // 主操作（原共享页头的“添加服务”）。
            <Button
              type="button"
              className="h-9"
              data-mcp-add
              disabled={busy}
              onClick={() => void addService()}
            >
              <Plus />
              {t('navigation:pageHeader.addService')}
            </Button>
          }
        />
      }
    >
      <McpHostPanel notify={notify} requestConfirm={requestConfirm} />
      {loading && !data && !error ? (
        <PageState kind="loading" title={t('mcp:loading')} />
      ) : !data && error ? (
        <PageState
          kind="error"
          title={t('mcp:loadFailed')}
          description={error.message}
          action={
            <Button
              variant="secondary"
              className="max-[650px]:h-11"
              disabled={busy}
              onClick={() => void reload()}
            >
              {t('mcp:retry')}
            </Button>
          }
        />
      ) : (
        data && (
          <>
            {error && (
              <Alert
                role="status"
                className="mt-4 flex min-w-0 items-center justify-between gap-3 text-muted-foreground [overflow-wrap:anywhere]"
              >
                <span>{error.message}</span>
                <Button
                  variant="ghost"
                  className="max-[650px]:h-11"
                  disabled={busy}
                  onClick={() => void reload()}
                >
                  {t('mcp:retry')}
                </Button>
              </Alert>
            )}
            <FeatureSection
              className="mcp-services"
              title={t('mcp:services')}
              count={visibleServices.length}
              meta={
                services.length > 0 &&
                t('mcp:serviceSummary', {
                  online: metrics.onlineServices,
                  total: metrics.totalServices,
                  rate: metrics.errorRate,
                })
              }
            >
              {visibleServices.length ? (
                <FeatureList>
                  {visibleServices.map((service) => {
                    const [label, attention] = mcpStatusMeta(service.status, t)
                    const location =
                      service.transport === 'stdio'
                        ? service.workingDirectory || service.command
                        : service.endpoint
                    return (
                      <FeatureRow
                        key={service.id}
                        className="service-row"
                        data-mcp-service={service.id}
                        icon={<Server />}
                        name={service.name}
                        description={
                          <span className="font-mono text-xs" title={location}>
                            {location}
                          </span>
                        }
                        status={
                          <span
                            data-mcp-status={service.status || 'offline'}
                            className={cn(
                              'flex items-center gap-1.5',
                              attention && 'font-medium text-st-attn',
                            )}
                          >
                            {attention && (
                              <span
                                aria-hidden="true"
                                className="size-2 flex-none rounded-full bg-st-attn-fill"
                              />
                            )}
                            {label}
                          </span>
                        }
                        actions={
                          <Switch
                            checked={service.enabled}
                            disabled={busy}
                            aria-label={t('mcp:toggleServiceName', { name: service.name })}
                            onCheckedChange={(enabled) => void toggleServer(service, enabled)}
                          />
                        }
                        expanded={expandedId === service.id}
                        onExpandedChange={(open) => setExpandedId(open ? service.id : '')}
                        toggleLabel={t('mcp:toggleServiceDetails', { name: service.name })}
                        detail={renderServiceDetail(service)}
                      />
                    )
                  })}
                </FeatureList>
              ) : (
                <InlineEmptyState
                  icon={Server}
                  title={t('mcp:noServerConfigured')}
                  description={services.length ? undefined : t('mcp:emptyAddHint')}
                />
              )}
            </FeatureSection>
            <FeatureSection
              className="mcp-tools-panel"
              title={t('mcp:toolCapabilities')}
              count={tools.length}
              meta={
                metrics.restrictedTools > 0 &&
                t('mcp:toolSummary', { count: metrics.restrictedTools })
              }
            >
              {tools.length ? (
                <FeatureList>
                  {tools.map((tool) => (
                    <FeatureRow
                      key={tool.piName}
                      className="tool-row"
                      data-mcp-tool={tool.piName}
                      icon={<Wrench />}
                      name={<span className="font-mono">{tool.name}</span>}
                      description={
                        <>
                          <span className="font-mono">{tool.serviceName}</span>
                          {tool.description && <> · {tool.description}</>}
                        </>
                      }
                      // 风险写成文字（规格 6.1：红绿不用于状态）。
                      status={mcpRiskLabel(tool.risk, t)}
                      actions={
                        <Switch
                          checked={Boolean(tool.enabled)}
                          disabled={busy || !tool.serviceEnabled}
                          aria-label={t('mcp:toggleToolName', { name: tool.name })}
                          onCheckedChange={(enabled) => void toggleTool(tool, enabled)}
                        />
                      }
                    />
                  ))}
                </FeatureList>
              ) : (
                <InlineEmptyState compact icon={Wrench} title={t('mcp:noToolsAvailable')} />
              )}
            </FeatureSection>
          </>
        )
      )}
    </FeaturePage>
  )
}
