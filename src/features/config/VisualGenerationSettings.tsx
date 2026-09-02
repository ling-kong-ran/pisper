// 视觉生成设置：与「当前对话模型」摘要卡同构的当前视觉模型卡片，
// 下方「视觉连接」折叠区与连接管理一致——启停/编辑/删除视觉供应商，
// 并选择图像/视频的默认模型（与会话内 generate_visual 的自动选中规则一致）。
import { useCallback, useEffect, useState } from 'react'
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  ChevronDown,
  CircleAlert,
  FlaskConical,
  RefreshCw,
  Server,
  Sparkles,
  Trash2,
  Wand2,
} from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { PROVIDER_ICONS } from './provider-constants'
import { SettingsBadge, SettingsCard, SettingsSwitch } from './settings-primitives'
import type { Notify } from '@/app/route-context'
import type {
  ConfigData,
  ProviderConfig,
  VisualModelStatus,
  VisualTestResult,
} from './config-types'

import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { AppError } from '@/components/ui/app-primitives'

// 视觉连接区展开状态持久化：与「连接管理」一致，默认折叠。
const VISUAL_CONNECTIONS_STORAGE_KEY = 'pisper.config.visualConnectionsOpen'

function storedVisualConnectionsOpen(): boolean {
  return window.localStorage.getItem(VISUAL_CONNECTIONS_STORAGE_KEY) === '1'
}

type VisualGenerationSettingsProps = {
  config: ConfigData
  notify: Notify
  toggling: string
  onToggleProvider: (provider: ProviderConfig, enabled: boolean) => void | Promise<void>
  onDeleteProvider: (provider: ProviderConfig) => void | Promise<void>
  onQuickSetup: () => void
  onEditVisualProvider: (providerId: string) => void
}

