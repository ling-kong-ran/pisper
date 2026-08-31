// 视觉生成设置：平铺展示当前自动选中的图像/视频模型（与 generate_visual 工具一致），
// 一行使用引导（可复制的示例提示词）+ 冒烟测试结果内联预览；
// 未配置时给出可一键添加的推荐视觉模型（复用已配置连接的密钥）。
import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import {
  AlertTriangle,
  Check,
  Image as ImageIcon,
  Play,
  Plus,
  RefreshCw,
  Sparkles,
  Video,
  Wand2,
} from 'lucide-react'
import { PAGE_PATHS } from '@/app/routes'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { requestSessionCreation } from '@/features/chat/events'
import { SettingsCard, SettingsSectionTitle } from './settings-primitives'
import type { Notify } from '@/app/route-context'
import type {
  ConfigData,
  ProviderConfig,
  VisualCandidate,
  VisualModelStatus,
  VisualTestResult,
} from './config-types'

import { Button } from '@/components/ui/button'

import { AppError } from '@/components/ui/app-primitives'

// 各 Provider 的推荐视觉模型：一键添加的候选，添加后由运行时自动选择。
const VISUAL_SUGGESTIONS: Record<string, Array<{ id: string; name: string; kind: string }>> = {
  openai: [
    { id: 'gpt-image-2', name: 'GPT Image 2', kind: 'image' },
    { id: 'sora-2', name: 'Sora 2', kind: 'video' },
  ],
  google: [
    { id: 'gemini-3-pro-image', name: 'Gemini 3 Pro Image', kind: 'image' },
    { id: 'veo-3.1', name: 'Veo 3.1', kind: 'video' },
  ],
  xai: [{ id: 'grok-imagine-image', name: 'Grok Imagine', kind: 'image' }],
}

// 单个模型槽位（图像/视频）：图标 + 类型 + 自动选中的模型名。
function VisualModelTile({
  icon: Icon,
  label,
  model,
  models,
  selection,
  saving,
  onChange,
}: {
  icon: typeof ImageIcon
  label: string
  model: VisualCandidate | null | undefined
  models: VisualCandidate[]
  selection: string
  saving: boolean
  onChange: (value: string) => void
}) {
  const { t } = useI18n()
  return (
    <div className="flex min-w-0 items-center gap-[9px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[9px_11px]">
      <span className="grid w-[30px] h-[30px] flex-none place-items-center rounded-[var(--r-xs)] bg-[var(--accent-soft)] text-[var(--star-strong)]">
        <Icon size={15} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-[4px]">
        <small className="text-[11px] text-[var(--text-muted)]">{label}</small>
        {model ? (
          <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
            {model.name} · {model.providerName}
          </strong>
        ) : (
          <span className="text-[12px] text-[var(--text-tertiary)]">
            {t('config:configPage.visualNotConfigured')}
          </span>
        )}
        {models.length > 0 && (
          <AppSelect
            value={selection}
            disabled={saving}
            aria-label={label}
            onChange={(event) => onChange(event.target.value)}
          >
            <option value="">{t('config:configPage.visualAutoSelected')}</option>
            {models.map((candidate) => (
              <option
                key={`${candidate.providerId}/${candidate.id}`}
                value={`${candidate.providerId}/${candidate.id}`}
              >
                {candidate.providerName} · {candidate.name}
              </option>
            ))}
          </AppSelect>
        )}
      </span>
    </div>
  )
}

type VisualGenerationSettingsProps = {
  config: ConfigData
  notify: Notify
  onConfigChanged: (data: ConfigData) => void
  onAddVisualProvider: () => void
}

