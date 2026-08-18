// 配置加载 hook：拉取配置数据、管理草稿与保存，返回给配置页使用。
import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react'
import { apiJson } from '@/lib/api'
import {
  configSettingsReducer,
  createConfigDraft,
  draftForProvider,
  initialConfigSettingsState,
} from './config-state'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type {
  ConfigData,
  ConfigDraft,
  DiscoveredProvider,
  DiscoveryData,
  ProviderConfig,
  ProviderImportResult,
  ProviderType,
  Translate,
} from './config-types'

// 归一化异常为文案（配置页共用）。
function errorMessage(caught: unknown) {
  return caught instanceof Error ? caught.message : String(caught)
}

type UseConfigSettingsOptions = {
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  t: Translate
}

// 配置设置 hook：加载配置与模型刷新、编辑草稿、保存（含密钥）、
// Provider 增删/启停，提供配置页所需的完整状态与操作。
export function useConfigSettings({ notify, requestConfirm, t }: UseConfigSettingsOptions) {
  const [state, dispatch] = useReducer(configSettingsReducer, initialConfigSettingsState)
  const apiKeyInputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    let active = true
    apiJson<ConfigData>('/api/config')
      .then((data) => {
        if (!active) return undefined
        dispatch({ type: 'load-succeeded', config: data })
        return apiJson<{ config?: ConfigData }>('/api/providers/models/refresh', {
          method: 'POST',
          body: '{}',
        })
      })
      .then((result) => {
        if (!active || !result?.config) return
        dispatch({ type: 'refresh-succeeded', config: result.config })
      })
      .catch((caught: unknown) => {
        if (!active) return
        dispatch({ type: 'set-error', value: errorMessage(caught) })
        dispatch({ type: 'set-loading', value: false })
      })
    return () => {
      active = false
    }
  }, [])

  const selectedProvider = useMemo(
    () =>
      state.config && state.draft
        ? state.config.providers.find((item) => item.id === state.draft?.provider) ||
          state.config.providers[0]
        : null,
    [state.config, state.draft],
  )

  const patchDraft = useCallback((patch: Partial<ConfigDraft>) => {
    dispatch({ type: 'patch-draft', patch })
  }, [])

  const selectProvider = useCallback(
    (provider: ProviderConfig) => {
      if (!state.config) return
      dispatch({
        type: 'replace',
        config: state.config,
        draft: draftForProvider(state.config, provider, state.draft),
        dirty: false,
      })
    },
    [state.config, state.draft],
  )

  const selectModel = useCallback(
    (modelId: string) => {
      if (!selectedProvider) return
      const model = selectedProvider.models.find((item) => item.id === modelId)
      patchDraft({ model: modelId, modelBaseUrl: model?.baseUrlOverride || '' })
    },
    [patchDraft, selectedProvider],
  )

  const selectProviderType = useCallback(
    (providerType: ProviderType) => {
      if (!state.draft || !selectedProvider) return
      const firstChatModel = selectedProvider.models.find((model) => model.kind === 'chat')
      patchDraft({
        providerType,
        model: providerType === 'visual' ? '' : state.draft.model || firstChatModel?.id || '',
        modelBaseUrl:
          providerType === 'visual'
            ? ''
            : state.draft.modelBaseUrl || firstChatModel?.baseUrlOverride || '',
      })
    },
    [patchDraft, selectedProvider, state.draft],
  )

  const toggleProvider = useCallback(
    async (provider: ProviderConfig, enabled: boolean) => {
      dispatch({ type: 'set-toggling', value: provider.id })
      dispatch({ type: 'set-error', value: '' })
      try {
        const updated = await apiJson<ConfigData>(
          `/api/providers/${encodeURIComponent(provider.id)}/enabled`,
          { method: 'PUT', body: JSON.stringify({ enabled }) },
        )
        dispatch({ type: 'config-updated', config: updated })
        notify(
          t('config:configPage.nameState', {
            name: provider.name,
            state: enabled ? t('config:configPage.enabled') : t('config:configPage.disabled'),
          }),
        )
      } catch (caught) {
        dispatch({ type: 'set-error', value: errorMessage(caught) })
      } finally {
        dispatch({ type: 'set-toggling', value: '' })
      }
    },
    [notify, t],
  )

  const deleteProvider = useCallback(
    async (provider: ProviderConfig) => {
      const approved = await requestConfirm({
        title: t('config:configPage.deleteProviderConnection'),
        message: t(
          'config:configPage.deleteNameItsModelSettingsAndAuthenticationDetailsWillAlsoBeRemoved',
          { name: provider.name },
        ),
        confirmLabel: t('config:configPage.delete'),
      })
      if (!approved) return
      dispatch({ type: 'set-error', value: '' })
      try {
        const updated = await apiJson<ConfigData>(
          `/api/providers/${encodeURIComponent(provider.id)}`,
          { method: 'DELETE' },
        )
        const nextProvider =
          updated.providers.find((item) => item.id === updated.provider) || updated.providers[0]
        dispatch({
          type: 'replace',
          config: updated,
          draft: createConfigDraft(updated, nextProvider, updated.model),
          dirty: false,
        })
        notify(t('config:configPage.providerConnectionDeleted'))
      } catch (caught) {
        dispatch({ type: 'set-error', value: errorMessage(caught) })
      }
    },
    [notify, requestConfirm, t],
  )

  const save = useCallback(
    async (setAsDefault = false) => {
      const draft = state.draft
      if (!draft || !state.dirty) return
      const provider = state.config?.providers.find((item) => item.id === draft.provider)
      const apiKey = apiKeyInputRef.current?.value || draft.apiKey
      if (!draft.baseUrl.trim()) {
        dispatch({
          type: 'set-error',
          value: t('config:configPage.enterProviderBaseURLBeforeSaving'),
        })
        return
      }
      if (draft.providerType !== 'visual' && !draft.model.trim()) {
        dispatch({
          type: 'set-error',
          value: t('config:configPage.selectAChatModelBeforeSaving'),
        })
        return
      }
      if (!provider?.configured && !apiKey.trim()) {
        dispatch({
          type: 'set-error',
          value: t('config:configPage.completeProviderAuthenticationBeforeSaving'),
        })
        return
      }
      dispatch({ type: 'set-saving', value: true })
      dispatch({ type: 'set-error', value: '' })
      try {
        const saved = await apiJson<ConfigData>('/api/config', {
          method: 'PUT',
          body: JSON.stringify({ ...draft, apiKey, setAsDefault, enabled: true }),
        })
        if (apiKey.trim() && !saved.apiKeyUpdated) {
          throw new Error(t('config:configPage.apiKeyCouldNotBeUpdatedPleaseRetry'))
        }
        const savedProvider = saved.providers.find((item) => item.id === draft.provider)
        if (!savedProvider?.configured || !savedProvider.enabled) {
          throw new Error(t('config:configPage.providerWasNotReadyAfterSaving'))
        }
        dispatch({ type: 'save-succeeded', config: saved })
        notify(
          savedProvider.type === 'visual'
            ? t('config:configPage.visualModelSettingsSaved')
            : setAsDefault && draft.model
              ? t('config:configPage.agentSettingsSavedNewChatsWillUseThisModel')
              : t('config:configPage.providerSettingsSaved'),
        )
      } catch (caught) {
        dispatch({ type: 'set-error', value: errorMessage(caught) })
      } finally {
        dispatch({ type: 'set-saving', value: false })
      }
    },
    [notify, state.config, state.dirty, state.draft, t],
  )

  const applyImportedProvider = useCallback(
    (result: ProviderImportResult) => {
      const imported =
        result.config.providers.find((item) => item.id === result.providerId) ||
        result.config.providers[0]
      if (result.kind === 'authentication') {
        dispatch({ type: 'refresh-succeeded', config: result.config })
        notify(
          t('config:configPage.nameLoginStateHasBeenLoadedIntoPisper', {
            name: imported.name,
          }),
        )
        return
      }
      dispatch({
        type: 'replace',
        config: result.config,
        draft: draftForProvider(result.config, imported, state.draft, result.selectedModel),
        dirty: true,
      })
      notify(
        t('config:configPage.nameConfigurationHasBeenLoadedIntoPisper', {
          name: imported.name,
        }),
      )
    },
    [notify, state.draft, t],
  )

  const applyCreatedProvider = useCallback(
    (data: ConfigData) => {
      const provider = data.providers.find((item) => item.id === data.createdProviderId)
      dispatch({
        type: 'replace',
        config: data,
        draft: createConfigDraft(data, provider),
        dirty: true,
      })
      notify(t('config:configPage.providerConnectionCreated'))
    },
    [notify, t],
  )

  const applySynchronizedModels = useCallback((data: ConfigData) => {
    dispatch({ type: 'refresh-succeeded', config: data })
  }, [])

  const applyCreatedModels = useCallback(
    (data: ConfigData, providerId: string, modelId: string) => {
      const provider = data.providers.find((item) => item.id === providerId)
      const nextDraft = {
        ...createConfigDraft(data, provider, modelId),
        providerType: state.draft?.providerType || provider?.type || ('chat' as const),
        thinkingLevel: state.draft?.thinkingLevel || data.thinkingLevel,
        toolMode: state.draft?.toolMode || data.toolMode,
      }
      dispatch({ type: 'replace', config: data, draft: nextDraft, dirty: true })
      notify(
        t('config:configPage.countModelsAdded', {
          count: data.addedModelIds?.length || 1,
        }),
      )
    },
    [notify, state.draft, t],
  )

  return {
    ...state,
    selectedProvider,
    apiKeyInputRef,
    patchDraft,
    selectProvider,
    selectModel,
    selectProviderType,
    toggleProvider,
    deleteProvider,
    save,
    applyImportedProvider,
    applyCreatedProvider,
    applySynchronizedModels,
    applyCreatedModels,
  }
}