export function VisualGenerationSettings({
  config,
  notify,
  toggling,
  onToggleProvider,
  onDeleteProvider,
  onQuickSetup,
  onEditVisualProvider,
}: VisualGenerationSettingsProps) {
  const { t } = useI18n()
  const [status, setStatus] = useState<VisualModelStatus | null>(null)
  const [loading, setLoading] = useState(true)
  const [selecting, setSelecting] = useState('')
  const [testing, setTesting] = useState(false)
  const [testResult, setTestResult] = useState<VisualTestResult | null>(null)
  const [error, setError] = useState('')
  const [connectionsOpen, setConnectionsOpen] = useState(storedVisualConnectionsOpen)

  const refresh = useCallback(async () => {
    try {
      setStatus(await apiJson<VisualModelStatus>('/api/visual/models'))
      setError('')
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh, config])

  const selectVisualModel = async (kind: 'image' | 'video', model: string) => {
    setSelecting(kind)
    setError('')
    try {
      setStatus(
        await apiJson<VisualModelStatus>(`/api/visual/models/${kind}`, {
          method: 'PUT',
          body: JSON.stringify({ model }),
        }),
      )
      notify(t('config:configPage.visualModelUpdated'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSelecting('')
    }
  }

  // 冒烟测试：实际生成一张小图，验证当前自动选中的视觉模型端到端可用。
  const runTest = async () => {
    setTesting(true)
    setError('')
    setTestResult(null)
    try {
      setTestResult(
        await apiJson<VisualTestResult>('/api/visual/test', { method: 'POST', body: '{}' }),
      )
      notify(t('config:configPage.testSucceeded'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setTesting(false)
    }
  }

  const setConnectionsOpenPersisted = (next: boolean) => {
    setConnectionsOpen(next)
    window.localStorage.setItem(VISUAL_CONNECTIONS_STORAGE_KEY, next ? '1' : '0')
  }

  const visualProviders = config.providers.filter((provider) => provider.type === 'visual')
  const imageModels = status?.imageModels || []
  const videoModels = status?.videoModels || []

  return (
    <section className="[margin-top:12px]">
      {/* 当前视觉模型摘要卡：与「当前对话模型」卡片同构 */}
      <SettingsCard className="[margin-bottom:12px]">
        <div className="flex flex-wrap items-center gap-[12px]">
          <span className="grid w-[40px] h-[40px] flex-none place-items-center rounded-[11px] bg-[var(--accent-soft)] text-[var(--star-strong)]">
            <Sparkles size={19} />
          </span>
          <div className="flex min-w-0 flex-1 flex-col gap-[3px]">
            <span className="text-[12px] font-[600] text-[var(--text-muted)]">
              {t('config:configPage.currentVisualModel')}
            </span>
            {loading ? (
              <span className="flex items-center gap-[7px] text-[12px] text-[var(--text-muted)]">
                <RefreshCw className="animate-spin" size={13} />
                {t('config:configPage.loadingVisualStatus')}
              </span>
            ) : status?.image ? (
              <>
                <span className="flex min-w-0 flex-wrap items-center gap-[7px]">
                  <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-[15px]">
                    {status.image.providerName} / {status.image.name}
                  </strong>
                  <SettingsBadge tone="green">
                    <CheckCircle2 size={11} className="mr-[3px] inline" />
                    {t('config:configPage.authenticationReady')}
                  </SettingsBadge>
                </span>
                {status.video && (
                  <small className="text-[12px] text-[var(--text-muted)]">
                    {t('config:configPage.visualVideoModel')}：{status.video.providerName} /{' '}
                    {status.video.name}
                  </small>
                )}
              </>
            ) : (
              <span className="flex min-w-0 flex-col gap-[2px]">
                <strong className="flex items-center gap-[6px] text-[14px] text-[var(--warning-strong)]">
                  <CircleAlert size={15} />
                  {t('config:configPage.visualNoneConfigured')}
                </strong>
                <small className="text-[12px] text-[var(--text-muted)]">
                  {t('config:configPage.visualEmptyHint')}
                </small>
              </span>
            )}
          </div>
          <div className="flex flex-none items-center gap-[7px]">
            {status?.image && (
              <Button
                variant="outline"
                className="bg-surface-subtle"
                disabled={testing}
                onClick={() => void runTest()}
              >
                {testing ? (
                  <RefreshCw className="animate-spin" size={13} />
                ) : (
                  <FlaskConical size={13} />
                )}
                {testing
                  ? t('config:configPage.testingGeneration')
                  : t('config:configPage.testGeneration')}
              </Button>
            )}
            <Button onClick={onQuickSetup}>
              <Wand2 size={13} />
              {t('config:configPage.visualQuickSetup')}
            </Button>
          </div>
        </div>
        {testResult && (
          <div className="flex items-center gap-[10px] [margin-top:10px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[8px_10px]">
            {testResult.previewDataUrl ? (
              <img
                src={testResult.previewDataUrl}
                alt={testResult.modelName}
                className="h-[44px] w-[44px] flex-none rounded-[var(--r-xs)] object-cover"
              />
            ) : (
              <Check size={16} className="flex-none text-[var(--success)]" />
            )}
            <span className="flex min-w-0 flex-col gap-[1px] text-[12px]">
              <strong className="text-[var(--success-strong)]">
                {t('config:configPage.testSucceeded')}
              </strong>
              <span className="overflow-hidden text-ellipsis whitespace-nowrap text-[var(--text-muted)]">
                {testResult.providerName} / {testResult.modelName} · {testResult.path}
              </span>
            </span>
          </div>
        )}
      </SettingsCard>

      {/* 视觉连接折叠区：与「连接管理」同构，行内开关启停视觉供应商 */}
      <Collapsible open={connectionsOpen} onOpenChange={setConnectionsOpenPersisted}>
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className="group flex w-full cursor-pointer items-center gap-[7px] border-0 bg-transparent p-[4px_2px] text-left"
          >
            <ChevronDown
              size={15}
              className="flex-none text-[var(--text-muted)] transition-transform group-data-[state=closed]:-rotate-90"
            />
            <span className="text-[13px] font-[700] text-[var(--text-secondary)]">
              {t('config:configPage.visualConnections')}
            </span>
            <span className="truncate text-[12px] text-[var(--text-tertiary)]">
              {t('config:configPage.visualConnectionsHint')}
            </span>
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <SettingsCard>
            <div className="flex flex-col gap-[8px]">
              {visualProviders.map((provider) => {
                const Icon = PROVIDER_ICONS[provider.id] || Server
                const statusText = !provider.configured
                  ? t('config:configPage.apiKeyRequired')
                  : provider.enabled
                    ? t('config:configPage.authenticationReady')
                    : t('config:configPage.disabled2')
                return (
                  <div
                    key={provider.id}
                    role="button"
                    tabIndex={0}
                    title={t('config:configPage.configure')}
                    className="flex cursor-pointer items-center gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[9px_11px] hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)] focus-visible:outline-2 focus-visible:outline-[var(--focus)]"
                    onClick={() => onEditVisualProvider(provider.id)}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter' || event.key === ' ') {
                        event.preventDefault()
                        onEditVisualProvider(provider.id)
                      }
                    }}
                  >
                    <span className="grid w-[30px] h-[30px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)]">
                      <Icon size={16} />
                    </span>
                    <strong className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
                      {provider.name}
                    </strong>
                    <small className="flex-none text-[12px] text-[var(--text-muted)]">
                      {statusText}
                    </small>
                    {/* 开关/删除是行内独立控件，不触发行点击编辑 */}
                    <div
                      className="flex flex-none items-center gap-[6px]"
                      onClick={(event) => event.stopPropagation()}
                      onKeyDown={(event) => event.stopPropagation()}
                    >
                      <SettingsSwitch
                        value={provider.configured && provider.enabled}
                        disabled={!provider.configured || toggling === provider.id}
                        onChange={(enabled) => onToggleProvider(provider, enabled)}
                      />
                      {provider.custom && (
                        <Button
                          variant="destructive"
                          size="icon"
                          className="h-[26px] w-[26px]"
                          title={t('config:configPage.deleteProvider')}
                          onClick={() => onDeleteProvider(provider)}
                        >
                          <Trash2 size={13} />
                        </Button>
                      )}
                    </div>
                  </div>
                )
              })}
              {visualProviders.length === 0 && (
                <p className="[margin:2px_0_0] text-[12px] text-[var(--text-muted)]">
                  {t('config:configPage.visualEmptyHint')}
                </p>
              )}
            </div>
            {(imageModels.length > 0 || videoModels.length > 0) && (
              <div className="flex flex-col gap-[8px] [border-top:1px_solid_var(--stroke-soft)] [margin-top:10px] pt-[10px]">
                {imageModels.length > 0 && (
                  <div className="flex items-center gap-[8px]">
                    <span className="w-[64px] flex-none text-[12px] text-[var(--text-muted)]">
                      {t('config:configPage.visualImageModel')}
                    </span>
                    <AppSelect
                      className="min-w-0 flex-1"
                      value={status?.imageSelection || ''}
                      disabled={selecting === 'image'}
                      aria-label={t('config:configPage.visualImageModel')}
                      onChange={(event) => void selectVisualModel('image', event.target.value)}
                    >
                      <option value="">{t('config:configPage.visualAutoSelected')}</option>
                      {imageModels.map((candidate) => (
                        <option
                          key={`${candidate.providerId}/${candidate.id}`}
                          value={`${candidate.providerId}/${candidate.id}`}
                        >
                          {candidate.providerName} · {candidate.name}
                        </option>
                      ))}
                    </AppSelect>
                  </div>
                )}
                {videoModels.length > 0 && (
                  <div className="flex items-center gap-[8px]">
                    <span className="w-[64px] flex-none text-[12px] text-[var(--text-muted)]">
                      {t('config:configPage.visualVideoModel')}
                    </span>
                    <AppSelect
                      className="min-w-0 flex-1"
                      value={status?.videoSelection || ''}
                      disabled={selecting === 'video'}
                      aria-label={t('config:configPage.visualVideoModel')}
                      onChange={(event) => void selectVisualModel('video', event.target.value)}
                    >
                      <option value="">{t('config:configPage.visualAutoSelected')}</option>
                      {videoModels.map((candidate) => (
                        <option
                          key={`${candidate.providerId}/${candidate.id}`}
                          value={`${candidate.providerId}/${candidate.id}`}
                        >
                          {candidate.providerName} · {candidate.name}
                        </option>
                      ))}
                    </AppSelect>
                  </div>
                )}
                <small className="text-[11px] text-[var(--text-tertiary)]">
                  {t(
                    'config:configPage.visualModelsAreSelectedByTheVisualGenerationToolAndDoNotAppearInTheChatModelList',
                  )}
                </small>
              </div>
            )}
          </SettingsCard>
        </CollapsibleContent>
      </Collapsible>

      {error && (
        <div className="[margin-top:10px]">
          <AppError>
            <AlertTriangle size={13} />
            {error}
          </AppError>
        </div>
      )}
    </section>
  )
}
