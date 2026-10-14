import type { ConfigData } from './config-types'

export function getProviderWorkbenchState(
  config: Pick<ConfigData, 'providers' | 'provider' | 'defaultProvider'>,
  selectedProviderId: string,
) {
  // 目录预设不属于用户连接；已添加但尚未补齐凭据的自定义连接仍需可编辑。
  const providers = config.providers.filter((provider) => provider.configured || provider.custom)
  const defaultId = config.defaultProvider || config.provider
  const selected =
    providers.find((provider) => provider.id === selectedProviderId) ??
    providers.find((provider) => provider.id === defaultId) ??
    providers[0]

  return { providers, selected, defaultId }
}
