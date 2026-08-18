// 配置状态 reducer：管理配置草稿的加载/编辑/保存生命周期。
import type { ConfigData, ConfigDraft, ProviderConfig } from './config-types'

export type ConfigSettingsState = {
  config: ConfigData | null
  draft: ConfigDraft | null
  loading: boolean
  saving: boolean
  toggling: string
  error: string
  dirty: boolean
}

export const initialConfigSettingsState: ConfigSettingsState = {
  config: null,
  draft: null,
  loading: true,
  saving: false,
  toggling: '',
  error: '',
  dirty: false,
}

export function createConfigDraft(
  data: ConfigData,
  provider?: ProviderConfig,
  preferredModel?: string,
): ConfigDraft {
  const chatModels = provider?.models.filter((item) => item.kind === 'chat') || []
  const model =
    chatModels.find((item) => item.id === preferredModel) ||
    chatModels.find((item) => item.id === provider?.defaultModel) ||
    chatModels[0]
  return {
    provider: provider?.id || 'openai',
    providerType: provider?.type || 'chat',
    api: provider?.api || 'openai-responses',
    model: model?.id || '',
    apiKey: '',
    baseUrl: provider?.baseUrl || '',
    modelBaseUrl: model?.baseUrlOverride || '',
    organization: provider?.organization || '',
    thinkingLevel: data.thinkingLevel || 'medium',
    toolMode: data.toolMode || 'full',
  }
}

export function refreshConfigDraft(data: ConfigData, current: ConfigDraft | null): ConfigDraft {
  if (!current) {
    const provider = data.providers.find((item) => item.id === data.provider) || data.providers[0]
    return createConfigDraft(data, provider, data.model)
  }
  const provider =
    data.providers.find((item) => item.id === current.provider) ||
    data.providers.find((item) => item.id === data.provider) ||
    data.providers[0]
  const chatModels = provider?.models.filter((model) => model.kind === 'chat') || []
  const model = chatModels.find((item) => item.id === current.model) || chatModels[0]
  return {
    ...current,
    provider: provider?.id || current.provider,
    providerType: current.providerType || provider?.type || 'chat',
    api:
      provider && provider.id !== current.provider
        ? provider.api || 'openai-responses'
        : current.api || provider?.api || 'openai-responses',
    model: model?.id || '',
    modelBaseUrl: model?.baseUrlOverride || '',
  }
}

export function draftForProvider(
  data: ConfigData,
  provider: ProviderConfig,
  current: ConfigDraft | null,
  preferredModel?: string,
): ConfigDraft {
  return {
    ...createConfigDraft(data, provider, preferredModel),
    thinkingLevel: current?.thinkingLevel || data.thinkingLevel || 'medium',
    toolMode: current?.toolMode || data.toolMode || 'full',
  }
}

type ConfigSettingsAction =
  | { type: 'load-succeeded'; config: ConfigData }
  | { type: 'refresh-succeeded'; config: ConfigData }
  | { type: 'replace'; config: ConfigData; draft: ConfigDraft; dirty: boolean }
  | { type: 'patch-draft'; patch: Partial<ConfigDraft> }
  | { type: 'config-updated'; config: ConfigData }
  | { type: 'set-loading'; value: boolean }
  | { type: 'set-saving'; value: boolean }
  | { type: 'set-toggling'; value: string }
  | { type: 'set-error'; value: string }
  | { type: 'save-succeeded'; config: ConfigData }

export function configSettingsReducer(
  state: ConfigSettingsState,
  action: ConfigSettingsAction,
): ConfigSettingsState {
  switch (action.type) {
    case 'load-succeeded': {
      const provider =
        action.config.providers.find((item) => item.id === action.config.provider) ||
        action.config.providers[0]
      return {
        ...state,
        config: action.config,
        draft: createConfigDraft(action.config, provider, action.config.model),
        loading: false,
        error: '',
        dirty: false,
      }
    }
    case 'refresh-succeeded':
      return {
        ...state,
        config: action.config,
        draft: refreshConfigDraft(action.config, state.draft),
      }
    case 'replace':
      return {
        ...state,
        config: action.config,
        draft: action.draft,
        loading: false,
        error: '',
        dirty: action.dirty,
      }
    case 'patch-draft':
      return state.draft
        ? { ...state, draft: { ...state.draft, ...action.patch }, dirty: true }
        : state
    case 'config-updated':
      return { ...state, config: action.config }
    case 'set-loading':
      return { ...state, loading: action.value }
    case 'set-saving':
      return { ...state, saving: action.value }
    case 'set-toggling':
      return { ...state, toggling: action.value }
    case 'set-error':
      return { ...state, error: action.value }
    case 'save-succeeded': {
      if (!state.draft) return { ...state, config: action.config, dirty: false }
      const provider =
        action.config.providers.find((item) => item.id === state.draft?.provider) ||
        action.config.providers[0]
      return {
        ...state,
        config: action.config,
        draft: {
          ...createConfigDraft(action.config, provider, state.draft.model),
          thinkingLevel: state.draft.thinkingLevel,
          toolMode: state.draft.toolMode,
        },
        error: '',
        dirty: false,
      }
    }
  }
}