export function VisualGenerationSettings({
  config,
  notify,
  onConfigChanged,
  onAddVisualProvider,
}: VisualGenerationSettingsProps) {
  const { t } = useI18n()
  const [status, setStatus] = useState<VisualModelStatus | null>(null)
  const [loading, setLoading] = useState(true)
  const [adding, setAdding] = useState('')
  const [selecting, setSelecting] = useState('')
  const [testing, setTesting] = useState(false)
  const [trying, setTrying] = useState(false)
  const [testResult, setTestResult] = useState<VisualTestResult | null>(null)
  const [error, setError] = useState('')
  const navigate = useNavigate()

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

  // 一键添加推荐视觉模型到已配置连接：写入模型目录后刷新状态与配置视图。
  const quickAdd = async (
    provider: ProviderConfig,
    model: { id: string; name: string; kind: string },
  ) => {
    const key = `${provider.id}/${model.id}`
    setAdding(key)
    setError('')
    try {
      const data = await apiJson<ConfigData>(
        `/api/providers/${encodeURIComponent(provider.id)}/models`,
        {
          method: 'POST',
          body: JSON.stringify({ id: model.id, name: model.name, kind: model.kind }),
        },
      )
      onConfigChanged(data)
      notify(t('config:configPage.visualModelAdded'))
      await refresh()
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setAdding('')
    }
  }

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

  // 试试示例：点击时实时探测是否有可用的视觉模型（自动选中规则与会话内
  // generate_visual 一致）；可用则新建会话并自动发送示例提示词，
  // 不可用则直接告知缺配置/供应商不可用。
  const tryExample = async () => {
    setTrying(true)
    setError('')
    try {
      const latest = await apiJson<VisualModelStatus>('/api/visual/models')
      setStatus(latest)
      if (!latest.image) {
        notify(t('config:configPage.visualUnavailableNotify'), 'error')
        return
      }
      if (requestSessionCreation('', examplePrompt)) navigate(PAGE_PATHS.chat)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setTrying(false)
    }
  }

  const configured = Boolean(status?.image || status?.video)
  const examplePrompt = t('config:configPage.visualExamplePrompt')
  // 只推荐「已配置且还没覆盖该类型」的连接，避免重复添加。
  const suggestions = config.providers
    .filter((provider) => provider.configured && VISUAL_SUGGESTIONS[provider.id])
    .flatMap((provider) =>
      (VISUAL_SUGGESTIONS[provider.id] || [])
        .filter((model) => (model.kind === 'image' ? !status?.image : !status?.video))
        .map((model) => ({ provider, model })),
    )

  // 「试试示例」行：点击新建会话并自动发送示例提示词（不可用时会明确提示）。
  const tryExampleRow = (
    <div className="flex flex-wrap items-center gap-[7px] [margin-top:10px] text-[12px] text-[var(--text-muted)]">
      <span>{t('config:configPage.tryExample')}</span>
      <button
        type="button"
        className="inline-flex max-w-full cursor-pointer items-center gap-[5px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-pill)] bg-[var(--surface-subtle)] px-[9px] py-[3px] text-[12px] text-[var(--text)] hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)]"
        title={t('config:configPage.tryExampleTooltip')}
        disabled={trying}
        onClick={() => void tryExample()}
      >
        <span className="overflow-hidden text-ellipsis whitespace-nowrap">{examplePrompt}</span>
        {trying ? (
          <RefreshCw size={12} className="flex-none animate-spin text-[var(--text-muted)]" />
        ) : (
          <Play size={12} className="flex-none text-[var(--star-strong)]" />
        )}
      </button>
      {configured && (
        <span className="text-[11px] text-[var(--text-tertiary)]">
          {t('config:configPage.visualOutputHint')}
        </span>
      )}
    </div>
  )

  return (
    <SettingsCard className="[margin-top:12px]">
      <div className="flex items-center justify-between gap-[10px]">
        <div className="flex min-w-0 items-center gap-[10px]">
          <span className="grid w-[34px] h-[34px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--star-soft)] text-[var(--star-strong)]">
            <Sparkles size={17} />
          </span>
          <div className="flex min-w-0 flex-col gap-[1px]">
            <SettingsSectionTitle title={t('config:configPage.visualGeneration')} />
            <span className="text-[12px] text-[var(--text-muted)]">
              {t('config:configPage.visualGenerationSubtitle')}
            </span>
          </div>
        </div>
        {status?.image && (
          <Button
            variant="outline"
            size="sm"
            className="flex-none bg-surface-subtle"
            disabled={testing}
            onClick={() => void runTest()}
          >
            {testing ? <RefreshCw className="animate-spin" size={13} /> : <Wand2 size={13} />}
            {testing
              ? t('config:configPage.testingGeneration')
              : t('config:configPage.testGeneration')}
          </Button>
        )}
      </div>

      {loading ? (
        <div className="flex items-center gap-[8px] [margin-top:12px] text-[12px] text-[var(--text-muted)]">
          <RefreshCw className="animate-spin" size={13} />
          {t('config:configPage.loadingVisualStatus')}
        </div>
      ) : configured ? (
        <>
          <div className="grid grid-cols-2 gap-[8px] [margin-top:12px] max-[520px]:grid-cols-1">
            <VisualModelTile
              icon={ImageIcon}
              label={t('config:configPage.visualImageModel')}
              model={status?.image}
              models={status?.imageModels || []}
              selection={status?.imageSelection || ''}
              saving={selecting === 'image'}
              onChange={(value) => void selectVisualModel('image', value)}
            />
            <VisualModelTile
              icon={Video}
              label={t('config:configPage.visualVideoModel')}
              model={status?.video}
              models={status?.videoModels || []}
              selection={status?.videoSelection || ''}
              saving={selecting === 'video'}
              onChange={(value) => void selectVisualModel('video', value)}
            />
          </div>
          {tryExampleRow}
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
        </>
      ) : (
        <>
          <p className="[margin:10px_0_0] text-[12px] leading-[1.6] text-[var(--text-muted)]">
            {t('config:configPage.visualEmptyHint')}
          </p>
          {tryExampleRow}
        </>
      )}

      {!loading && (!configured || suggestions.length > 0) && (
        <div className="flex flex-wrap items-center gap-[7px] [margin-top:10px]">
          {!configured && suggestions.length > 0 && (
            <span className="text-[12px] font-[600] text-[var(--text-muted)]">
              {t('config:configPage.quickAddVisualModel')}
            </span>
          )}
          {suggestions.map(({ provider, model }) => {
            const key = `${provider.id}/${model.id}`
            return (
              <Button
                key={key}
                variant="outline"
                size="sm"
                className="h-[27px] bg-surface-subtle px-[9px] text-[12px]"
                disabled={Boolean(adding)}
                onClick={() => void quickAdd(provider, model)}
              >
                {adding === key ? (
                  <RefreshCw className="animate-spin" size={12} />
                ) : (
                  <Plus size={12} />
                )}
                {provider.name} · {model.name}
              </Button>
            )
          })}
          {!configured && (
            <Button
              variant="ghost"
              size="sm"
              className="text-[var(--text-muted)]"
              onClick={onAddVisualProvider}
            >
              <Plus size={13} />
              {t('config:configPage.newVisualConnection')}
            </Button>
          )}
        </div>
      )}

      {error && (
        <AppError>
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
    </SettingsCard>
  )
}
