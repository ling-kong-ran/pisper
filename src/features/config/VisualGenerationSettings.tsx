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
  Sparkles,
  Wand2,
} from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { ConnectionCardGrid } from './ConnectionList'
import { SettingsBadge, SettingsCard } from './settings-primitives'
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

// 冒烟测试状态提升为模块级缓存：真实生图可能耗时数十秒，期间切走页面组件会卸载，
// 局部状态丢失导致“切回来什么都没了”。缓存在途 Promise 与最近一次结果，
// 重新回到本页时在途的继续转圈、已完成的直接展示。
const visualTestCache = {
  running: null as Promise<VisualTestResult> | null,
  result: null as VisualTestResult | null,
  error: '',
}

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

  // 挂载时恢复缓存的测试状态：在途请求重新挂回调继续转圈，已完成的直接展示。
  useEffect(() => {
    if (visualTestCache.result) setTestResult(visualTestCache.result)
    if (visualTestCache.error) setError(visualTestCache.error)
    const running = visualTestCache.running
    if (!running) return undefined
    setTesting(true)
    let active = true
    running
      .then((result) => {
        if (active) setTestResult(result)
      })
      .catch(() => {
        if (active) setError(visualTestCache.error)
      })
      .finally(() => {
        if (active) setTesting(false)
      })
    return () => {
      active = false
    }
  }, [])

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
  // 状态写入模块级缓存，页面导航后不丢失；组件卸载后的 setState 为静默无操作。
  const runTest = async () => {
    if (visualTestCache.running) return
    setTesting(true)
    setError('')
    setTestResult(null)
    visualTestCache.result = null
    visualTestCache.error = ''
    const pending = apiJson<VisualTestResult>('/api/visual/test', {
      method: 'POST',
      body: '{}',
      // 真实生图可能远超默认 30s：服务端图像驱动超时为 3 分钟，这里留足余量。
      timeout: 210_000,
    })
    visualTestCache.running = pending
    try {
      const result = await pending
      visualTestCache.result = result
      setTestResult(result)
      notify(t('config:configPage.testSucceeded'))
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      visualTestCache.error = message
      setError(message)
      notify(message)
    } finally {
      visualTestCache.running = null
      setTesting(false)
    }
  }

  const setConnectionsOpenPersisted = (next: boolean) => {
    setConnectionsOpen(next)
    window.localStorage.setItem(VISUAL_CONNECTIONS_STORAGE_KEY, next ? '1' : '0')
  }

  const visualProviders = config.providers.filter((provider) => provider.type === 'visual')
  // 与对话连接列表同一套过滤规则：只展示已配置或自定义的连接。
  const visibleVisualProviders = visualProviders.filter(
    (provider) => provider.configured || provider.custom,
  )
  const imageModels = status?.imageModels || []
  const videoModels = status?.videoModels || []

  return (
    <section className="[margin-top:12px]">
      {/* 当前视觉模型摘要卡：与「当前对话模型」卡片同构 */}
      <SettingsCard className="[margin-bottom:12px]" data-config-card="models-visual">
        <div className="flex flex-wrap items-center gap-[12px] max-[650px]:grid max-[650px]:grid-cols-[40px_minmax(0,1fr)]">
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
                  <small className="text-[12px] text-[var(--text-muted)] [overflow-wrap:anywhere]">
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
          <div className="flex min-w-0 max-w-full flex-none flex-wrap items-center gap-[7px] max-[650px]:col-span-full">
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
            <span className="shrink-0 text-[13px] font-[700] text-[var(--text-secondary)]">
              {t('config:configPage.visualConnections')}
            </span>
            <span className="min-w-0 text-[12px] text-[var(--text-tertiary)]">
              {t('config:configPage.visualConnectionsHint')}
            </span>
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <SettingsCard>
            {/* 与对话连接管理共用同一套卡片网格，保证两边样式一致 */}
            <ConnectionCardGrid
              providers={visualProviders}
              toggling={toggling}
              onConfigure={(provider) => onEditVisualProvider(provider.id)}
              onToggle={onToggleProvider}
              onDelete={onDeleteProvider}
            />
            {visibleVisualProviders.length === 0 && (
              <p className="[margin:2px_0_0] text-[12px] text-[var(--text-muted)]">
                {t('config:configPage.visualEmptyHint')}
              </p>
            )}
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
