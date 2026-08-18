// Provider 发现状态辅助：判断扫描结果里是否有可导入且未冲突的项。
import type { DiscoveryData } from './config-types'

export function providerDiscoveryHasImportable(discovery: DiscoveryData) {
  return (discovery.providers || []).some(
    (provider) => provider.importable && !provider.imported && !provider.conflict,
  )
}

export function providerDiscoveryShouldCollapse(discovery: DiscoveryData, discovering: boolean) {
  return !discovering && !providerDiscoveryHasImportable(discovery)
}

export function providerDiscoveryShouldRender(
  discovery: DiscoveryData,
  discovering: boolean,
  error: string,
) {
  const providers = discovery.providers || []
  const errors = discovery.errors || []
  const hasImportable = providerDiscoveryHasImportable(discovery)
  const hasIssue = Boolean(
    error ||
    errors.length ||
    providers.some(
      (provider) =>
        provider.conflict ||
        (!provider.imported && !provider.importable) ||
        provider.warnings?.some((warning) => warning.code !== 'login_auth_not_imported'),
    ),
  )
  return discovering || hasImportable || hasIssue
}
