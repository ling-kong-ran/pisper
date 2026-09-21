// 远端配置卡片：Provider / Base URL / 模型 ID / API Key 编辑，
// 保存走 PUT /api/decisions/config；连通性测试走 POST /api/decisions/test。
import { useEffect, useState } from 'react'
import { CheckCircle2, FlaskConical, KeyRound, RefreshCw, Save, XCircle } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { AppSelect } from '@/components/AppSelect'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import {
  AppCard as Panel,
  AppCardHeader,
  AppSectionTitle as SectionTitle,
  StatusBadge as Badge,
} from '@/components/ui/app-primitives'
import { cn } from '@/lib/utils'
import {
  REMOTE_PROVIDER_PRESETS,
  decisionErrorMessage,
  testDecisionsConnection,
  updateDecisionsConfig,
  type DecisionConfig,
  type DecisionRemoteConfig,
  type DecisionRemoteProvider,
  type DecisionTestResult,
} from './decisions-api'

const PROVIDERS: DecisionRemoteProvider[] = ['typesafe', 'openrouter', 'custom']

type TestState =
  | { status: 'idle' | 'running' }
  | { status: 'ok'; result: DecisionTestResult }
  | { status: 'error'; message: string }

type RemoteSettingsCardProps = {
  remote: DecisionRemoteConfig
  disabled: boolean
  notify: Notify
  onSaved: (config: DecisionConfig) => void
}

