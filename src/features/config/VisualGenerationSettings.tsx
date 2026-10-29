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
  Sparkles,
  Wand2,
} from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { pageStateStorage } from '@/lib/page-state-storage'
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
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
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
  return pageStateStorage.getItem(VISUAL_CONNECTIONS_STORAGE_KEY) === '1'
}

type VisualGenerationSettingsProps = {
  config: ConfigData
  notify: Notify
  toggling: string
  onToggleProvider: (provider: ProviderConfig, enabled: boolean) => void | Promise<void>
  onDeleteProvider: (provider: ProviderConfig) => void | Promise<void>
  onCloneProvider: (provider: ProviderConfig) => void
  onQuickSetup: () => void
  onEditVisualProvider: (providerId: string) => void
}

export function VisualGenerationSettings({
  config,
  notify,
  toggling,
  onToggleProvider,
  onDeleteProvider,
  onCloneProvider,
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
    pageStateStorage.setItem(VISUAL_CONNECTIONS_STORAGE_KEY, next ? '1' : '0')
  }

  const visualProviders = config.providers.filter((provider) => provider.type === 'visual')
  // 与对话连接列表同一套过滤规则：只展示已配置或自定义的连接。
  const visibleVisualProviders = visualProviders.filter(
    (provider) => provider.configured || provider.custom,
  )
  const imageModels = status?.imageModels || []
  const videoModels = status?.videoModels || []

  return (
    <section className="mt-3">
      {/* 当前视觉模型摘要卡：键帽图标 + 读数（提供方 / 模型），右侧测试与快速设置 */}
      <SettingsCard className="mb-3 p-4 py-3.5" data-config-card="models-visual">
        <div className="flex flex-wrap items-center gap-3">
          <span
            aria-hidden="true"
            className={cn(
              'grid size-9.5 flex-none place-items-center rounded-lg border border-key-border bg-key shadow-key',
              status?.image ? 'text-accent-fg' : 'text-content-muted',
            )}
          >
            <Sparkles size={18} />
          </span>
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="text-xs font-medium text-content-muted">
              {t('config:configPage.currentVisualModel')}
            </span>
            {loading ? (
              <span role="status" className="flex items-center gap-2 text-xs text-content-muted">
                <OrbitSpinner className="size-3.5" />
                {t('config:configPage.loadingVisualStatus')}
              </span>
            ) : status?.image ? (
              <>
                <span className="flex min-w-0 flex-wrap items-center gap-2">
                  <span className="truncate text-sm font-medium text-content">
                    {status.image.providerName} / {status.image.name}
                  </span>
                  <SettingsBadge tone="green">
                    <CheckCircle2 size={11} className="mr-[3px] inline" />
                    {t('config:configPage.authenticationReady')}
                  </SettingsBadge>
                </span>
                {status.video && (
                  <small className="text-xs text-content-muted [overflow-wrap:anywhere]">
                    {t('config:configPage.visualVideoModel')}：{status.video.providerName} /{' '}
                    {status.video.name}
                  </small>
                )}
              </>
            ) : (
              <span className="flex min-w-0 flex-col gap-0.5">
                <span className="flex items-center gap-1.5 text-sm font-medium text-warning-strong">
                  <CircleAlert size={14} aria-hidden="true" />
                  {t('config:configPage.visualNoneConfigured')}
                </span>
                <small className="text-xs text-content-muted">
                  {t('config:configPage.visualEmptyHint')}
                </small>
              </span>
            )}
          </div>
          <div className="flex w-full max-w-full min-w-0 flex-none flex-wrap items-center gap-2 sm:w-auto">
            {status?.image && (
              <Button
                variant="secondary"
                className="max-[650px]:h-11"
                disabled={testing}
                onClick={() => void runTest()}
              >
                {testing ? <OrbitSpinner className="text-current" /> : <FlaskConical size={13} />}
                {testing
                  ? t('config:configPage.testingGeneration')
                  : t('config:configPage.testGeneration')}
              </Button>
            )}
            <Button className="max-[650px]:h-11" onClick={onQuickSetup}>
              <Wand2 size={13} />
              {t('config:configPage.visualQuickSetup')}
            </Button>
          </div>
        </div>
        {testResult && (
          <div className="mt-3 flex items-center gap-2.5 rounded-sm border border-stroke-soft bg-surface-subtle px-2.5 py-2">
            {testResult.previewDataUrl ? (
              <img
                src={testResult.previewDataUrl}
                alt={testResult.modelName}
                className="size-11 flex-none rounded-xs object-cover"
              />
            ) : (
              <Check size={16} className="flex-none text-success" />
            )}
            <span className="flex min-w-0 flex-col gap-px text-xs">
              <span className="font-medium text-success-strong">
                {t('config:configPage.testSucceeded')}
              </span>
              <span className="truncate font-mono text-content-muted">
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
            className="group flex min-h-9 w-full cursor-pointer items-center gap-2 rounded-sm border-0 bg-transparent px-0.5 text-left max-[650px]:min-h-11"
          >
            <ChevronDown
              size={16}
              aria-hidden="true"
              className="flex-none text-content-muted transition-transform group-data-[state=closed]:-rotate-90"
            />
            <span className="shrink-0 text-sm font-medium text-content">
              {t('config:configPage.visualConnections')}
            </span>
            <span className="min-w-0 truncate text-xs text-content-muted">
              {t('config:configPage.visualConnectionsHint')}
            </span>
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <SettingsCard className="mt-2 p-4 py-4">
            {/* 与对话连接管理共用同一套卡片网格，保证两边样式一致 */}
            <ConnectionCardGrid
              providers={visualProviders}
              toggling={toggling}
              onClone={onCloneProvider}
              onConfigure={(provider) => onEditVisualProvider(provider.id)}
              onToggle={onToggleProvider}
              onDelete={onDeleteProvider}
            />
            {visibleVisualProviders.length === 0 && (
              <p className="text-xs text-content-muted">{t('config:configPage.visualEmptyHint')}</p>
            )}
            {(imageModels.length > 0 || videoModels.length > 0) && (
              <div className="mt-3 flex flex-col gap-2 border-t border-stroke-soft pt-3">
                {imageModels.length > 0 && (
                  <div className="flex items-center gap-2">
                    <span className="w-16 flex-none text-xs text-content-muted">
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
                  <div className="flex items-center gap-2">
                    <span className="w-16 flex-none text-xs text-content-muted">
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
                <small className="text-xs text-content-muted">
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
        <div className="mt-2.5">
          <AppError>
            <AlertTriangle size={13} />
            {error}
          </AppError>
        </div>
      )}
    </section>
  )
}
