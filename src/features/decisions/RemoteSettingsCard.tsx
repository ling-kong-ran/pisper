// 远端配置卡片：Provider / Base URL / 模型 ID / API Key 编辑，
// 保存走 PUT /api/decisions/config；连通性测试走 POST /api/decisions/test。
import { useEffect, useRef, useState } from 'react'
import {
  CheckCircle2,
  ChevronDown,
  CircleHelp,
  FlaskConical,
  KeyRound,
  RefreshCw,
  Save,
  XCircle,
} from 'lucide-react'
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
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import {
  REMOTE_PROVIDER_PRESETS,
  DECISION_PROVIDER_OPTIONS,
  isDecisionRemoteProvider,
  decisionErrorMessage,
  testDecisionsConnection,
  updateDecisionsConfig,
  type DecisionConfig,
  type DecisionRemoteConfig,
  type DecisionRemoteProvider,
  type DecisionTestResult,
} from './decisions-api'

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
  const testRequest = useRef<AbortController | null>(null)
  useEffect(() => () => testRequest.current?.abort(), [])

  // 后端配置变化（保存成功/切换 Provider 后后端回填默认值）时重新同步表单。
  useEffect(() => {
    setProvider(remote.provider)
    setBaseUrl(remote.baseUrl)
    setModelId(remote.modelId)
    setApiKey('')
  }, [remote.provider, remote.baseUrl, remote.modelId, remote.hasKey])

  useEffect(() => {
    testRequest.current?.abort()
    setTest({ status: 'idle' })
  }, [provider, baseUrl, modelId, apiKey, remote.hasKey])

  const dirty =
    provider !== remote.provider ||
    baseUrl.trim() !== remote.baseUrl ||
    modelId.trim() !== remote.modelId ||
    Boolean(apiKey.trim())
  const providerLabel = (value: DecisionRemoteProvider) => {
    if (value === 'custom') return t('decisions:remote.providerCustom')
    if (value === 'typesafe') return t('decisions:remote.providerTypesafe')
    if (value === 'openrouter') return t('decisions:remote.providerOpenrouter')
    return value
  }
  const busy = disabled || saving || test.status === 'running'
  const preset = REMOTE_PROVIDER_PRESETS[provider]
  const baseUrlEditable = !preset.baseUrl

  // 切换 Provider 时回填该 Provider 的默认地址与模型，避免遗留上一个 Provider 的值。
  const changeProvider = (next: DecisionRemoteProvider) => {
    setProvider(next)
    setBaseUrl(REMOTE_PROVIDER_PRESETS[next].baseUrl)
    setModelId(REMOTE_PROVIDER_PRESETS[next].modelId)
  }

  const save = async (patch?: { apiKey: string | null }) => {
    setSaving(true)
    setTest({ status: 'idle' })
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
    if (dirty || !remote.hasKey) return
    testRequest.current?.abort()
    const controller = new AbortController()
    testRequest.current = controller
    setTest({ status: 'running' })
    try {
      const result = await testDecisionsConnection(controller.signal)
      if (controller.signal.aborted) return
      setTest({ status: 'ok', result })
    } catch (caught) {
      if (controller.signal.aborted) return
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
            onChange={(event) => {
              if (isDecisionRemoteProvider(event.target.value)) changeProvider(event.target.value)
            }}
          >
            {DECISION_PROVIDER_OPTIONS.map((value) => (
              <option key={value} value={value}>
                {providerLabel(value)}
              </option>
            ))}
          </AppSelect>
        </FieldLabel>
        <FieldLabel variant="control" className="mt-0">
          {t('decisions:remote.modelId')}
          <Input
            value={modelId}
            disabled={busy}
            placeholder={t('decisions:remote.modelIdPlaceholder')}
            onChange={(event) => setModelId(event.target.value)}
          />
        </FieldLabel>
      </div>

      {(baseUrlEditable || baseUrl !== preset.baseUrl) && (
        <FieldLabel variant="control">
          {t('decisions:remote.baseUrl')}
          <Input
            value={baseUrl}
            disabled={busy || !baseUrlEditable}
            placeholder="https://…"
            onChange={(event) => setBaseUrl(event.target.value)}
          />
        </FieldLabel>
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
        <Button size="sm" disabled={busy || !dirty} onClick={() => void save()}>
          {saving ? <RefreshCw size={13} className="animate-spin" /> : <Save size={13} />}
          {t('decisions:remote.save')}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={busy || dirty || !remote.hasKey}
          onClick={() => void runTest()}
        >
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
            disabled={busy}
            onClick={() => void save({ apiKey: null })}
          >
            <XCircle size={13} />
            {t('decisions:remote.clearKey')}
          </Button>
        )}
      </div>

      {dirty && (
        <p className="mt-2 text-xs text-content-muted">{t('decisions:remote.saveBeforeTest')}</p>
      )}

      {test.status === 'ok' && (
        <div className="mt-3 flex items-center gap-2 text-xs text-content-muted" role="status">
          <CheckCircle2 size={14} className="shrink-0" />
          {t('decisions:remote.testSucceeded', { model: test.result.model ?? remote.modelId })}
        </div>
      )}
      {test.status === 'error' && (
        <Collapsible className="mt-3 rounded-md border border-border bg-muted/40 text-content-muted">
          <div className="flex items-center justify-between gap-3 p-3">
            <span className="flex items-center gap-2 text-xs" role="status">
              <CircleHelp size={14} className="shrink-0" />
              {t('decisions:remote.testFailed')}
            </span>
            <CollapsibleTrigger asChild>
              <Button variant="ghost" size="sm" className="group shrink-0">
                {t('decisions:remote.details')}
                <ChevronDown className="size-3 transition-transform group-data-[state=open]:rotate-180" />
              </Button>
            </CollapsibleTrigger>
          </div>
          <CollapsibleContent>
            <p className="max-h-40 overflow-auto border-t border-border p-3 text-xs whitespace-pre-wrap [overflow-wrap:anywhere]">
              {test.message}
            </p>
          </CollapsibleContent>
        </Collapsible>
      )}
    </Panel>
  )
}