export function RemoteSettingsCard({ remote, disabled, notify, onSaved }: RemoteSettingsCardProps) {
  const { t } = useI18n()
  const [provider, setProvider] = useState<DecisionRemoteProvider>(remote.provider)
  const [baseUrl, setBaseUrl] = useState(remote.baseUrl)
  const [modelId, setModelId] = useState(remote.modelId)
  const [apiKey, setApiKey] = useState('')
  const [saving, setSaving] = useState(false)
  const [test, setTest] = useState<TestState>({ status: 'idle' })

  // 后端配置变化（保存成功/切换 Provider 后后端回填默认值）时重新同步表单。
  useEffect(() => {
    setProvider(remote.provider)
    setBaseUrl(remote.baseUrl)
    setModelId(remote.modelId)
    setApiKey('')
  }, [remote])

  const busy = disabled || saving || test.status === 'running'
  const preset = REMOTE_PROVIDER_PRESETS[provider]
  const baseUrlEditable = provider === 'custom'

  // 切换 Provider 时回填该 Provider 的默认地址与模型，避免遗留上一个 Provider 的值。
  const changeProvider = (next: DecisionRemoteProvider) => {
    setProvider(next)
    setBaseUrl(REMOTE_PROVIDER_PRESETS[next].baseUrl)
    setModelId(REMOTE_PROVIDER_PRESETS[next].modelId)
  }

  const save = async (patch?: { apiKey: string | null }) => {
    setSaving(true)
    try {
      const result = await updateDecisionsConfig({
        remote: patch ?? {
          provider,
          baseUrl: baseUrl.trim(),
          modelId: modelId.trim(),
          apiKey: apiKey.trim(),
        },
      })
      onSaved(result.config)
      if (patch?.apiKey === null) {
        setApiKey('')
        notify(t('decisions:remote.keyCleared'), 'success')
      } else {
        notify(t('decisions:remote.saved'), 'success')
      }
    } catch (caught) {
      notify(decisionErrorMessage(caught, t('decisions:remote.saveFailed')), 'error')
    } finally {
      setSaving(false)
    }
  }

  const runTest = async () => {
    setTest({ status: 'running' })
    try {
      const result = await testDecisionsConnection()
      setTest({ status: 'ok', result })
    } catch (caught) {
      setTest({
        status: 'error',
        message: decisionErrorMessage(caught, t('decisions:remote.testFailed')),
      })
    }
  }

  return (
    <Panel>
      <AppCardHeader>
        <div className="flex min-w-0 flex-col gap-1">
          <SectionTitle title={t('decisions:remote.title')} />
          <p>{t('decisions:remote.subtitle')}</p>
        </div>
        <Badge tone={remote.hasKey ? 'green' : 'gray'}>
          <KeyRound size={11} className="mr-1 inline" />
          {remote.hasKey ? t('decisions:remote.keyConfigured') : t('decisions:remote.keyMissing')}
        </Badge>
      </AppCardHeader>

      <div className="mt-3 grid gap-3 sm:grid-cols-2">
        <FieldLabel variant="control" className="mt-0">
          {t('decisions:remote.provider')}
          <AppSelect
            value={provider}
            disabled={busy}
            aria-label={t('decisions:remote.provider')}
            onChange={(event) =>
              changeProvider(
                PROVIDERS.includes(event.target.value as DecisionRemoteProvider)
                  ? (event.target.value as DecisionRemoteProvider)
                  : 'custom',
              )
            }
          >
            <option value="typesafe">{t('decisions:remote.providerTypesafe')}</option>
            <option value="openrouter">{t('decisions:remote.providerOpenrouter')}</option>
            <option value="custom">{t('decisions:remote.providerCustom')}</option>
          </AppSelect>
        </FieldLabel>
        <FieldLabel variant="control" className="mt-0">
          {t('decisions:remote.modelId')}
          <Input
            value={modelId}
            disabled={busy}
            placeholder={preset.modelId}
            onChange={(event) => setModelId(event.target.value)}
          />
        </FieldLabel>
      </div>

      <FieldLabel variant="control">
        {t('decisions:remote.baseUrl')}
        <Input
          value={baseUrl}
          disabled={busy || !baseUrlEditable}
          placeholder={baseUrlEditable ? 'https://…' : preset.baseUrl}
          onChange={(event) => setBaseUrl(event.target.value)}
        />
      </FieldLabel>
      {!baseUrlEditable && (
        <p className="mt-1 text-[11px] text-content-muted">
          {t('decisions:remote.baseUrlLockedHint')}
        </p>
      )}

      <FieldLabel variant="control">
        {t('decisions:remote.apiKey')}
        <Input
          type="password"
          autoComplete="new-password"
          value={apiKey}
          disabled={busy}
          placeholder={
            remote.hasKey
              ? t('decisions:remote.apiKeyPlaceholderKeep')
              : t('decisions:remote.apiKeyPlaceholderEnter')
          }
          onChange={(event) => setApiKey(event.target.value)}
        />
      </FieldLabel>

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Button size="sm" disabled={busy} onClick={() => void save()}>
          {saving ? <RefreshCw size={13} className="animate-spin" /> : <Save size={13} />}
          {t('decisions:remote.save')}
        </Button>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void runTest()}>
          {test.status === 'running' ? (
            <RefreshCw size={13} className="animate-spin" />
          ) : (
            <FlaskConical size={13} />
          )}
          {test.status === 'running' ? t('decisions:remote.testing') : t('decisions:remote.test')}
        </Button>
        {remote.hasKey && (
          <Button
            size="sm"
            variant="ghost"
            className="text-danger"
            disabled={busy}
            onClick={() => void save({ apiKey: null })}
          >
            <XCircle size={13} />
            {t('decisions:remote.clearKey')}
          </Button>
        )}
      </div>

      {test.status !== 'idle' && test.status !== 'running' && (
        <div
          className={cn(
            'mt-3 flex items-start gap-2 rounded-[var(--r-xs)] p-2 text-[12px] leading-[1.5]',
            test.status === 'ok'
              ? 'bg-success-soft text-success-strong'
              : 'bg-danger-soft text-danger',
          )}
          role={test.status === 'error' ? 'alert' : 'status'}
        >
          {test.status === 'ok' ? (
            <CheckCircle2 size={14} className="mt-0.5 flex-none" />
          ) : (
            <XCircle size={14} className="mt-0.5 flex-none" />
          )}
          <span className="min-w-0 [overflow-wrap:anywhere]">
            {test.status === 'ok'
              ? t('decisions:remote.testSucceeded', { model: test.result.model })
              : test.status === 'error'
                ? `${t('decisions:remote.testFailed')}：${test.message}`
                : ''}
          </span>
        </div>
      )}
    </Panel>
  )
}