type UseProviderDiscoveryOptions = {
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  onImported: (result: ProviderImportResult) => void
  t: Translate
}

// Provider 发现 hook：扫描本地可导入 Provider、导入并回调结果，
// 处理扫描/导入中的错误与冲突确认。
export function useProviderDiscovery({
  requestConfirm,
  onImported,
  t,
}: UseProviderDiscoveryOptions) {
  const [discovery, setDiscovery] = useState<DiscoveryData>({ providers: [], errors: [] })
  const [discovering, setDiscovering] = useState(true)
  const [error, setError] = useState('')
  const [operationError, setOperationError] = useState('')
  const [importing, setImporting] = useState('')

  const refresh = useCallback(async () => {
    setDiscovering(true)
    setError('')
    try {
      setDiscovery(await apiJson<DiscoveryData>('/api/providers/discovery'))
    } catch (caught) {
      setError(errorMessage(caught))
    } finally {
      setDiscovering(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const importProvider = useCallback(
    async (provider: DiscoveredProvider) => {
      const source =
        provider.source === 'codex-config'
          ? 'Codex config.toml'
          : provider.source === 'claude-config'
            ? 'Claude settings.json'
            : provider.source === 'codex-auth'
              ? 'Codex login'
              : 'Claude login'
      const authentication = provider.kind === 'authentication'
      const approved = await requestConfirm({
        title: authentication
          ? t('config:configPage.loadLoginState')
          : t('config:configPage.loadProviderConfiguration'),
        message: authentication
          ? t('config:configPage.loadOfficialProviderLoginStateFromSource', { source })
          : t('config:configPage.loadThisProviderConfigurationFromSource', { source }),
        confirmLabel: authentication
          ? t('config:configPage.loadLoginState')
          : t('config:configPage.loadConfiguration'),
      })
      if (!approved) return
      setImporting(provider.id)
      setOperationError('')
      try {
        const result = await apiJson<ProviderImportResult>(
          `/api/providers/${encodeURIComponent(provider.id)}/import`,
          { method: 'POST', body: '{}' },
        )
        setDiscovery(result.discovery)
        onImported(result)
      } catch (caught) {
        setOperationError(errorMessage(caught))
      } finally {
        setImporting('')
      }
    },
    [onImported, requestConfirm, t],
  )

  return {
    discovery,
    discovering,
    error,
    operationError,
    importing,
    refresh,
    importProvider,
  }
}
