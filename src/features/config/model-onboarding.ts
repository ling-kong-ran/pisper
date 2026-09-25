// 模型新手引导的纯判定：已设置过对话模型的用户即使主动禁用连接，
// 也不应再收到首次设置提示。
export type ModelOnboardingConfig = {
  providers?: Array<{
    configured?: boolean
    models?: Array<{ kind?: string }>
  }>
}

export function hasConfiguredChatProvider(config: ModelOnboardingConfig) {
  return Boolean(
    config.providers?.some(
      (provider) => provider.configured && provider.models?.some((model) => model.kind === 'chat'),
    ),
  )
}

export function shouldShowModelOnboarding(config: ModelOnboardingConfig, dismissed: boolean) {
  return !dismissed && !hasConfiguredChatProvider(config)
}
